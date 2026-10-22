use crate::{diagrams, model::*, providers};
use anyhow::{bail, Context, Result};
use calamine::{Data, Reader};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::Instant,
};

pub const ENGINE_VERSION: &str = concat!("doc-extractor/", env!("CARGO_PKG_VERSION"), "-schema1");
pub fn parser_fingerprint() -> String {
    hash(
        concat!(
            include_str!("extract.rs"),
            include_str!("diagrams.rs"),
            include_str!("providers.rs"),
            include_str!("adapters.rs"),
            include_str!("pdf_source.rs"),
            include_str!("layout.rs"),
            include_str!("model.rs"),
            include_str!("charts.rs"),
            include_str!("markup.rs"),
            include_str!("doctags.rs"),
            include_str!("../Cargo.lock")
        )
        .as_bytes(),
    )
}
pub struct ContextData {
    pub dir: PathBuf,
    pub options: Options,
    pub limits: Limits,
    pub document: Document,
    start: Instant,
    retained: u64,
    identity_ok: bool,
    /// Parallel batch workers write only unit files and asset sidecars; the coordinator owns
    /// checkpoints, progress manifests and the final document order.
    write_checkpoint: bool,
    /// Large table inputs drop block payloads from the in-memory document after each unit file
    /// is written; result/markdown assembly then streams the unit files. See `stream_bytes`.
    release_units: bool,
    /// Optional live progress board: unit stubs for a writer thread that emits complete,
    /// ordered checkpoints while a PDF job is still running.
    progress: Option<std::sync::Arc<ProgressBoard>>,
    /// Next asset sidecar sequence number; PDF batches set it per page so merge order is
    /// page order regardless of worker scheduling.
    next_asset_seq: u64,
}
/// Build an empty document shell for a parallel worker context.
fn empty_document(from: &Document) -> Document {
    Document {
        schema_version: from.schema_version.clone(),
        id: from.id.clone(),
        hash: from.hash.clone(),
        mime: from.mime.clone(),
        filename: from.filename.clone(),
        versions: from.versions.clone(),
        options: from.options.clone(),
        status: "complete".into(),
        metadata: json!({}),
        units: vec![],
        assets: vec![],
        warnings: vec![],
        errors: vec![],
        elapsed_ms: 0,
    }
}
pub(crate) fn parallel_context(c: &ContextData) -> ContextData {
    ContextData {
        dir: c.dir.clone(),
        options: c.options.clone(),
        limits: c.limits.clone(),
        document: empty_document(&c.document),
        start: c.start,
        retained: 0,
        identity_ok: false,
        write_checkpoint: false,
        release_units: false,
        progress: c.progress.clone(),
        next_asset_seq: 0,
    }
}

/// Live progress registry for long PDF runs. Workers publish lightweight unit stubs
/// (metadata and failures only, no blocks) by page index; a writer thread turns them into
/// complete, page-ordered checkpoint/progress/manifest snapshots on disk.
pub(crate) struct ProgressBoard {
    units: std::sync::Mutex<Vec<Option<Unit>>>,
    completed: std::sync::atomic::AtomicUsize,
    asset_generation: std::sync::atomic::AtomicUsize,
    done: std::sync::atomic::AtomicBool,
}
impl ProgressBoard {
    pub(crate) fn new(pages: usize) -> Self {
        Self {
            units: std::sync::Mutex::new((0..pages).map(|_| None).collect()),
            completed: std::sync::atomic::AtomicUsize::new(0),
            asset_generation: std::sync::atomic::AtomicUsize::new(0),
            done: std::sync::atomic::AtomicBool::new(false),
        }
    }
    fn publish(&self, unit: &Unit) {
        let mut stub = unit.clone();
        stub.blocks.clear();
        let mut units = self.units.lock().unwrap();
        if let Some(slot) = units.get_mut(unit.index) {
            if slot.is_none() {
                self.completed
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            *slot = Some(stub);
        }
    }
    fn snapshot(&self) -> Vec<Unit> {
        self.units
            .lock()
            .unwrap()
            .iter()
            .filter_map(|u| u.clone())
            .collect()
    }
}
/// Write a complete, ordered checkpoint/progress snapshot from the progress board. Safe to call
/// concurrently with extraction: unit files and asset sidecars are append-only during a run.
fn write_progress_snapshot(
    dir: &Path,
    envelope: &Document,
    board: &ProgressBoard,
    limits: &Limits,
) -> Result<()> {
    let mut doc = envelope.clone();
    doc.units = board.snapshot();
    let (assets, _) = ContextData::load_assets(dir)?;
    doc.assets = assets;
    let retained: u64 = doc.assets.iter().map(|a| a.bytes).sum();
    let cap = limits
        .output_bytes
        .saturating_sub(retained.min(limits.output_bytes));
    atomic_json_limited(&dir.join("checkpoint.json"), &doc, cap)?;
    atomic_json(&dir.join("assets.json"), &doc.assets)?;
    let warnings = doc
        .warnings
        .iter()
        .chain(doc.units.iter().flat_map(|u| u.warnings.iter()))
        .cloned()
        .collect();
    let errors = doc
        .errors
        .iter()
        .chain(doc.units.iter().flat_map(|u| u.errors.iter()))
        .cloned()
        .collect();
    atomic_json(
        &dir.join("progress.json"),
        &Progress {
            units: doc.units.len(),
            result_status: None,
            warnings,
            errors,
        },
    )
}
/// Whole-document checkpoint + manifest cadence. Per-unit files are always written; rewriting
/// the complete checkpoint for every unit is O(n^2) for large documents (500 PDF pages produced
/// ~680 MB of checkpoint traffic) and adds no resume safety beyond the unit files. Asset resume
/// data lives in per-asset sidecars, so this is only progress/status bookkeeping.
const CHECKPOINT_EVERY: usize = 100;
/// Resume artifacts (identity/units/sidecars) format. A mismatch forces a clean re-extraction
/// instead of reusing files whose meaning may have changed.
const CHECKPOINT_FORMAT: u32 = 2;

/// Stage timing counters (milliseconds), written to timings.json per extraction. Cheap atomic
/// adds around the hot calls; per-process and aggregated across batch workers.
pub static STAGE_TEXT_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static STAGE_IMAGES_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static STAGE_PARSE_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static STAGE_OCR_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static STAGE_WRITE_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static STAGE_MERGE_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static STAGE_RESULT_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static STAGE_DERIVED_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
fn time_stage(slot: &std::sync::atomic::AtomicU64, start: std::time::Instant) {
    slot.fetch_add(
        start.elapsed().as_millis() as u64,
        std::sync::atomic::Ordering::Relaxed,
    );
}
fn stage_timings() -> Value {
    let get = |c: &std::sync::atomic::AtomicU64| c.load(std::sync::atomic::Ordering::Relaxed);
    json!({
        "poppler_text_ms": get(&STAGE_TEXT_MS),
        "poppler_images_ms": get(&STAGE_IMAGES_MS),
        "page_parse_ms": get(&STAGE_PARSE_MS),
        "ocr_ms": get(&STAGE_OCR_MS),
        "unit_write_ms": get(&STAGE_WRITE_MS),
        "merge_ms": get(&STAGE_MERGE_MS),
        "result_write_ms": get(&STAGE_RESULT_MS),
        "derived_write_ms": get(&STAGE_DERIVED_MS),
    })
}
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
/// Remove temp files left by a kill during an atomic write; they are never valid state.
fn sweep_tmp_files(dir: &Path) -> Result<()> {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.contains(".tmp-") && entry.path().is_file() {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    Ok(())
}

/// Write the final result.json by streaming unit files instead of serializing the in-memory
/// unit list. Output bytes are identical to serializing `document` (unit files are serde output
/// of the same values) and peak memory stays at one unit. Falls back to direct serialization
/// when a unit file is missing or the envelope marker is absent.
pub fn write_result_streamed(dir: &Path, document: &Document, limit: u64) -> Result<()> {
    let path = dir.join("result.json");
    let mut envelope = document.clone();
    envelope.units.clear();
    let envelope_bytes = serde_json::to_vec(&envelope)?;
    let marker = b"\"units\":[]";
    let Some(pos) = find_subslice(&envelope_bytes, marker) else {
        return atomic_json_limited(&path, document, limit);
    };
    let mut files = Vec::with_capacity(document.units.len());
    for unit in &document.units {
        let file = dir.join("units").join(format!("{}.json", unit.id));
        if !file.is_file() {
            return atomic_json_limited(&path, document, limit);
        }
        files.push(file);
    }
    struct Counting<W: std::io::Write> {
        inner: std::io::BufWriter<W>,
        bytes: u64,
    }
    impl<W: std::io::Write> std::io::Write for Counting<W> {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let n = self.inner.write(buf)?;
            self.bytes += n as u64;
            Ok(n)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.inner.flush()
        }
    }
    let tmp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let mut out = Counting {
        inner: std::io::BufWriter::with_capacity(1024 * 1024, fs::File::create(&tmp)?),
        bytes: 0,
    };
    use std::io::Write;
    out.write_all(&envelope_bytes[..pos])?;
    out.write_all(b"\"units\":[")?;
    for (i, file) in files.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        let mut unit = fs::File::open(file)?;
        std::io::copy(&mut unit, &mut out)?;
    }
    out.write_all(b"]")?;
    out.write_all(&envelope_bytes[pos + marker.len()..])?;
    out.flush()?;
    let counted = out.bytes;
    out.inner.get_ref().sync_all()?;
    if counted > limit {
        let _ = fs::remove_file(&tmp);
        bail!("limit_exceeded: output bytes");
    }
    fs::rename(&tmp, &path)?;
    if let Some(parent) = path.parent() {
        fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}
pub fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub fn atomic_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let mut file = fs::File::create(&tmp)?;
    use std::io::Write;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(tmp, path)?;
    if let Some(parent) = path.parent() {
        fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}
pub fn atomic_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    atomic_bytes(path, &serde_json::to_vec(value)?)
}
/// Serialize straight into the atomic temp file while counting bytes; if the counted size
/// exceeds `limit`, the temp file is removed and a clean error returned. This avoids holding a
/// second full copy of large result documents in memory.
pub fn atomic_json_limited(path: &Path, value: &impl serde::Serialize, limit: u64) -> Result<()> {
    struct Counting<W> {
        inner: W,
        bytes: u64,
    }
    impl<W: std::io::Write> std::io::Write for Counting<W> {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let n = self.inner.write(buf)?;
            self.bytes += n as u64;
            Ok(n)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.inner.flush()
        }
    }
    let tmp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    use std::io::Write;
    let mut writer = Counting {
        inner: std::io::BufWriter::with_capacity(1024 * 1024, fs::File::create(&tmp)?),
        bytes: 0,
    };
    serde_json::to_writer(&mut writer, value)?;
    writer.inner.flush()?;
    let counted = writer.bytes;
    writer.inner.get_ref().sync_all()?;
    if counted > limit {
        let _ = fs::remove_file(&tmp);
        bail!("limit_exceeded: output bytes");
    }
    fs::rename(&tmp, path)?;
    if let Some(parent) = path.parent() {
        fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}
