//! Format adapters keep upstream engines behind the canonical document boundary.
use crate::{extract::ContextData, model::*, providers};
use anyhow::{bail, Context, Result};
use base64::Engine;
use serde_json::{json, Value};
use std::path::Path;

pub trait FormatAdapter {
    fn name(&self) -> &str;
    fn extract(&self, context: &mut ContextData, input: &Path, extension: &str) -> Result<()>;
}
/// Remove ANSI SGR escape sequences (`ESC [ ... m`) from provider output.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('\u{1b}') {
        out.push_str(&rest[..i]);
        rest = &rest[i + 1..];
        if let Some(stripped) = rest.strip_prefix('[') {
            rest = stripped
                .split_once(|c: char| c.is_ascii_alphabetic())
                .map(|(_, tail)| tail)
                .unwrap_or("");
        }
    }
    out.push_str(rest);
    out
}
pub struct Xberg;
impl FormatAdapter for Xberg {
    fn name(&self) -> &str {
        "xberg/1.3.8"
    }
    fn extract(&self, c: &mut ContextData, input: &Path, extension: &str) -> Result<()> {
        let executable = std::env::var("DOC_XBERG")
            .context("dependency_unavailable: provision Xberg 1.3.8 and set DOC_XBERG")?;
        let version = providers::command(&executable, &["version"])?;
        // Accept decoration around the version token (e.g. "xberg-cli 1.3.8" or ANSI color,
        // which older builds may emit despite NO_COLOR).
        let plain = strip_ansi(String::from_utf8_lossy(&version).trim());
        let observed = plain
            .split_whitespace()
            .last()
            .unwrap_or("")
            .trim_start_matches('v');
        if observed != "1.3.8" {
            bail!("unsupported: Xberg adapter requires evaluated version 1.3.8 (found {plain:?})");
        }
        if !extension.chars().all(|c| c.is_ascii_alphanumeric()) {
            bail!("unsupported extension");
        }
        // No URL mode, discovery, caches, layout models, OCR or cloud providers.
        let local = c.dir.join(format!("adapter-input.{extension}"));
        std::fs::copy(input, &local)?;
        let output = c.dir.join("xberg-output.json");
        if output.exists() {
            std::fs::remove_file(&output)?;
        }
        let bytes = providers::command(
            &executable,
            &[
                "extract",
                local.to_str().context("invalid path")?,
                "--no-config-discovery",
                "--ocr",
                "false",
                "--no-cache",
                "true",
                "--layout",
                "false",
                "--extract-pages",
                "true",
                "--extract-images",
                "true",
                "--include-structure",
                "true",
                "--format",
                "json",
                "--max-threads",
                "1",
            ],
        )?;
        if bytes.len() as u64 > c.limits.output_bytes {
            bail!("limit_exceeded: Xberg output");
        }
        std::fs::write(&output, &bytes)?;
        let wrapper: Value = serde_json::from_slice(&bytes)?;
        let result = &wrapper["result"];
        c.document.metadata = result["metadata"].clone();
        c.document.versions["adapter"] = json!(self.name());
        let kind = match extension {
            "pdf" => "page",
            "pptx" | "ppt" | "odp" => "slide",
            "xlsx" | "xls" | "xlsm" | "xlsb" | "ods" => "sheet",
            _ => "section",
        };
        let mut pages = result["pages"].as_array().cloned().unwrap_or_default();
        if pages.is_empty() {
            pages.push(json!({"content":result["content"]}));
        }
        for (i, page) in pages.into_iter().enumerate() {
            let mut u = Unit::new(
                i,
                kind,
                page["sheet_name"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("{} {}", kind, i + 1)),
            );
            u.text(
                "text",
                page["content"].as_str().unwrap_or(""),
                "Xberg native",
            )
            .attributes = page.clone();
            for table in page["tables"].as_array().into_iter().flatten() {
                let rows = table["cells"].as_array().cloned().unwrap_or_default();
                let mut t = Table {
                    cells: vec![],
                    rows: rows.len(),
                    columns: 0,
                    headers: vec![],
                    compact: false,
                };
                for (r, row) in rows.iter().enumerate() {
                    if let Some(cols) = row.as_array() {
                        t.columns = t.columns.max(cols.len());
                        for (col, value) in cols.iter().enumerate() {
                            let raw = value
                                .as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| value.to_string());
                            t.cells.push(Cell {
                                row: r as u32,
                                column: col as u32,
                                raw,
                                value: value.clone(),
                                formula: None,
                                cached_value: None,
                                row_span: 1,
                                column_span: 1,
                            });
                        }
                    }
                }
                u.text("table", "", "Xberg table source").table = Some(t);
            }
            for image in result["images"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|image| {
                    image["page_number"]
                        .as_u64()
                        .map(|p| p == i as u64 + 1)
                        .unwrap_or(i == 0)
                })
            {
                let bytes = if let Some(data) = image["data"].as_array() {
                    data.iter()
                        .map(|v| {
                            v.as_u64()
                                .filter(|v| *v <= 255)
                                .map(|v| v as u8)
                                .context("corrupt Xberg image byte")
                        })
                        .collect::<Result<Vec<_>>>()?
                } else if let Some(data) = image["data"].as_str() {
                    base64::engine::general_purpose::STANDARD.decode(data)?
                } else {
                    u.warnings
                        .push("Xberg image bytes unavailable; full source retained".into());
                    continue;
                };
                let name = format!(
                    "xberg-image-{}.{}",
                    image["image_index"].as_u64().unwrap_or(0),
                    image["format"].as_str().unwrap_or("bin")
                );
                c.figure(&bytes, &name, &mut u)?;
            }
            u.warnings.push("Xberg canonical mapping is partial. Full source JSON retained; Word units are logical, and cached formulas/merge geometry require the direct adapter".into());
            if i == 0 {
                let id = c.asset(&bytes, "xberg-source.json", &mut u)?;
                u.text("unknown", "Full upstream result", "xberg-source.json")
                    .asset_id = Some(id);
                let original = std::fs::read(input)?;
                let id = c.asset(&original, &format!("original.{extension}"), &mut u)?;
                u.text("figure", "Original document", "source").asset_id = Some(id);
            }
            c.checkpoint(u)?;
        }
        if c.options.ocr || c.options.vision {
            c.document.warnings.push("Xberg itself runs native extraction only; extracted image assets are separately routed to optional OCR/vision, without a scan-render fallback".into());
        }
        Ok(())
    }
}