impl ContextData {
    /// Rebuild the asset manifest from per-asset sidecars written at creation time, in sequence
    /// order. Also returns the next sequence number. Used on resume instead of the
    /// whole-document checkpoint, which may lag behind the unit files.
    fn load_assets(dir: &Path) -> Result<(Vec<Asset>, u64)> {
        let mut entries: Vec<(u64, Asset)> = Vec::new();
        let assets = dir.join("assets");
        if assets.exists() {
            for entry in fs::read_dir(&assets)? {
                let path = entry?.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let Ok(bytes) = fs::read(&path) else { continue };
                let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                    continue;
                };
                let Ok(asset) = serde_json::from_value::<Asset>(value["asset"].clone()) else {
                    continue;
                };
                entries.push((value["seq"].as_u64().unwrap_or(0), asset));
            }
        }
        entries.sort_by_key(|(seq, _)| *seq);
        let next_seq = entries.last().map(|(seq, _)| seq + 1).unwrap_or(0);
        Ok((
            entries.into_iter().map(|(_, asset)| asset).collect(),
            next_seq,
        ))
    }
    pub(crate) fn checkpoint(&mut self, mut u: Unit) -> Result<()> {
        if self.document.units.len() >= self.limits.units {
            bail!("limit_exceeded: units");
        }
        if !u.errors.is_empty() || !u.warnings.is_empty() {
            u.status = "partial".into();
        }
        let path = self.dir.join("units").join(format!("{}.json", u.id));
        // Re-extract on resume using deterministic IDs. Atomic replacement retains each
        // successful unit on crashes. Serialization streams straight into the temp file so a
        // large chunk never allocates its full JSON image in memory on top of its cells.
        let started = Instant::now();
        atomic_json_limited(&path, &u, u64::MAX)?;
        time_stage(&STAGE_WRITE_MS, started);
        if let Some(board) = &self.progress {
            board.publish(&u);
        }
        if self.release_units {
            // Large table inputs: keep only the unit stub in memory; result/markdown assembly
            // streams units/uN.json. In-memory blocks would otherwise scale with the whole file.
            u.blocks.clear();
            self.document.units.push(u);
            // Snapshots are cheap here (stubs only), so refresh progress at every chunk.
            self.write_manifests(false)?;
            return Ok(());
        }
        self.document.units.push(u);
        let units = self.document.units.len();
        if self.write_checkpoint && (units == 1 || units % CHECKPOINT_EVERY == 0) {
            atomic_json_limited(
                &self.dir.join("checkpoint.json"),
                &self.document,
                self.limits
                    .output_bytes
                    .saturating_sub(self.retained.min(self.limits.output_bytes)),
            )?;
            self.write_manifests(false)?;
        }
        Ok(())
    }
    fn write_manifests(&self, final_result: bool) -> Result<()> {
        atomic_json(&self.dir.join("assets.json"), &self.document.assets)?;
        let warnings = self
            .document
            .warnings
            .iter()
            .chain(self.document.units.iter().flat_map(|u| u.warnings.iter()))
            .cloned()
            .collect();
        let errors = self
            .document
            .errors
            .iter()
            .chain(self.document.units.iter().flat_map(|u| u.errors.iter()))
            .cloned()
            .collect();
        atomic_json(
            &self.dir.join("progress.json"),
            &Progress {
                units: self.document.units.len(),
                result_status: if final_result {
                    Some(self.document.status.clone())
                } else {
                    None
                },
                warnings,
                errors,
            },
        )
    }
    pub(crate) fn asset(&mut self, bytes: &[u8], name: &str, u: &mut Unit) -> Result<String> {
        let h = hash(bytes);
        let id = format!("a{}", &h[..24]);
        if self.document.assets.iter().any(|a| a.id == id) {
            return Ok(id);
        }
        let retained = self
            .retained
            .checked_add(bytes.len() as u64)
            .context("asset overflow")?;
        if retained > self.limits.output_bytes {
            bail!("limit_exceeded: asset bytes");
        }
        self.retained = retained;
        fs::write(self.dir.join("assets").join(&id), bytes)?;
        let dimensions = image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .ok()
            .and_then(|r| r.into_dimensions().ok());
        // Keep the original asset regardless of its pixel count. `page_pixels` bounds OCR and
        // render work (where images are decoded/downscaled), not cold storage; rejecting here
        // discarded large scans before the page-render OCR path could recover them.
        let ext = name.rsplit('.').next().unwrap_or("");
        let provenance = Provenance {
            unit_id: u.id.clone(),
            method: "native".into(),
            source: name.into(),
            bounds: None,
            cell_range: None,
            evidence: vec![],
        };
        let asset = Asset {
            id: id.clone(),
            mime: providers::mime_for(ext).into(),
            hash: h,
            bytes: bytes.len() as u64,
            width: dimensions.map(|d| d.0),
            height: dimensions.map(|d| d.1),
            provenance,
        };
        // Sidecar metadata lets resume rebuild the asset manifest without touching the checkpoint.
        atomic_json(
            &self.dir.join("assets").join(format!("{id}.json")),
            &json!({"seq": self.next_asset_seq, "asset": &asset}),
        )?;
        self.next_asset_seq += 1;
        self.document.assets.push(asset);
        if let Some(board) = &self.progress {
            board
                .asset_generation
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(id)
    }
    /// Register an asset and its figure block without running OCR/vision.
    pub(crate) fn figure_asset(
        &mut self,
        bytes: &[u8],
        name: &str,
        u: &mut Unit,
    ) -> Result<String> {
        let id = self.asset(bytes, name, u)?;
        u.text("figure", "", name).asset_id = Some(id.clone());
        Ok(id)
    }
    /// Keep a raster of one page for downstream vision/LLM extraction. The bytes are stored
    /// once as a content-addressed asset; `pages/<unit>.<ext>` is a hard link (copy fallback)
    /// so consumers can ship the directory without parsing `result.json`.
    pub(crate) fn page_image(
        &mut self,
        bytes: &[u8],
        ext: &str,
        page: usize,
        reason: &str,
        dpi: Option<u32>,
        u: &mut Unit,
    ) -> Result<()> {
        let id = self.asset(bytes, &format!("page-{page}.{ext}"), u)?;
        self.record_page_image(&id, ext, page, reason, dpi, u)
    }
    /// Link an existing asset into `pages/` and annotate the unit block that references it.
    /// Used for standalone images whose original bytes are already the page raster.
    pub(crate) fn record_page_image(
        &mut self,
        asset_id: &str,
        ext: &str,
        page: usize,
        reason: &str,
        dpi: Option<u32>,
        u: &mut Unit,
    ) -> Result<()> {
        let pages = self.dir.join("pages");
        fs::create_dir_all(&pages)?;
        let path = format!("pages/{}.{ext}", u.id);
        let destination = self.dir.join(&path);
        if destination.exists() {
            fs::remove_file(&destination)?;
        }
        let source = self.dir.join("assets").join(asset_id);
        if fs::hard_link(&source, &destination).is_err() {
            fs::copy(&source, &destination)?;
        }
        let meta = json!({
            "page": page,
            "reason": reason,
            "path": path,
            "asset_id": asset_id,
            "dpi": dpi,
        });
        if let Some(block) = u
            .blocks
            .iter_mut()
            .find(|b| b.asset_id.as_deref() == Some(asset_id))
        {
            if !block.attributes.is_object() {
                block.attributes = json!({});
            }
            block.attributes["page_image"] = meta;
        } else {
            let block = u.text("page_image", "", "rendered page");
            block.provenance.method = "rendered".into();
            block.asset_id = Some(asset_id.to_owned());
            block.attributes = json!({"page_image": meta});
        }
        Ok(())
    }
    pub(crate) fn figure(&mut self, bytes: &[u8], name: &str, u: &mut Unit) -> Result<String> {
        let id = self.figure_asset(bytes, name, u)?;
        let path = self.dir.join("assets").join(&id);
        if self.options.ocr {
            let before = u.blocks.len();
            let ocr_started = Instant::now();
            let result = providers::ocr(&path, u, &self.options, &self.limits);
            time_stage(&STAGE_OCR_MS, ocr_started);
            if let Err(e) = result {
                u.errors.push(failure(&e, Some(u.id.clone())));
            }
            for b in &mut u.blocks[before..] {
                b.asset_id = Some(id.clone());
            }
        }
        if self.options.vision {
            match providers::vision(&path, &self.options) {
                Ok(v) => {
                    let content = v["choices"][0]["message"]["content"].as_str().unwrap_or("");
                    if crate::doctags::contains_doctags(content) {
                        let before = u.blocks.len();
                        if let Err(e) = crate::doctags::extract(content, u, 0, 0, "inferred") {
                            u.warnings
                                .push(format!("SmolDocling DocTags parse warning: {e}"));
                        }
                        for b in &mut u.blocks[before..] {
                            b.asset_id = Some(id.clone());
                        }
                    } else {
                        let b = u.text("caption", content, "local vision");
                        b.provenance.method = "inferred".into();
                        b.asset_id = Some(id.clone());
                        b.attributes = json!({"raw_response":v,"confidence_calibrated":false});
                        if self.options.interpret_diagrams {
                            match crate::diagrams::vision_graph(&b.text) {
                                Ok(graph) => {
                                    b.kind = "diagram".into();
                                    let warnings = graph.warnings.clone();
                                    b.diagram = Some(graph);
                                    u.warnings.extend(warnings);
                                }
                                Err(e) => u.warnings.push(format!(
                                    "Vision graph JSON invalid; raw inferred description retained: {e}"
                                )),
                            }
                        }
                    }
                }
                Err(e) => u.errors.push(failure(&e, Some(u.id.clone()))),
            }
        }
        Ok(id)
    }
}
pub fn failure(e: &anyhow::Error, unit_id: Option<String>) -> Failure {
    let message = format!("{e:#}");
    let code = [
        "limit_exceeded",
        "unsupported",
        "dependency_unavailable",
        "unsafe_xml",
        "unsafe_endpoint",
        "password_required",
        "corrupt",
        "provider_error",
    ]
    .iter()
    .find(|c| message.contains(**c))
    .copied()
    .unwrap_or("extraction_failed");
    Failure {
        code: code.into(),
        message,
        unit_id,
    }
}
pub fn extract(
    input: &Path,
    filename: &str,
    dir: &Path,
    options: Options,
    limits: Limits,
    id: &str,
) -> Result<Document> {
    fs::create_dir_all(dir.join("assets"))?;
    fs::create_dir_all(dir.join("units"))?;
    if options.page_images == "none" {
        // A resumed/re-run job with page images disabled must not leave stale rasters behind.
        let _ = fs::remove_dir_all(dir.join("pages"));
    }
    let bytes = fs::read(input)?;
    if bytes.len() as u64 > limits.upload_bytes {
        bail!("limit_exceeded: upload bytes");
    }
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    let ocr_threads = limits.ocr_threads;
    // Resume identity is a small immutable file; the document checkpoint may lag behind the
    // per-unit files, so it is not used to decide whether completed pages can be reused.
    let previous_identity = if ext == "pdf" {
        fs::read(dir.join("identity.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
    } else {
        None
    };
    let mut c = ContextData {
        dir: dir.to_owned(),
        options: options.clone(),
        limits,
        document: Document {
            schema_version: "1.0".into(),
            id: id.into(),
            hash: hash(&bytes),
            mime: providers::mime_for(&ext).into(),
            filename: filename.into(),
            versions: json!({"engine":ENGINE_VERSION,"parser_fingerprint":parser_fingerprint(),"providers":providers::identity(),"calamine":"0.36.1","pdf":"Poppler CLI (runtime)","ocr":"Tesseract TSV (runtime)"}),
            options,
            status: "complete".into(),
            metadata: json!({}),
            units: vec![],
            assets: vec![],
            warnings: vec![],
            errors: vec![],
            elapsed_ms: 0,
        },
        start: Instant::now(),
        retained: 0,
        identity_ok: false,
        write_checkpoint: true,
        release_units: false,
        progress: None,
        next_asset_seq: 0,
    };
    providers::configure_ocr_budget(
        std::env::var_os("DOC_OCR_SLOTS_DIR").map(std::path::PathBuf::from),
        ocr_threads,
    );
    for counter in [
        &STAGE_TEXT_MS,
        &STAGE_IMAGES_MS,
        &STAGE_PARSE_MS,
        &STAGE_OCR_MS,
        &STAGE_WRITE_MS,
        &STAGE_MERGE_MS,
        &STAGE_RESULT_MS,
        &STAGE_DERIVED_MS,
    ] {
        counter.store(0, std::sync::atomic::Ordering::Relaxed);
    }
    sweep_tmp_files(dir)?;
    if ext == "pdf" {
        if let Some(previous) = previous_identity {
            if previous["checkpoint_format"] == json!(CHECKPOINT_FORMAT)
                && previous["hash"] == json!(c.document.hash)
                && previous["versions"] == c.document.versions
                && previous["options"] == serde_json::to_value(&c.options)?
            {
                let (assets, next_seq) = ContextData::load_assets(dir)?;
                c.document.assets = assets;
                c.next_asset_seq = next_seq;
                c.retained = c.document.assets.iter().map(|a| a.bytes).sum();
                c.identity_ok = true;
            }
        }
    }
    // A non-matching identity (new options, page range or parser) re-extracts every unit, so
    // stale rasters from the previous run must not survive in `pages/`. A matching PDF identity
    // keeps them: a checkpointed unit always has its page image because it is written first.
    if c.options.page_images != "none" && !c.identity_ok {
        let _ = fs::remove_dir_all(dir.join("pages"));
        if supports_page_images(&ext) {
            fs::create_dir_all(dir.join("pages"))?;
        }
    }
    atomic_json(
        &dir.join("identity.json"),
        &json!({
            "checkpoint_format": CHECKPOINT_FORMAT,
            "hash": c.document.hash,
            "versions": c.document.versions,
            "options": &c.options,
        }),
    )?;
    let parser_error = match route(&mut c, input, &bytes, &ext) {
        Ok(()) => None,
        Err(e) => {
            let f = failure(&e, None);
            c.document.errors.push(f.clone());
            Some(f)
        }
    };
    let needs_ocr = c.document.units.iter().any(|u| u.status == "needs_ocr");
    let partial = !c.document.errors.is_empty()
        || !c.document.warnings.is_empty()
        || c.document
            .units
            .iter()
            .any(|u| !u.warnings.is_empty() || !u.errors.is_empty());
    c.document.status = if c.document.units.is_empty() && !c.document.errors.is_empty() {
        if c.document.errors.iter().any(|e| e.code == "unsupported") {
            "unsupported"
        } else {
            "failed"
        }
    } else if partial {
        "partial"
    } else if needs_ocr {
        "needs_ocr"
    } else {
        "complete"
    }
    .into();
    // Attach the engine's reason after status/strictness decisions: a bare `unsupported`
    // result without the actual message is not actionable, but the diagnostic must not
    // change the status semantics of the extraction itself.
    if let Some(f) = parser_error {
        c.document
            .warnings
            .push(format!("parser error: {}", f.message));
    }
    c.document.elapsed_ms = c.start.elapsed().as_millis();
    // Streamed assembly with a size cap: unit payloads are copied from units/uN.json one at a
    // time instead of serializing the whole in-memory document; fails cleanly instead of
    // tripping RLIMIT_FSIZE.
    let result_started = Instant::now();
    write_result_streamed(
        dir,
        &c.document,
        c.limits
            .output_bytes
            .saturating_sub(c.retained.min(c.limits.output_bytes)),
    )?;
    time_stage(&STAGE_RESULT_MS, result_started);
    c.write_manifests(true)?;
    let derived_started = Instant::now();
    let chunking = c.options.chunk_chars.map(|target| {
        let overlap = c
            .options
            .chunk_overlap
            .unwrap_or(target / 8)
            .min(target.saturating_sub(1));
        (target, overlap)
    });
    write_derived(dir, &c.document, chunking)?;
    time_stage(&STAGE_DERIVED_MS, derived_started);
    if supports_page_images(&ext) {
        write_page_image_manifest(dir, &c.document, &c.options.page_images)?;
    }
    atomic_json(&dir.join("timings.json"), &stage_timings())?;
    Ok(c.document)
}
/// Index the page rasters referenced by unit blocks. Written as `pages/manifest.json` next to
/// the images so an LLM pipeline can consume the directory without parsing the full result.
fn write_page_image_manifest(dir: &Path, doc: &Document, mode: &str) -> Result<()> {
    if mode == "none" {
        return Ok(());
    }
    fs::create_dir_all(dir.join("pages"))?;
    let mut pages = Vec::new();
    for unit in &doc.units {
        for block in &unit.blocks {
            let Some(meta) = block.attributes.get("page_image") else {
                continue;
            };
            let Some(asset_id) = meta["asset_id"].as_str() else {
                continue;
            };
            let Some(asset) = doc.assets.iter().find(|a| a.id == asset_id) else {
                continue;
            };
            pages.push(PageImage {
                unit_id: unit.id.clone(),
                unit_name: unit.name.clone(),
                kind: unit.kind.clone(),
                page: meta["page"].as_u64().unwrap_or(1) as usize,
                reason: meta["reason"].as_str().unwrap_or("requested").to_owned(),
                path: meta["path"].as_str().unwrap_or_default().to_owned(),
                asset_id: asset_id.to_owned(),
                mime: asset.mime.clone(),
                width: asset.width,
                height: asset.height,
                bytes: asset.bytes,
                hash: asset.hash.clone(),
                dpi: meta["dpi"].as_u64().map(|dpi| dpi as u32),
            });
        }
    }
    atomic_json(
        &dir.join("pages/manifest.json"),
        &PageImageManifest {
            mode: mode.to_owned(),
            count: pages.len(),
            pages,
        },
    )
}
/// Load a unit's full payload for derivation: released units read their unit file, everything
/// else is used from memory. One unit is materialized at a time.
fn load_unit_payload(dir: &Path, unit: &Unit) -> Result<Unit> {
    if unit.blocks.is_empty() {
        let path = dir.join("units").join(format!("{}.json", unit.id));
        if let Ok(bytes) = fs::read(&path) {
            if let Ok(full) = serde_json::from_slice::<Unit>(&bytes) {
                return Ok(full);
            }
        }
    }
    Ok(unit.clone())
}
/// Stream result.md/result.txt unit by unit so multi-million-row tables never build the whole
/// derived text in memory. Output is byte-identical to `model::markdown`/`plaintext`.
fn write_derived(dir: &Path, doc: &Document, chunking: Option<(usize, usize)>) -> Result<()> {
    use std::io::{BufWriter, Write};
    let mut md = BufWriter::with_capacity(256 * 1024, fs::File::create(dir.join("result.md"))?);
    let mut txt = BufWriter::with_capacity(256 * 1024, fs::File::create(dir.join("result.txt"))?);
    let mut jsonl =
        BufWriter::with_capacity(256 * 1024, fs::File::create(dir.join("result.jsonl"))?);
    let mut html = BufWriter::with_capacity(256 * 1024, fs::File::create(dir.join("result.html"))?);
    let mut chunks = match chunking {
        Some((target, overlap)) => Some((
            BufWriter::with_capacity(
                256 * 1024,
                fs::File::create(dir.join("result.chunks.jsonl"))?,
            ),
            target,
            overlap,
        )),
        None => None,
    };
    write!(md, "# {}\n\nStatus: {}\n\n", doc.filename, doc.status)?;
    write!(html, "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<title>{}</title>\n<style>body{{font:14px/1.5 -apple-system,system-ui,sans-serif;margin:2rem auto;max-width:70rem;padding:0 1rem}}table{{border-collapse:collapse;margin:.6rem 0}}td,th{{border:1px solid #bbb;padding:.2rem .45rem;vertical-align:top;max-width:28rem;overflow-wrap:anywhere}}tbody tr:nth-child(even){{background:rgba(127,127,127,.09)}}pre,p{{overflow-wrap:anywhere}}</style>\n</head>\n<body>\n<h1>{}</h1>\n<p>Status: {}</p>\n",
        crate::model::html_escape(&doc.filename), crate::model::html_escape(&doc.filename), crate::model::html_escape(&doc.status))?;
    let mut buffer = String::new();
    for unit in &doc.units {
        let full = load_unit_payload(dir, unit)?;
        buffer.clear();
        crate::model::unit_markdown(&full, &mut buffer);
        md.write_all(buffer.as_bytes())?;
        buffer.clear();
        crate::model::unit_plaintext(&full, &mut buffer);
        txt.write_all(buffer.as_bytes())?;
        buffer.clear();
        crate::model::unit_jsonl(&full, &mut buffer)?;
        jsonl.write_all(buffer.as_bytes())?;
        buffer.clear();
        crate::model::unit_html(&full, &mut buffer);
        html.write_all(buffer.as_bytes())?;
        if let Some((writer, target, overlap)) = &mut chunks {
            buffer.clear();
            crate::model::unit_chunks(&full, *target, *overlap, &mut buffer)?;
            writer.write_all(buffer.as_bytes())?;
        }
    }
    write!(html, "</body>\n</html>\n")?;
    md.flush()?;
    txt.flush()?;
    jsonl.flush()?;
    html.flush()?;
    if let Some((writer, _, _)) = &mut chunks {
        writer.flush()?;
    }
    Ok(())
}
fn route(c: &mut ContextData, input: &Path, bytes: &[u8], ext: &str) -> Result<()> {
    if c.options.parser == "xberg" {
        use crate::adapters::FormatAdapter;
        return crate::adapters::Xberg.extract(c, input, ext);
    }
    if c.options.parser != "direct" {
        bail!("unsupported parser");
    }
    match ext {
        "csv" | "tsv" => delimited(c, bytes, ext),
        "xlsx" | "xls" | "xlsm" | "xlsb" | "ods" => spreadsheet(c, input, bytes, ext),
        "docx" | "pptx" | "odt" | "odp" | "epub" | "vsdx" => office(c, bytes, ext),
        "pdf" => pdf(c, input),
        "png" | "jpg" | "jpeg" | "tif" | "tiff" | "bmp" | "gif" | "webp" => {
            let (w, h) = image::ImageReader::open(input)?
                .with_guessed_format()?
                .into_dimensions()?;
            if w as u64 * h as u64 > c.limits.page_pixels {
                bail!("limit_exceeded: image pixels");
            }
            let mut u = Unit::new(0, "image", "Image");
            let asset = c.figure(bytes, &format!("original.{ext}"), &mut u)?;
            if ext == "png" {
                if let Some(source) = diagrams::embedded_png(bytes, c.limits.archive_bytes)? {
                    for (_, g) in diagrams::drawio(&source, c.limits.archive_bytes)? {
                        u.text("diagram", "", "embedded PNG mxfile").diagram = Some(g);
                    }
                }
            }
            if !c.options.ocr {
                u.status = "needs_ocr".into();
            }
            let reason = if c.options.page_images == "all" {
                Some("requested")
            } else if c.options.page_images == "failed" {
                let recovered = c.options.ocr
                    && u.errors.is_empty()
                    && u.blocks
                        .iter()
                        .any(|b| b.provenance.method == "ocr" && !b.text.trim().is_empty());
                if recovered {
                    None
                } else {
                    Some(page_image_reason(&u, 0).unwrap_or("no_text"))
                }
            } else {
                None
            };
            if let Some(reason) = reason {
                c.record_page_image(&asset, ext, 1, reason, None, &mut u)?;
            }
            c.checkpoint(u)
        }
        "drawio" => drawio(c, std::str::from_utf8(bytes)?),
        "mmd" | "mermaid" | "dot" | "gv" | "puml" | "plantuml" | "dbml" => {
            let f = match ext {
                "mmd" => "mermaid",
                "gv" => "dot",
                "puml" => "plantuml",
                v => v,
            };
            let g = diagrams::source(std::str::from_utf8(bytes)?, f)?;
            let mut u = Unit::new(0, "diagram", "Source diagram");
            u.warnings.extend(g.warnings.clone());
            let id = c.asset(bytes, &format!("source.{ext}"), &mut u)?;
            let b = u.text("diagram", std::str::from_utf8(bytes)?, "source");
            b.diagram = Some(g);
            b.asset_id = Some(id);
            c.checkpoint(u)
        }
        "svg" => svg(c, std::str::from_utf8(bytes)?, bytes),
        "doctags" | "dt" => {
            let mut u = Unit::new(0, "page", "DocTags Page");
            crate::doctags::extract(std::str::from_utf8(bytes)?, &mut u, 0, 0, "native")?;
            if !u.warnings.is_empty() {
                u.status = "partial".into();
            }
            c.checkpoint(u)
        }
        "eml" => email(c, bytes),
        "rtf" => rtf(c, bytes),
        "txt" | "md" | "markdown" | "json" | "xml" | "html" | "htm" => text_document(c, bytes, ext),
        _ => bail!("unsupported: format {ext}; native extraction adapter not provisioned"),
    }
}
fn drawio(c: &mut ContextData, s: &str) -> Result<()> {
    for (i, (name, g)) in diagrams::drawio(s, c.limits.archive_bytes)?
        .into_iter()
        .enumerate()
    {
        let mut u = Unit::new(i, "diagram", name);
        u.text("diagram", "", "mxGraphModel").diagram = Some(g);
        let id = c.asset(s.as_bytes(), "source.drawio", &mut u)?;
        u.blocks[0].asset_id = Some(id);
        c.checkpoint(u)?;
    }
    Ok(())
}
fn delimited(c: &mut ContextData, bytes: &[u8], ext: &str) -> Result<()> {
    std::str::from_utf8(bytes).context("unsupported: CSV encoding must be UTF-8")?;
    let delimiter = c
        .options
        .delimiter
        .unwrap_or(if ext == "tsv" { '\t' } else { ',' });
    if !delimiter.is_ascii() || delimiter == '\n' || delimiter == '\r' {
        bail!("unsupported delimiter");
    }
    // Large files are split into bounded chunk units so memory stays proportional to a chunk,
    // not the whole file. Small files keep a single unit named "Records". Inputs above
    // `limits.stream_bytes` additionally release block payloads after each unit file is
    // written; result/markdown assembly then streams those files.
    const CHUNK_ROWS: usize = 50_000;
    const CHUNK_CELLS: usize = 250_000;
    c.release_units = bytes.len() as u64 > c.limits.stream_bytes;
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .delimiter(delimiter as u8)
        .flexible(c.options.flexible_rows)
        .from_reader(bytes);
    let mut table = Table {
        cells: vec![],
        rows: 0,
        columns: 0,
        headers: vec![],
        compact: c.options.compact_tables,
    };
    let mut chunk_start = 0usize; // absolute 0-based index of the chunk's first row
    let mut chunks = 0usize;
    let mut total_rows = 0usize;
    let flush =
        |c: &mut ContextData, table: &mut Table, chunk_start: usize, chunks: usize| -> Result<()> {
            let end = chunk_start + table.rows;
            let mut u = Unit::new(
                chunks,
                "sheet",
                if chunks == 0 {
                    "Records".to_string()
                } else {
                    format!("Records {}", chunks + 1)
                },
            );
            let source = format!("records rows {}-{}", chunk_start + 1, end);
            let b = u.text("table", "", source);
            b.provenance.cell_range = Some(format!("rows {}-{}", chunk_start + 1, end));
            b.table = Some(std::mem::replace(
                table,
                Table {
                    cells: vec![],
                    rows: 0,
                    columns: 0,
                    headers: vec![],
                    compact: table.compact,
                },
            ));
            c.checkpoint(u)?;
            Ok(())
        };
    for (r, row) in reader.records().enumerate() {
        let row = row.context("corrupt CSV")?;
        if row.len() > CHUNK_CELLS {
            bail!("limit_exceeded: CSV row cells");
        }
        table.columns = table.columns.max(row.len());
        table.rows += 1;
        total_rows = r + 1;
        for (col, raw) in row.iter().enumerate() {
            table.cells.push(Cell {
                row: (r - chunk_start) as u32,
                column: col as u32,
                raw: raw.into(),
                value: json!(raw),
                formula: None,
                cached_value: None,
                row_span: 1,
                column_span: 1,
            });
        }
        if table.rows >= CHUNK_ROWS || table.cells.len() >= CHUNK_CELLS {
            flush(c, &mut table, chunk_start, chunks)?;
            chunks += 1;
            chunk_start = r + 1;
        }
    }
    if !table.cells.is_empty() {
        flush(c, &mut table, chunk_start, chunks)?;
        chunks += 1;
    }
    c.document.metadata = json!({"encoding":"UTF-8","delimiter":delimiter.to_string(),"header_inference":false,"flexible_rows":c.options.flexible_rows,"rows":total_rows,"chunks":chunks});
    Ok(())
}
fn data_value(d: &Data) -> Value {
    match d {
        Data::Empty => Value::Null,
        Data::Int(v) => json!(v),
        Data::Float(v) => json!(v),
        Data::Bool(v) => json!(v),
        Data::DateTime(v) => json!({"type":"excel_datetime","serial":v.as_f64()}),
        Data::DateTimeIso(v) => json!({"type":"datetime","value":v}),
        Data::DurationIso(v) => json!({"type":"duration","value":v}),
        Data::Error(v) => json!({"type":"error","value":v.to_string()}),
        _ => json!(d.to_string()),
    }
}
fn spreadsheet(c: &mut ContextData, input: &Path, bytes: &[u8], ext: &str) -> Result<()> {
    let archive = if bytes.starts_with(b"PK") {
        Some(archive(bytes, &c.limits)?)
    } else {
        None
    };
    let mut book = calamine::open_workbook_auto(input).context("corrupt spreadsheet")?;
    let mut sheet_paths = std::collections::HashMap::new();
    if let Some(entries) = &archive {
        if let Some(workbook) = entries.get("xl/workbook.xml") {
            let doc = diagrams::xml(std::str::from_utf8(workbook)?)?;
            let relationships = relationship_map(entries, "xl/_rels/workbook.xml.rels")?;
            for sheet in doc.descendants().filter(|n| n.has_tag_name("sheet")) {
                if let (Some(name), Some(id)) = (
                    sheet.attribute("name"),
                    sheet
                        .attributes()
                        .find(|a| a.name() == "id" && a.namespace().is_some())
                        .map(|a| a.value()),
                ) {
                    if let Some(target) = relationships.get(id) {
                        sheet_paths
                            .insert(name.to_owned(), package_target("xl/workbook.xml", target)?);
                    }
                }
            }
        }
    }

    for (i, name) in book.sheet_names().to_vec().into_iter().enumerate() {
        let mut u = Unit::new(i, "sheet", &name);
        match book.worksheet_range(&name) {
            Err(e) => {
                u.errors.push(Failure {
                    code: "corrupt".into(),
                    message: e.to_string(),
                    unit_id: Some(u.id.clone()),
                });
            }
            Ok(range) => {
                let formulas = book.worksheet_formula(&name).ok();
                let (row_offset, col_offset) = range.start().unwrap_or((0, 0));
                let mut table = Table {
                    cells: vec![],
                    rows: range.height(),
                    columns: range.width(),
                    headers: vec![],
                    compact: false,
                };
                for (r, col, d) in range.cells() {
                    let formula = formulas
                        .as_ref()
                        .and_then(|f| f.get_value((r as u32 + row_offset, col as u32 + col_offset)))
                        .filter(|s| !s.is_empty())
                        .cloned();
                    let value = data_value(d);
                    table.cells.push(Cell {
                        row: r as u32,
                        column: col as u32,
                        raw: d.to_string(),
                        cached_value: formula.as_ref().map(|_| value.clone()),
                        value,
                        formula,
                        row_span: 1,
                        column_span: 1,
                    });
                    if table.cells.len() > 1_000_000 {
                        bail!("limit_exceeded: cells");
                    }
                }
                if let Some(entries) = &archive {
                    if ext == "xlsx" || ext == "xlsm" {
                        if let Some(b) =
                            entries.get(sheet_paths.get(&name).map(String::as_str).unwrap_or(""))
                        {
                            let s = std::str::from_utf8(b)?;
                            let doc = diagrams::xml(s)?;
                            let cell_index = table
                                .cells
                                .iter()
                                .enumerate()
                                .map(|(i, cell)| {
                                    ((cell.row + row_offset, cell.column + col_offset), i)
                                })
                                .collect::<std::collections::HashMap<_, _>>();
                            for node in doc.descendants().filter(|n| n.has_tag_name("c")) {
                                if ![Some("s"), Some("inlineStr")].contains(&node.attribute("t")) {
                                    if let Some((r, col)) = node.attribute("r").and_then(cell_coord)
                                    {
                                        if let Some(&index) = cell_index.get(&(r, col)) {
                                            let cell = &mut table.cells[index];
                                            if let Some(raw) = node
                                                .children()
                                                .find(|n| n.has_tag_name("v"))
                                                .and_then(|n| n.text())
                                            {
                                                cell.raw = raw.to_owned();
                                            }
                                        }
                                    }
                                }
                            }
                            for n in doc.descendants().filter(|n| n.has_tag_name("mergeCell")) {
                                if let Some(reference) = n.attribute("ref") {
                                    let coords: Vec<_> =
                                        reference.split(':').map(cell_coord).collect();
                                    if coords.len() == 2 {
                                        if let (Some((r1, c1)), Some((r2, c2))) =
                                            (coords[0], coords[1])
                                        {
                                            if let Some(&index) = cell_index.get(&(r1, c1)) {
                                                let cell = &mut table.cells[index];
                                                cell.row_span = r2 - r1 + 1;
                                                cell.column_span = c2 - c1 + 1;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                let block = u.text("table", "", format!("sheet:{name}"));
                block.provenance.cell_range = range.end().map(|(r, col)| {
                    format!(
                        "{}:{}",
                        cell_address(row_offset, col_offset),
                        cell_address(r, col)
                    )
                });
                block.table = Some(table);
            }
        }
        if i == 0 {
            let id = c.asset(bytes, &format!("original.{ext}"), &mut u)?;
            u.text("unknown", "Original workbook", "source").asset_id = Some(id);
            if let Some(entries) = &archive {
                preserve_package(c, entries, &mut u)?;
            }
        }
        c.checkpoint(u)?;
    }
    c.document.metadata = json!({"formula_recalculation":false,"sheet_xml_paths":sheet_paths});
    Ok(())
}
fn cell_coord(s: &str) -> Option<(u32, u32)> {
    let mut column = 0u32;
    let mut digits = String::new();
    for ch in s.chars().filter(|c| *c != '$') {
        if ch.is_ascii_alphabetic() {
            column = column
                .checked_mul(26)?
                .checked_add(ch.to_ascii_uppercase() as u32 - 'A' as u32 + 1)?;
        } else if ch.is_ascii_digit() {
            digits.push(ch);
        } else {
            return None;
        }
    }
    Some((
        digits.parse::<u32>().ok()?.checked_sub(1)?,
        column.checked_sub(1)?,
    ))
}
fn archive(bytes: &[u8], limits: &Limits) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut zip =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).context("corrupt ZIP package")?;
    if zip.len() > limits.archive_entries {
        bail!("limit_exceeded: archive entries");
    }
    let mut total = 0u64;
    let mut entries = BTreeMap::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| {
            if e.to_string().to_lowercase().contains("password") {
                anyhow::anyhow!("password_required: {e}")
            } else {
                anyhow::anyhow!("corrupt archive: {e}")
            }
        })?;
        let name = f.name().to_owned();
        if f.enclosed_name().is_none()
            || name.contains('\\')
            || name.split('/').any(|p| p == "..")
            || f.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000)
        {
            bail!("unsafe archive path");
        }
        total = total.checked_add(f.size()).context("archive overflow")?;
        if total > limits.archive_bytes {
            bail!("limit_exceeded: archive bytes");
        }
        if f.is_dir() {
            continue;
        }
        let mut b = vec![];
        f.by_ref()
            .take(limits.archive_bytes + 1)
            .read_to_end(&mut b)?;
        if b.len() as u64 > limits.archive_bytes {
            bail!("limit_exceeded: archive entry");
        }
        if entries.insert(name, b).is_some() {
            bail!("corrupt: duplicate ZIP entry");
        }
    }
    Ok(entries)
}
fn preserve_package(
    c: &mut ContextData,
    entries: &BTreeMap<String, Vec<u8>>,
    u: &mut Unit,
) -> Result<()> {
    for (name, b) in entries {
        if name.contains("/media/")
            || name.starts_with("Pictures/")
            || name.contains("/embeddings/")
        {
            c.figure(b, name, u)?;
        } else if name.contains("/diagrams/")
            || name.contains("/charts/")
            || name.ends_with(".rels")
            || name.starts_with("visio/")
            || name == "docProps/core.xml"
        {
            let id = c.asset(b, name, u)?;
            u.text("unknown", "", name).asset_id = Some(id);
            if name.contains("/charts/") && name.ends_with(".xml") && !name.contains("/_rels/") {
                crate::charts::extract(b, name, u)?;
            }
        }
    }
    Ok(())
}
fn node_text(n: roxmltree::Node<'_, '_>) -> String {
    n.descendants()
        .filter(|v| v.is_text())
        .filter_map(|v| v.text())
        .collect::<Vec<_>>()
        .join("")
}
fn office(c: &mut ContextData, bytes: &[u8], ext: &str) -> Result<()> {
    let entries = archive(bytes, &c.limits)?;
    if let Some(b) = entries.get("docProps/core.xml") {
        let doc = diagrams::xml(std::str::from_utf8(b)?)?;
        let metadata = doc
            .root_element()
            .children()
            .filter(|n| n.is_element())
            .map(|n| (n.tag_name().name().to_owned(), node_text(n)))
            .collect::<BTreeMap<_, _>>();
        c.document.metadata = json!(metadata);
    }
    match ext {
        "docx" => {
            let s = std::str::from_utf8(
                entries
                    .get("word/document.xml")
                    .context("corrupt DOCX: missing document.xml")?,
            )?;
            let doc = diagrams::xml(s)?;
            let mut u = Unit::new(0, "section", "Logical section 1");
            let body = doc
                .descendants()
                .find(|n| n.has_tag_name("body"))
                .context("corrupt DOCX body")?;
            for n in body.children().filter(|n| n.is_element()) {
                if n.has_tag_name("p") {
                    word_paragraph(&mut u, n, "word/document.xml");
                    if n.descendants().any(|n| n.has_tag_name("sectPr")) {
                        c.checkpoint(u)?;
                        u = Unit::new(
                            c.document.units.len(),
                            "section",
                            format!("Logical section {}", c.document.units.len() + 1),
                        );
                    }
                } else if n.has_tag_name("tbl") {
                    word_table(&mut u, n, "word/document.xml");
                }
            }
            for (name, b) in &entries {
                if name.starts_with("word/")
                    && (name.contains("footnotes")
                        || name.contains("endnotes")
                        || name.contains("header")
                        || name.contains("footer")
                        || name.contains("comments"))
                    && name.ends_with(".xml")
                {
                    let d = diagrams::xml(std::str::from_utf8(b)?)?;
                    for p in d.descendants().filter(|n| n.has_tag_name("p")) {
                        let block = u.text("footnote", node_text(p), name);
                        block.attributes = json!({"logical_auxiliary":true});
                    }
                }
            }
            preserve_package(c, &entries, &mut u)?;
            u.warnings.push("Word units are logical sections; drawings retain relationship XML, without rendered placement or SmartArt semantics".into());
            c.checkpoint(u)?;
        }
        "pptx" => {
            let presentation = entries
                .get("ppt/presentation.xml")
                .context("corrupt PPTX presentation")?;
            let doc = diagrams::xml(std::str::from_utf8(presentation)?)?;
            let rels = relationship_map(&entries, "ppt/_rels/presentation.xml.rels")?;
            let paths = doc
                .descendants()
                .filter(|n| n.has_tag_name("sldId"))
                .filter_map(|n| {
                    n.attributes()
                        .find(|a| a.name() == "id" && a.namespace().is_some())
                        .and_then(|a| rels.get(a.value()).cloned())
                })
                .collect::<Vec<_>>();
            for (i, path) in paths.iter().enumerate() {
                let name = package_target("ppt/presentation.xml", path)?;
                let b = entries.get(&name).context("corrupt PPTX missing slide")?;
                let d = diagrams::xml(std::str::from_utf8(b)?)?;
                let mut u = Unit::new(i, "slide", format!("Slide {}", i + 1));
                for n in d
                    .descendants()
                    .filter(|n| n.has_tag_name("sp") || n.has_tag_name("graphicFrame"))
                {
                    if n.descendants().any(|n| n.has_tag_name("tbl")) {
                        if let Some(t) = n.descendants().find(|n| n.has_tag_name("tbl")) {
                            word_table(&mut u, t, &name);
                        }
                    } else {
                        let text = n
                            .descendants()
                            .filter(|n| n.has_tag_name("t"))
                            .map(node_text)
                            .collect::<Vec<_>>()
                            .join(" ");
                        let block = u.text("text", text, &name);
                        block.attributes =
                            json!({"source_xml":&std::str::from_utf8(b)?[n.range()]});
                    }
                }
                let relationship_path =
                    format!("ppt/slides/_rels/{}.rels", name.rsplit('/').next().unwrap());
                for target in relationship_map(&entries, &relationship_path)?.values() {
                    let target = package_target(&name, target)?;
                    if let Some(b) = entries.get(&target) {
                        if target.contains("/media/") {
                            c.figure(b, &target, &mut u)?;
                        } else if target.contains("notesSlides/") {
                            let notes = diagrams::xml(std::str::from_utf8(b)?)?;
                            u.text(
                                "footnote",
                                notes
                                    .descendants()
                                    .filter(|n| n.has_tag_name("t"))
                                    .map(node_text)
                                    .collect::<Vec<_>>()
                                    .join(" "),
                                &target,
                            );
                        }
                    }
                }
                c.checkpoint(u)?;
            }
            let mut u = Unit::new(
                c.document.units.len(),
                "assets",
                "Presentation source assets",
            );
            preserve_package(c, &entries, &mut u)?;
            if !u.blocks.is_empty() {
                c.checkpoint(u)?;
            }
        }
        "odt" | "odp" => {
            let b = entries.get("content.xml").context("corrupt ODF content")?;
            let d = diagrams::xml(std::str::from_utf8(b)?)?;
            let roots = if ext == "odp" {
                d.descendants()
                    .filter(|n| n.has_tag_name("page"))
                    .collect::<Vec<_>>()
            } else {
                vec![d.root_element()]
            };
            for (i, root) in roots.into_iter().enumerate() {
                let mut u = Unit::new(
                    i,
                    if ext == "odp" { "slide" } else { "section" },
                    format!("Logical unit {}", i + 1),
                );
                for n in root
                    .descendants()
                    .filter(|n| n.has_tag_name("p") || n.has_tag_name("h"))
                {
                    if n.ancestors().any(|p| p.has_tag_name("table-cell")) {
                        continue;
                    }
                    u.text(
                        if n.has_tag_name("h") {
                            "heading"
                        } else {
                            "text"
                        },
                        node_text(n),
                        "content.xml",
                    );
                }
                for n in root.descendants().filter(|n| n.has_tag_name("table")) {
                    word_table(&mut u, n, "content.xml");
                }
                preserve_package(c, &entries, &mut u)?;
                u.warnings.push("ODF source order retained; rendered layout and repeated table cells may require a dedicated adapter".into());
                c.checkpoint(u)?;
            }
        }
        "epub" => {
            let container = entries
                .get("META-INF/container.xml")
                .context("corrupt EPUB container")?;
            let d = diagrams::xml(std::str::from_utf8(container)?)?;
            let root = d
                .descendants()
                .find(|n| n.has_tag_name("rootfile"))
                .and_then(|n| n.attribute("full-path"))
                .context("corrupt EPUB rootfile")?;
            let opf = diagrams::xml(std::str::from_utf8(
                entries.get(root).context("missing EPUB OPF")?,
            )?)?;
            let manifest = opf
                .descendants()
                .filter(|n| n.has_tag_name("item"))
                .filter_map(|n| Some((n.attribute("id")?, n.attribute("href")?)))
                .collect::<BTreeMap<_, _>>();
            for (i, n) in opf
                .descendants()
                .filter(|n| n.has_tag_name("itemref"))
                .enumerate()
            {
                let href = manifest
                    .get(n.attribute("idref").unwrap_or(""))
                    .context("missing EPUB spine item")?;
                let path = package_target(root, href)?;
                let bytes = entries.get(&path).context("missing EPUB chapter")?;
                let mut u = Unit::new(i, "section", format!("Chapter {}", i + 1));
                html_blocks(&mut u, std::str::from_utf8(bytes)?, &path);
                c.checkpoint(u)?;
            }
            let mut u = Unit::new(c.document.units.len(), "assets", "EPUB media");
            for (name, b) in &entries {
                if ["png", "jpg", "jpeg", "svg", "gif"]
                    .iter()
                    .any(|ext| name.ends_with(&format!(".{ext}")))
                {
                    let id = c.asset(b, name, &mut u)?;
                    u.text("figure", "", name).asset_id = Some(id);
                }
            }
            if !u.blocks.is_empty() {
                c.checkpoint(u)?;
            }
        }
        "vsdx" => {
            let mut u = Unit::new(0, "source", "Visio source");
            preserve_package(c, &entries, &mut u)?;
            u.warnings.push(
                "Visio source XML preserved; relationship graph interpretation is unsupported"
                    .into(),
            );
            c.checkpoint(u)?;
        }
        _ => unreachable!(),
    }
    Ok(())
}
fn relationship_map(
    entries: &BTreeMap<String, Vec<u8>>,
    path: &str,
) -> Result<BTreeMap<String, String>> {
    let mut map = BTreeMap::new();
    if let Some(b) = entries.get(path) {
        let doc = diagrams::xml(std::str::from_utf8(b)?)?;
        for n in doc.descendants().filter(|n| n.has_tag_name("Relationship")) {
            if n.attribute("TargetMode") == Some("External") {
                continue;
            }
            if let (Some(id), Some(target)) = (n.attribute("Id"), n.attribute("Target")) {
                map.insert(id.into(), target.into());
            }
        }
    }
    Ok(map)
}
fn package_target(base: &str, target: &str) -> Result<String> {
    if target.contains(':') || target.contains('\\') {
        bail!("unsupported external relationship");
    }
    let mut parts = if target.starts_with('/') {
        vec![]
    } else {
        base.split('/').map(str::to_owned).collect::<Vec<_>>()
    };
    if !target.starts_with('/') {
        parts.pop();
    }
    for p in target.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    bail!("unsafe relationship traversal");
                }
            }
            _ => parts.push(p.into()),
        }
    }
    Ok(parts.join("/"))
}
fn word_paragraph(u: &mut Unit, n: roxmltree::Node<'_, '_>, source: &str) {
    let mut text = String::new();
    for t in n.descendants() {
        if t.has_tag_name("t") {
            text.push_str(&node_text(t));
        } else if t.has_tag_name("tab") {
            text.push('\t');
        } else if t.has_tag_name("br") {
            text.push('\n');
        }
    }
    let style = n
        .descendants()
        .find(|n| n.has_tag_name("pStyle"))
        .and_then(|n| {
            n.attributes()
                .find(|a| a.name() == "val")
                .map(|a| a.value())
        });
    let kind = if n.descendants().any(|n| n.has_tag_name("numPr")) {
        "list"
    } else if style.is_some_and(|s| s.to_lowercase().starts_with("heading")) {
        "heading"
    } else {
        "text"
    };
    let block = u.text(kind, text, source);
    block.attributes = json!({"style":style,"hyperlinks":n.descendants().filter(|n|n.has_tag_name("hyperlink")).map(|n|json!({"text":node_text(n),"relationship_id":n.attributes().find(|a|a.name()=="id").map(|a|a.value())})).collect::<Vec<_>>()});
}
fn word_table(u: &mut Unit, n: roxmltree::Node<'_, '_>, source: &str) {
    let mut table = Table {
        cells: vec![],
        rows: 0,
        columns: 0,
        headers: vec![],
        compact: false,
    };
    let mut vertical = std::collections::HashMap::<u32, usize>::new();
    for (r, row) in n
        .children()
        .filter(|n| n.has_tag_name("tr") || n.has_tag_name("table-row"))
        .enumerate()
    {
        let mut col = 0;
        for cell in row
            .children()
            .filter(|n| n.has_tag_name("tc") || n.has_tag_name("table-cell"))
        {
            let span = cell
                .descendants()
                .find(|n| n.has_tag_name("gridSpan"))
                .and_then(|n| n.attributes().find(|a| a.name() == "val"))
                .and_then(|a| a.value().parse().ok())
                .or_else(|| {
                    cell.attributes()
                        .find(|a| a.name() == "number-columns-spanned")
                        .and_then(|a| a.value().parse().ok())
                })
                .or_else(|| cell.attribute("gridSpan").and_then(|v| v.parse().ok()))
                .unwrap_or(1);
            let merge = cell.descendants().find(|n| n.has_tag_name("vMerge"));
            if let Some(merge) = merge {
                let restart = merge
                    .attributes()
                    .find(|a| a.name() == "val")
                    .is_some_and(|a| a.value() == "restart");
                if restart {
                    vertical.insert(col, table.cells.len());
                } else if let Some(index) = vertical.get(&col) {
                    table.cells[*index].row_span += 1;
                }
            } else {
                vertical.remove(&col);
            }
            let raw = node_text(cell);
            table.cells.push(Cell {
                row: r as u32,
                column: col,
                raw: raw.clone(),
                value: json!(raw),
                formula: cell
                    .attributes()
                    .find(|a| a.name() == "formula")
                    .map(|a| a.value().into()),
                cached_value: None,
                row_span: cell
                    .attributes()
                    .find(|a| a.name() == "number-rows-spanned" || a.name() == "rowSpan")
                    .and_then(|a| a.value().parse().ok())
                    .unwrap_or(1),
                column_span: span,
            });
            col += span;
        }
        table.rows = r + 1;
        table.columns = table.columns.max(col as usize);
        if row.descendants().any(|n| n.has_tag_name("tblHeader")) {
            table.headers.push(r);
        }
    }
    let b = u.text("table", "", source);
    b.table = Some(table);
    b.attributes = json!({"source_xml":&n.document().input_text()[n.range()]});
}
fn html_blocks(u: &mut Unit, s: &str, source: &str) {
    let doc = scraper::Html::parse_document(s);
    let sel = scraper::Selector::parse("h1,h2,h3,h4,h5,h6,p,li,table,a,img").unwrap();
    for n in doc.select(&sel) {
        let tag = n.value().name();
        if n.ancestors()
            .filter_map(scraper::ElementRef::wrap)
            .any(|p| {
                p.value().name() == "table" || p.value().name() == "li" || p.value().name() == "p"
            })
        {
            continue;
        }
        if tag == "table" {
            let mut table = Table {
                cells: vec![],
                rows: 0,
                columns: 0,
                headers: vec![],
                compact: false,
            };
            let rows = scraper::Selector::parse("tr").unwrap();
            let cells = scraper::Selector::parse("th,td").unwrap();
            for (r, row) in n.select(&rows).enumerate() {
                let mut col = 0;
                for cell in row.select(&cells) {
                    let raw = cell.text().collect::<Vec<_>>().join(" ");
                    let span = cell
                        .value()
                        .attr("colspan")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(1);
                    table.cells.push(Cell {
                        row: r as u32,
                        column: col,
                        raw: raw.clone(),
                        value: json!(raw),
                        formula: None,
                        cached_value: None,
                        row_span: cell
                            .value()
                            .attr("rowspan")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(1),
                        column_span: span,
                    });
                    col += span;
                }
                table.rows = r + 1;
                table.columns = table.columns.max(col as usize);
            }
            u.text("table", "", source).table = Some(table);
        } else {
            let kind = if tag.starts_with('h') {
                "heading"
            } else if tag == "li" {
                "list"
            } else if tag == "img" {
                "figure"
            } else {
                "text"
            };
            let block = u.text(kind, n.text().collect::<Vec<_>>().join(" "), source);
            block.attributes = json!({"href":n.value().attr("href"),"src":n.value().attr("src"),"alt":n.value().attr("alt")});
        }
    }
    if u.blocks.is_empty() {
        let body = scraper::Selector::parse("body").unwrap();
        for n in doc.select(&body) {
            u.text("text", n.text().collect::<Vec<_>>().join(" "), source);
        }
    }
}
fn text_document(c: &mut ContextData, bytes: &[u8], ext: &str) -> Result<()> {
    let s = std::str::from_utf8(bytes).context("unsupported: text encoding must be UTF-8")?;
    if ext == "xml" && (s.contains("<mxfile") || s.contains("<mxGraphModel")) {
        return drawio(c, s);
    }
    let mut u = Unit::new(0, "section", "Document");
    match ext {
        "html" | "htm" => html_blocks(&mut u, s, "HTML"),
        "json" => {
            let v: Value = serde_json::from_str(s).context("corrupt JSON")?;
            let pretty = serde_json::to_string_pretty(&v)?;
            u.text("text", pretty, "JSON").attributes = v;
        }
        "xml" => {
            let d = diagrams::xml(s)?;
            u.text("text", node_text(d.root_element()), "XML")
                .attributes = json!({"source_xml":s});
        }
        "md" | "markdown" => {
            crate::markup::markdown(&mut u, s);
            let id = c.asset(bytes, "source.md", &mut u)?;
            u.text("unknown", "Original Markdown", "source.md").asset_id = Some(id);
        }
        _ => {
            u.text("text", s, "UTF-8");
        }
    }
    c.checkpoint(u)
}
fn svg(c: &mut ContextData, s: &str, bytes: &[u8]) -> Result<()> {
    let doc = diagrams::xml(s)?;
    let mut u = Unit::new(0, "figure", "SVG");
    let id = c.asset(bytes, "source.svg", &mut u)?;
    u.text("figure", "", "SVG").asset_id = Some(id);
    if let Some(content) = doc.root_element().attribute("content") {
        for (_, g) in diagrams::drawio(content, c.limits.archive_bytes)? {
            u.text("diagram", "", "SVG content mxfile").diagram = Some(g);
        }
    }
    for n in doc.descendants().filter(|n| {
        n.has_tag_name("text")
            || n.has_tag_name("path")
            || n.has_tag_name("rect")
            || n.has_tag_name("line")
            || n.has_tag_name("polygon")
    }) {
        let b = u.text(
            if n.has_tag_name("text") {
                "text"
            } else {
                "unknown"
            },
            node_text(n),
            "SVG",
        );
        b.attributes = json!({"tag":n.tag_name().name(),"attributes":n.attributes().map(|a|(a.name(),a.value())).collect::<BTreeMap<_,_>>(),"ancestor_transforms":n.ancestors().filter_map(|p|p.attribute("transform")).collect::<Vec<_>>()});
    }
    u.warnings.push(
        "SVG geometry retained; arbitrary paths do not establish verified graph relationships"
            .into(),
    );
    c.checkpoint(u)
}
fn email(c: &mut ContextData, bytes: &[u8]) -> Result<()> {
    use mailparse::MailHeaderMap;
    let mail = mailparse::parse_mail(bytes).context("corrupt email")?;
    c.document.metadata = json!({"subject":mail.headers.get_first_value("Subject"),"from":mail.headers.get_first_value("From"),"to":mail.headers.get_first_value("To"),"date":mail.headers.get_first_value("Date")});
    let mut u = Unit::new(0, "message", "Email");
    email_parts(c, &mail, &mut u, 0)?;
    c.checkpoint(u)
}
fn email_parts(
    c: &mut ContextData,
    mail: &mailparse::ParsedMail<'_>,
    u: &mut Unit,
    depth: usize,
) -> Result<()> {
    if depth > 16 {
        bail!("limit_exceeded: email nesting");
    }
    if mail.subparts.is_empty() {
        if mail.ctype.mimetype == "text/plain" {
            u.text("text", mail.get_body()?, "email MIME part");
        } else if mail.ctype.mimetype == "text/html" {
            html_blocks(u, &mail.get_body()?, "email HTML part");
        } else {
            let b = mail.get_body_raw()?;
            let disposition = mail.get_content_disposition();
            let filename = disposition
                .params
                .get("filename")
                .map(String::as_str)
                .unwrap_or("attachment.bin");
            let id = c.asset(&b, filename, u)?;
            if let Some(asset) = c.document.assets.iter_mut().find(|a| a.id == id) {
                asset.mime = mail.ctype.mimetype.clone();
            }
            let block = u.text("figure", "", "email attachment");
            block.asset_id = Some(id);
            block.attributes = json!({"filename":filename,"mime":mail.ctype.mimetype});
        }
    } else {
        for part in &mail.subparts {
            email_parts(c, part, u, depth + 1)?;
        }
    }
    Ok(())
}
fn rtf(c: &mut ContextData, bytes: &[u8]) -> Result<()> {
    let s = std::str::from_utf8(bytes).context("unsupported RTF encoding")?;
    if !s.starts_with("{\\rtf") {
        bail!("corrupt RTF");
    }
    let mut u = Unit::new(0, "section", "RTF logical document");
    #[cfg(target_os = "macos")]
    {
        let path = c.dir.join("source.rtf");
        fs::write(&path, bytes)?;
        let output = providers::command(
            "/usr/bin/textutil",
            &["-convert", "txt", "-stdout", path.to_str().unwrap()],
        )?;
        u.text("text", String::from_utf8_lossy(&output), "macOS textutil");
    }
    #[cfg(not(target_os = "macos"))]
    {
        let re = regex::Regex::new(r"\\[a-zA-Z]+-?\d* ?|[{}]")?;
        u.text("text", re.replace_all(s, ""), "RTF subset");
    }
    let id = c.asset(bytes, "source.rtf", &mut u)?;
    u.text("unknown", "Original RTF", "source.rtf").asset_id = Some(id);
    u.warnings.push("RTF text extraction only; original formatting, tables and objects preserved in source asset".into());
    c.checkpoint(u)
}
/// One 100-page batch: Poppler text + image extraction, per-page parsing and checkpointing.
/// Runs inside a parallel worker context or the coordinator, so it only touches unit files,
/// asset files and sidecars.
#[allow(clippy::too_many_arguments)]
/// Native text shorter than this on an image-free page is treated as a partial text layer: with
/// OCR enabled the page is rendered and OCR'd too. Duplicate words are suppressed by
/// [`providers::attach_ocr`], so a page whose OCR matches its text layer gains no blocks.
const OCR_RENDER_MAX_NATIVE_CHARS: usize = 24;
/// Raster density for opt-in page images. Matches the OCR render path so a page that was
/// OCR'd and a page kept for a downstream model share one rendering.
const PAGE_IMAGE_DPI: u32 = 150;

/// Formats whose units can produce a page raster: PDFs by rendering, standalone images by
/// linking their original bytes. Other formats ignore `page_images`.
fn supports_page_images(ext: &str) -> bool {
    matches!(
        ext,
        "pdf" | "png" | "jpg" | "jpeg" | "tif" | "tiff" | "bmp" | "gif" | "webp"
    )
}

/// Why a raster should be kept for downstream vision, if this unit's text is not trustworthy.
/// Standalone images pass `native_text_chars = 0`.
fn page_image_reason(u: &Unit, native_text_chars: usize) -> Option<&'static str> {
    if !u.errors.is_empty() {
        return Some("extraction_error");
    }
    if native_text_chars == 0 {
        return Some("no_text");
    }
    if native_text_chars < OCR_RENDER_MAX_NATIVE_CHARS {
        return Some("sparse_text");
    }
    if u.warnings
        .iter()
        .any(|w| w.starts_with("OCR returned no text"))
    {
        return Some("ocr_empty");
    }
    None
}

/// Resolve the `-bbox-layout` XML for one page of a batch. Poppler emits one `<page>` per
/// requested page; if that invariant ever breaks, resolve the page individually instead of
/// silently attributing another page's text.
fn page_slice_for(
    path: &str,
    page: &str,
    slices: &[String],
    offset: usize,
    fallback: bool,
    u: &mut Unit,
) -> Option<String> {
    if !fallback {
        return slices.get(offset).cloned();
    }
    u.warnings.push(
        "Poppler returned a page-count mismatch for this batch; per-page text fallback used".into(),
    );
    match providers::command(
        "pdftotext",
        &["-f", page, "-l", page, "-bbox-layout", path, "-"],
    ) {
        Ok(bytes) => {
            let raw = String::from_utf8_lossy(&bytes);
            page_slices_of(&raw).into_iter().next().map(str::to_owned)
        }
        Err(e) => {
            u.errors.push(failure(&e, Some(u.id.clone())));
            None
        }
    }
}

/// Render one page at 150 dpi. Errors are returned to the caller so a render failure marks the
/// page partial (OCR) or warns (page-image capture) instead of aborting the whole document.
fn render_page(
    c: &ContextData,
    path: &str,
    page: &str,
    dimensions: &regex::Regex,
    index: usize,
    dpi: u32,
) -> Result<Vec<u8>> {
    let page_info = providers::command("pdfinfo", &["-f", page, "-l", page, path])?;
    let page_info = String::from_utf8_lossy(&page_info);
    let size = dimensions
        .captures(&page_info)
        .context("unsupported: PDF page dimensions unavailable")?;
    let w = size[1].parse::<f64>()?;
    let h = size[2].parse::<f64>()?;
    if (w * dpi as f64 / 72.).ceil() * (h * dpi as f64 / 72.).ceil() > c.limits.page_pixels as f64 {
        bail!("limit_exceeded: PDF render pixels");
    }
    let prefix = c.dir.join(format!("render-{}-{dpi}", index + 1));
    providers::command(
        "pdftoppm",
        &[
            "-f",
            page,
            "-l",
            page,
            "-singlefile",
            "-r",
            &dpi.to_string(),
            "-png",
            path,
            prefix.to_str().unwrap(),
        ],
    )?;
    let image = prefix.with_extension("png");
    let bytes = fs::read(&image)?;
    let _ = fs::remove_file(&image);
    Ok(bytes)
}

/// Resolution used for OCR rendering. Tesseract prefers roughly 300 dpi; small table cells,
/// chart labels and stamps read much better above 150 dpi, at the cost of render and OCR time.
/// `DOC_OCR_DPI` raises it for hard scans (default: the 150 dpi page-image resolution).
fn ocr_render_dpi() -> u32 {
    std::env::var("DOC_OCR_DPI")
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .map(|dpi| dpi.clamp(96, 400))
        .unwrap_or(PAGE_IMAGE_DPI)
}

/// One batch of pages to extract: the 100-page Poppler range plus the missing page indices.
struct Batch<'a> {
    doctype: &'a regex::Regex,
    dimensions: &'a regex::Regex,
    start: usize,
    end: usize,
    missing: &'a [usize],
}

fn pdf_batch(c: &mut ContextData, input: &Path, path: &str, batch: &Batch) -> Result<()> {
    let batch_start = batch.start;
    let batch_end = batch.end;
    let missing = batch.missing;
    let doctype = batch.doctype;
    let dimensions = batch.dimensions;

    let from = (batch_start + 1).to_string();
    let to = batch_end.to_string();
    let mut text_error = None;
    let mut page_slices: Vec<String> = Vec::new();
    let mut image_dir: Option<PathBuf> = None;
    let mut page_images: std::collections::HashMap<usize, Vec<PathBuf>> =
        std::collections::HashMap::new();
    let text_started = Instant::now();
    let text_result = providers::command(
        "pdftotext",
        &["-f", &from, "-l", &to, "-bbox-layout", path, "-"],
    );
    time_stage(&STAGE_TEXT_MS, text_started);
    match text_result {
        Err(e) => text_error = Some(e),
        Ok(b) => {
            let raw = String::from_utf8_lossy(&b); // Poppler emits a fixed XHTML doctype; remove it before safe parsing.
            let cleaned = doctype.replace_all(&raw, "").into_owned();
            page_slices = page_slices_of(&cleaned)
                .into_iter()
                .map(str::to_owned)
                .collect();
        }
    }
    // Poppler emits exactly one <page> element per requested page; fall back to per-page text
    // extraction if that ever stops holding, rather than shifting text onto the wrong page.
    let per_page_fallback = text_error.is_none() && page_slices.len() != batch_end - batch_start;
    // One image extraction call per batch; `pdfimages -list` maps files back to pages.
    // When OCR is on and the batch has no native words at all, it is a scanned document: render
    // each page once and OCR that raster instead of decoding huge embedded images. `pdfimages
    // -list` is still queried (fast) so the warning can mention skipped embedded images.
    let dir = c.dir.join(format!("batch-{}-images", batch_start + 1));
    fs::create_dir_all(&dir)?;
    let images_started = Instant::now();
    let list = providers::command("pdfimages", &["-list", "-f", &from, "-l", &to, path]);
    let scanned_batch = c.options.ocr && !page_slices.iter().any(|slice| slice.contains("<word"));
    let embedded_images_present = list
        .as_ref()
        .map(|bytes| String::from_utf8_lossy(bytes).lines().count() > 2)
        .unwrap_or(false);
    let extracted = if scanned_batch {
        None
    } else {
        Some(providers::command(
            "pdfimages",
            &[
                "-png",
                "-f",
                &from,
                "-l",
                &to,
                path,
                dir.join("image").to_str().unwrap(),
            ],
        ))
    };
    time_stage(&STAGE_IMAGES_MS, images_started);
    if let Some(Ok(_)) = extracted {
        if let Ok(list) = list {
            match batch_image_map(&list, &dir) {
                Ok(map) => {
                    page_images = map;
                    image_dir = Some(dir);
                }
                Err(_) => {
                    let _ = fs::remove_dir_all(&dir);
                }
            }
        } else {
            let _ = fs::remove_dir_all(&dir);
        }
    } else {
        let _ = fs::remove_dir_all(&dir);
    }
    let batch_images_active = image_dir.is_some();
    for &i in missing {
        // Page-scoped sequence numbers keep merged asset order deterministic across workers.
        c.next_asset_seq = ((i as u64) + 1) << 32;
        let mut u = Unit::new(i, "page", format!("Page {}", i + 1));
        if i == 0 {
            let original = fs::read(input)?;
            let id = c.asset(&original, "original.pdf", &mut u)?;
            u.text("figure", "", "original.pdf").asset_id = Some(id);
            if let Err(e) = crate::pdf_source::embedded(c, input, &mut u) {
                u.errors.push(failure(&e, Some(u.id.clone())));
            }
        }
        let page = (i + 1).to_string();
        if let Some(e) = &text_error {
            u.errors.push(failure(e, Some(u.id.clone())));
        } else if let Some(slice) = page_slice_for(
            path,
            &page,
            &page_slices,
            i - batch_start,
            per_page_fallback,
            &mut u,
        )
        .as_deref()
        {
            let parse_started = Instant::now();
            let doc = diagrams::xml(slice)?;
            let words = doc
                .descendants()
                .filter(|n| n.has_tag_name("word"))
                .map(|n| {
                    let num = |a| {
                        n.attribute(a)
                            .and_then(|v| v.parse::<f64>().ok())
                            .unwrap_or(0.)
                    };
                    crate::layout::Word {
                        text: node_text(n),
                        x: num("xMin"),
                        y: num("yMin"),
                        width: num("xMax") - num("xMin"),
                        height: num("yMax") - num("yMin"),
                    }
                })
                .collect();
            for n in doc.descendants().filter(|n| n.has_tag_name("line")) {
                let words = n
                    .children()
                    .filter(|n| n.has_tag_name("word"))
                    .map(node_text)
                    .collect::<Vec<_>>();
                let b = u.text("text", words.join(" "), format!("PDF page {}", i + 1));
                let num = |a| {
                    n.attribute(a)
                        .and_then(|v| v.parse::<f64>().ok())
                        .unwrap_or(0.)
                };
                b.provenance.bounds = Some(Bounds {
                    x: num("xMin"),
                    y: num("yMin"),
                    width: num("xMax") - num("xMin"),
                    height: num("yMax") - num("yMin"),
                    coordinate_space: "pdf_points_top_left".into(),
                });
            }
            crate::layout::pdf_tables(&mut u, words);
            time_stage(&STAGE_PARSE_MS, parse_started);
        } else {
            u.warnings
                .push("PDF batch text extraction omitted this page; text may be incomplete".into());
        }
        let native_text_chars: usize = u
            .blocks
            .iter()
            .filter(|b| b.kind == "text")
            .map(|b| b.text.chars().count())
            .sum();
        // A page with no native text is a scan: render the whole page once and OCR that raster
        // instead of decoding a potentially huge embedded image. This keeps OCR page-scoped,
        // avoids duplicate work, and bypasses embedded images above the page pixel limit.
        let page_will_render_for_ocr = c.options.ocr && native_text_chars == 0;
        if batch_images_active {
            if let Some(images) = page_images.get(&(i + 1)) {
                for image in images {
                    let bytes = fs::read(image)?;
                    let name = image.file_name().unwrap().to_str().unwrap();
                    let result = if page_will_render_for_ocr {
                        c.figure_asset(&bytes, name, &mut u)
                    } else {
                        c.figure(&bytes, name, &mut u)
                    };
                    if let Err(e) = result {
                        u.errors.push(failure(&e, Some(u.id.clone())));
                    }
                }
            }
        } else {
            // Batch image extraction unavailable for this range; fall back to per-page Poppler.
            // Scanned batches deliberately skip embedded images and OCR the rendered page.
            if scanned_batch {
                if embedded_images_present {
                    u.warnings.push(
                        "Scanned page: embedded image extraction skipped; the rendered page is used for OCR".into(),
                    );
                }
            } else {
                let page_dir = c.dir.join(format!("page-{}-images", i + 1));
                fs::create_dir_all(&page_dir)?;
                let prefix = page_dir.join("image");
                match providers::command(
                    "pdfimages",
                    &[
                        "-f",
                        &page,
                        "-l",
                        &page,
                        "-png",
                        path,
                        prefix.to_str().unwrap(),
                    ],
                ) {
                    Err(e) => u.errors.push(failure(&e, Some(u.id.clone()))),
                    Ok(_) => {
                        let mut paths = fs::read_dir(&page_dir)?
                            .map(|p| p.map(|p| p.path()))
                            .collect::<std::io::Result<Vec<_>>>()?;
                        paths.sort();
                        for image in paths {
                            let bytes = fs::read(&image)?;
                            let name = image.file_name().unwrap().to_str().unwrap();
                            let result = if page_will_render_for_ocr {
                                c.figure_asset(&bytes, name, &mut u)
                            } else {
                                c.figure(&bytes, name, &mut u)
                            };
                            if let Err(e) = result {
                                u.errors.push(failure(&e, Some(u.id.clone())));
                            }
                        }
                    }
                }
                fs::remove_dir_all(&page_dir)?;
            }
        }
        let has_figures = u.blocks.iter().any(|b| b.kind == "figure");
        if !c.options.ocr && native_text_chars == 0 {
            u.status = "needs_ocr".into();
        }
        // Render when the page has no native text at all, or when it carries almost none and
        // no embedded images were extracted (a partial text layer over a scan). The same raster
        // is kept for downstream vision when `page_images` asks for it. OCR output that
        // duplicates the native text layer is suppressed downstream.
        let needs_ocr_render = c.options.ocr
            && (native_text_chars == 0
                || (!has_figures && native_text_chars < OCR_RENDER_MAX_NATIVE_CHARS));
        let image_before_ocr = page_image_reason(&u, native_text_chars);
        let wants_page_image = c.options.page_images == "all"
            || (c.options.page_images == "failed" && image_before_ocr.is_some());
        let ocr_dpi = ocr_render_dpi();
        let mut ocr_bytes: Option<Vec<u8>> = None;
        if needs_ocr_render {
            match render_page(c, path, &page, dimensions, i, ocr_dpi) {
                Ok(bytes) => {
                    if let Err(e) = c.figure(&bytes, "page-render.png", &mut u) {
                        u.errors.push(failure(&e, Some(u.id.clone())));
                    }
                    ocr_bytes = Some(bytes);
                }
                Err(e) => u.errors.push(failure(&e, Some(u.id.clone()))),
            }
        }
        if wants_page_image {
            let reason = if c.options.page_images == "all" {
                Some("requested")
            } else if c.options.page_images == "failed" {
                // OCR can add errors/warnings (empty text, unavailable language), so
                // re-evaluate after the OCR pass.
                page_image_reason(&u, native_text_chars).or(image_before_ocr)
            } else {
                None
            };
            if let Some(reason) = reason {
                // The downloadable page image stays at the documented 150 dpi; the OCR raster may
                // be rendered at a higher resolution.
                let rendered = match ocr_bytes.clone() {
                    Some(bytes) if ocr_dpi == PAGE_IMAGE_DPI => Some(bytes),
                    _ => render_page(c, path, &page, dimensions, i, PAGE_IMAGE_DPI).ok(),
                };
                match rendered {
                    Some(bytes) => {
                        if let Err(e) =
                            c.page_image(&bytes, "png", i + 1, reason, Some(PAGE_IMAGE_DPI), &mut u)
                        {
                            u.warnings.push(format!("page image unavailable: {e:#}"));
                        }
                    }
                    None => u.warnings.push("page image render failed".to_owned()),
                }
            }
        }
        if u.blocks.iter().any(|b| b.kind == "figure") {
            u.warnings.push("Embedded images retained; OCR coordinates refer to each asset, with PDF placement unresolved. Word-level native/OCR duplicates suppressed conservatively".into());
        }
        if c.options.interpret_diagrams {
            crate::layout::figure_graph(&mut u);
            u.warnings.push("Flattened PDF diagrams expose geometry-based label candidates; connector interpretation remains unknown".into());
        }
        u.warnings.push("Poppler reading order and text geometry retained; PDF table grids are conservative geometric proposals".into());
        c.checkpoint(u)?;
    }
    if let Some(dir) = &image_dir {
        let _ = fs::remove_dir_all(dir);
    }
    Ok(())
}
fn pdf(c: &mut ContextData, input: &Path) -> Result<()> {
    let path = input.to_str().context("invalid PDF path")?;
    let info = providers::command("pdfinfo", &[path]).map_err(|e| {
        if format!("{e:#}").contains("password") {
            anyhow::anyhow!("password_required: PDF is encrypted")
        } else {
            e
        }
    })?;
    let info = String::from_utf8_lossy(&info);
    c.document.metadata = json!({"pdfinfo":info});
    let pages = info
        .lines()
        .find(|l| l.starts_with("Pages:"))
        .and_then(|l| l.split_whitespace().last())
        .and_then(|s| s.parse::<usize>().ok())
        .context("corrupt PDF: page count")?;
    if pages > c.limits.units {
        bail!("limit_exceeded: PDF pages");
    }
    // Optional 1-based page selection: only selected pages become units; checkpoints for
    // other pages are ignored, and out-of-range pages are dropped.
    let selected: Vec<usize> = match c.options.pages.as_deref() {
        Some(spec) => {
            let requested = crate::model::parse_page_range(spec)
                .map_err(|e| anyhow::anyhow!("invalid_options: bad page range '{spec}': {e}"))?;
            let selected = requested
                .into_iter()
                .filter(|page| *page <= pages)
                .map(|page| page - 1)
                .collect::<Vec<_>>();
            if selected.is_empty() {
                bail!(
                    "invalid_options: page range '{spec}' selects no pages (document has {pages})"
                );
            }
            selected
        }
        None => (0..pages).collect(),
    };
    c.document.metadata["pages_total"] = json!(pages);
    c.document.metadata["page_range"] = json!(c.options.pages);
    let doctype = regex::Regex::new(r"(?s)<!DOCTYPE[^>]*>")?;
    let dimensions =
        regex::Regex::new(r"(?:Page\s+\d+\s+size:|Page size:)\s*([\d.]+)\s+x\s+([\d.]+)")?;
    const PDF_BATCH: usize = 100;
    // PDF runs get a progress board so checkpoints, assets and progress stay current while
    // workers are still processing; the board owns those writes instead of per-unit cadence.
    if c.progress.is_none() {
        c.progress = Some(std::sync::Arc::new(ProgressBoard::new(pages)));
    }
    c.write_checkpoint = false;
    // Completed pages are reused by identity; a fully resumed document never invokes Poppler.
    let mut slots: Vec<Option<Unit>> = (0..pages).map(|_| None).collect();
    for &p in &selected {
        slots[p] = fs::read(c.dir.join("units").join(format!("u{}.json", p + 1)))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Unit>(&bytes).ok())
            .filter(|unit| c.identity_ok && unit.kind == "page" && unit.index == p);
    }
    // Seed reused units into the in-memory document so checkpoints and the final result list
    // every page in order, not just newly processed ones.
    if let Some(board) = &c.progress {
        for unit in slots.iter().flatten() {
            board.publish(unit);
        }
    }
    let missing: Vec<usize> = selected
        .iter()
        .copied()
        .filter(|p| slots[*p].is_none())
        .collect();
    let mut worker_error = None;
    if !missing.is_empty() {
        let mut batches: Vec<(usize, usize, Vec<usize>)> = Vec::new();
        // Group consecutive pages so Poppler slice indexes map 1:1; resumed and ranged
        // selections skip pages that are already complete or out of range.
        for &p in &missing {
            match batches.last_mut() {
                Some((_, end, list)) if p == *end && *end - list[0] < PDF_BATCH => {
                    list.push(p);
                    *end = p + 1;
                }
                _ => batches.push((p, p + 1, vec![p])),
            }
        }
        let workers = c.limits.pdf_workers.clamp(1, 8).min(batches.len());
        // Live checkpoint/progress writer: workers publish stubs; this thread snapshots them.
        let (board, writer) = match c.progress.clone() {
            Some(board) => {
                let envelope = empty_document(&c.document);
                let dir = c.dir.clone();
                let limits = c.limits.clone();
                let watch = board.clone();
                let handle = std::thread::spawn(move || {
                    let mut last_units = 0usize;
                    let mut last_assets = 0usize;
                    let mut last_write = Instant::now();
                    loop {
                        let done = watch.done.load(std::sync::atomic::Ordering::Relaxed);
                        let units = watch.completed.load(std::sync::atomic::Ordering::Relaxed);
                        let assets = watch
                            .asset_generation
                            .load(std::sync::atomic::Ordering::Relaxed);
                        let progressed = units.saturating_sub(last_units) >= CHECKPOINT_EVERY
                            || last_write.elapsed() >= std::time::Duration::from_secs(2);
                        if (progressed || done)
                            && (units != last_units || assets != last_assets || done)
                        {
                            if write_progress_snapshot(&dir, &envelope, &watch, &limits).is_ok() {
                                last_units = units;
                                last_assets = assets;
                            }
                            last_write = Instant::now();
                        }
                        if done {
                            return;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(250));
                    }
                });
                (Some(board), Some(handle))
            }
            None => (None, None),
        };
        let _ = &board;
        let batches = std::sync::Arc::new(batches);
        let outcome: Result<()> = if workers <= 1 {
            let mut result = Ok(());
            for batch in batches.iter() {
                let batch = Batch {
                    doctype: &doctype,
                    dimensions: &dimensions,
                    start: batch.0,
                    end: batch.1,
                    missing: &batch.2,
                };
                if let Err(e) = pdf_batch(c, input, path, &batch) {
                    result = Err(e);
                    break;
                }
            }
            result
        } else {
            // Dynamic scheduling: workers pull the next batch index instead of fixed slices, so
            // a slow (image/OCR-heavy) batch does not idle the other workers.
            let next = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let results = std::thread::scope(|scope| {
                let mut handles = Vec::new();
                for _ in 0..workers {
                    let doctype = &doctype;
                    let dimensions = &dimensions;
                    let batches = batches.clone();
                    let next = next.clone();
                    let mut worker = parallel_context(c);
                    handles.push(scope.spawn(move || -> Result<()> {
                        loop {
                            let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            let Some(batch) = batches.get(index) else {
                                return Ok(());
                            };
                            let batch = Batch {
                                doctype,
                                dimensions,
                                start: batch.0,
                                end: batch.1,
                                missing: &batch.2,
                            };
                            pdf_batch(&mut worker, input, path, &batch)?;
                        }
                    }));
                }
                handles
                    .into_iter()
                    .map(|handle| {
                        handle
                            .join()
                            .map_err(|_| anyhow::anyhow!("PDF batch worker panicked"))?
                    })
                    .collect::<Vec<Result<()>>>()
            });
            let mut outcome = Ok(());
            for result in results {
                if let Err(e) = result {
                    outcome = Err(e);
                    break;
                }
            }
            outcome
        };
        if let Some(board) = &board {
            board.done.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if let Some(writer) = writer {
            let _ = writer.join();
        }
        // Merge what completed before propagating any failure, so successful units survive.
        worker_error = outcome.err();
        let started = Instant::now();
        let mut missing_pages = Vec::new();
        for &p in &selected {
            if slots[p].is_none() {
                match fs::read(c.dir.join("units").join(format!("u{}.json", p + 1)))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Unit>(&bytes).ok())
                {
                    Some(unit) => slots[p] = Some(unit),
                    None => missing_pages.push(p + 1),
                }
            }
        }
        if !missing_pages.is_empty() {
            c.document.warnings.push(format!(
                "incomplete pages retained no unit after failure: {missing_pages:?}"
            ));
        }
        // Worker contexts only wrote files; rebuild the asset manifest from sidecars.
        let (assets, next_seq) = ContextData::load_assets(&c.dir)?;
        c.document.assets = assets;
        c.next_asset_seq = next_seq;
        c.retained = c.document.assets.iter().map(|a| a.bytes).sum();
        time_stage(&STAGE_MERGE_MS, started);
    }
    c.document.units = slots.into_iter().flatten().collect();
    match worker_error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}
/// Map `pdfimages` output files to 1-based page numbers using the matching `-list` table.
/// Files are emitted in list order; a count mismatch fails so callers fall back per page.
fn batch_image_map(
    list: &[u8],
    dir: &Path,
) -> Result<std::collections::HashMap<usize, Vec<PathBuf>>> {
    let pages: Vec<usize> = String::from_utf8_lossy(list)
        .lines()
        .filter_map(|line| line.split_whitespace().next()?.parse::<usize>().ok())
        .collect();
    let mut files = fs::read_dir(dir)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    files.sort();
    if pages.len() != files.len() {
        bail!(
            "image list mismatch: {} listed, {} extracted",
            pages.len(),
            files.len()
        );
    }
    let mut map: std::collections::HashMap<usize, Vec<PathBuf>> = std::collections::HashMap::new();
    for (page, file) in pages.into_iter().zip(files) {
        map.entry(page).or_default().push(file);
    }
    Ok(map)
}
/// Split Poppler `-bbox-layout` output into per-page XML slices without a second parse of the
/// whole range. Poppler always emits one `<page ...>...</page>` element per requested page.
fn page_slices_of(xml: &str) -> Vec<&str> {
    let mut slices = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<page") {
        let Some(end) = rest[start..].find("</page>") else {
            break;
        };
        let stop = start + end + "</page>".len();
        slices.push(&rest[start..stop]);
        rest = &rest[stop..];
    }
    slices
}

fn cell_address(row: u32, column: u32) -> String {
    let mut column = column + 1;
    let mut letters = vec![];
    while column > 0 {
        column -= 1;
        letters.push((b'A' + (column % 26) as u8) as char);
        column /= 26;
    }
    letters.reverse();
    format!("{}{}", letters.into_iter().collect::<String>(), row + 1)
}
