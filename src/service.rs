use crate::{
    extract::{self, atomic_json, ENGINE_VERSION},
    model::*,
    providers,
};
use anyhow::{Context, Result};
use axum::{
    extract::{DefaultBodyLimit, Multipart, Path as ApiPath, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        Html, IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use base64::Engine as _;
use hmac::{Hmac, Mac};
use regex::Regex;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::process::Command;
use tokio_stream::wrappers::ReceiverStream;
use utoipa_swagger_ui::SwaggerUi;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub data_dir: PathBuf,
    pub bind: String,
    pub limits: Limits,
    pub tokens: HashMap<String, String>,
    /// HMAC-SHA256 secret for `X-DocSmith-Signature` on webhook deliveries.
    #[serde(default)]
    pub webhook_secret: Option<String>,
    /// Allow webhook callbacks to loopback/private addresses (default refuses them).
    #[serde(default)]
    pub webhook_allow_private: bool,
    /// Shell command run once per completed job (cwd = job dir). Placeholders:
    /// `{id}`, `{name}`, `{state}`, `{dir}`, `{result}`, `{format}`; also exported as
    /// `DOC_ID`, `DOC_FILENAME`, `DOC_STATE`, `DOC_DIR`, `DOC_RESULT`, `DOC_FORMAT`.
    /// Example: `aws s3 cp {result} s3://bucket/results/{id}.{format}`.
    #[serde(default)]
    pub sink_command: Option<String>,
    /// Directory polled for new files; a file whose size/mtime stops changing is ingested as
    /// a job (tenant `local`) and moved to `<watch_dir>/processed/`.
    #[serde(default)]
    pub watch_dir: Option<PathBuf>,
    /// Options applied to watched files (for example `{"ocr": true}`).
    #[serde(default)]
    pub watch_options: Options,
    /// Tenant that owns watched jobs (default `local`).
    #[serde(default)]
    pub watch_tenant: Option<String>,
    /// Native S3 upload of result artifacts after completion (alternative to `sink_command`;
    /// `sink_command` wins when both are configured). Credentials fall back to
    /// `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` / `AWS_SESSION_TOKEN`.
    #[serde(default)]
    pub s3: Option<crate::s3::S3Config>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from("data"),
            bind: "127.0.0.1:8787".into(),
            limits: Limits::default(),
            tokens: HashMap::new(),
            webhook_secret: None,
            webhook_allow_private: false,
            sink_command: None,
            watch_dir: None,
            watch_options: Options::default(),
            watch_tenant: None,
            s3: None,
        }
    }
}
/// Last (size, mtime) per watched file; a stable pair means the writer finished.
type WatchSeen = Arc<Mutex<HashMap<PathBuf, (u64, Option<std::time::SystemTime>)>>>;
#[derive(Clone)]
pub struct App {
    pub config: Arc<Config>,
    db: Arc<Mutex<Connection>>,
    running: Arc<Mutex<HashMap<String, u32>>>,
    provider_fingerprint: String,
    started_at: i64,
    watch_seen: WatchSeen,
}
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Job {
    pub id: String,
    pub state: String,
    pub filename: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// Units completed so far.
    pub progress: usize,
    /// Units expected in total when known; zero for older jobs or open-ended work.
    #[serde(default)]
    pub total: usize,
    /// The options this job was queued with, so the UI can show and reuse them.
    #[serde(default)]
    pub options: Options,
    /// Optional user-facing title; the original `filename` is always retained.
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub collection: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub favorite: bool,
    pub result_status: Option<String>,
    pub error: Option<Failure>,
    pub warnings: Vec<String>,
    pub unit_failures: Vec<Failure>,
}
type ApiResult<T> = std::result::Result<T, ApiError>;
pub struct ApiError(StatusCode, String, String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(json!({"error":{"code":self.1,"message":self.2}})),
        )
            .into_response()
    }
}
impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error".into(),
            e.to_string(),
        )
    }
}
impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        anyhow::Error::from(e).into()
    }
}
impl From<std::io::Error> for ApiError {
    fn from(e: std::io::Error) -> Self {
        anyhow::Error::from(e).into()
    }
}
fn error(status: StatusCode, code: &str, message: &str) -> ApiError {
    ApiError(status, code.into(), message.into())
}
fn epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
/// Plain searchable text for a block. Tables are flattened row by row so a query can match
/// any cell while still pointing the result at the owning block. The cap keeps a pathological
/// table from turning the index into a second copy of the whole result.
fn block_search_text(block: &Block) -> String {
    let mut text = if let Some(table) = &block.table {
        let mut rows: BTreeMap<usize, BTreeMap<usize, String>> = BTreeMap::new();
        for cell in &table.cells {
            if cell.raw.is_empty() {
                continue;
            }
            rows.entry(cell.row as usize)
                .or_default()
                .insert(cell.column as usize, cell.raw.clone());
        }
        if rows.is_empty() {
            block.text.clone()
        } else {
            rows.values()
                .map(|row| {
                    row.values()
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join("\t")
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
    } else {
        block.text.clone()
    };
    text = text.trim().to_string();
    if text.is_empty() {
        return text;
    }
    if text.chars().count() > 50_000 {
        text = text.chars().take(50_000).collect();
    }
    text
}

/// Window around the SQL `instr` result (1-based character offset). Whitespace is collapsed so
/// a match in a table row or a multi-line block reads as one line in the palette.
fn search_snippet(text: &str, position: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return String::new();
    }
    let at = position.saturating_sub(1).min(chars.len() - 1);
    let start = at.saturating_sub(55);
    let end = (start + 180).min(chars.len());
    let mut out: String = chars[start..end].iter().collect();
    out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if start > 0 {
        out = format!("…{out}");
    }
    if end < chars.len() {
        out.push('…');
    }
    out
}

impl App {
    pub fn open(mut config: Config) -> Result<Self> {
        if config.limits.workers == 0 || config.limits.workers > 32 {
            anyhow::bail!("workers must be 1..32");
        }
        std::fs::create_dir_all(&config.data_dir)?;
        config.data_dir = std::fs::canonicalize(&config.data_dir)?;
        let db = Connection::open(config.data_dir.join("jobs.sqlite"))?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "FULL")?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS jobs(id TEXT PRIMARY KEY, tenant TEXT NOT NULL, filename TEXT NOT NULL, cache_key TEXT NOT NULL, options TEXT NOT NULL, state TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, error TEXT); CREATE INDEX IF NOT EXISTS job_queue ON jobs(state,created_at); CREATE INDEX IF NOT EXISTS job_cache ON jobs(tenant,cache_key,state);")?;
        // Additive migrations: older databases lack these optional columns.
        let _ = db.execute("ALTER TABLE jobs ADD COLUMN callback_url TEXT", []);
        let _ = db.execute("ALTER TABLE jobs ADD COLUMN display_name TEXT", []);
        let _ = db.execute("ALTER TABLE jobs ADD COLUMN collection TEXT", []);
        let _ = db.execute("ALTER TABLE jobs ADD COLUMN tags TEXT", []);
        let _ = db.execute(
            "ALTER TABLE jobs ADD COLUMN favorite INTEGER NOT NULL DEFAULT 0",
            [],
        );
        db.execute_batch("CREATE TABLE IF NOT EXISTS webhook_deliveries(id TEXT PRIMARY KEY, job_id TEXT NOT NULL, url TEXT NOT NULL, event TEXT NOT NULL, payload TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, next_at INTEGER NOT NULL, delivered_at INTEGER, last_error TEXT); CREATE INDEX IF NOT EXISTS webhook_pending ON webhook_deliveries(delivered_at,next_at); CREATE INDEX IF NOT EXISTS webhook_job ON webhook_deliveries(job_id);")?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS sink_runs(job_id TEXT PRIMARY KEY, state TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, next_at INTEGER NOT NULL, completed_at INTEGER, last_error TEXT); CREATE INDEX IF NOT EXISTS sink_pending ON sink_runs(completed_at,next_at);")?;
        // Full-text-ish search over completed results. One row per block (plus one per unit
        // name) keeps matches addressable in the reader. `search_state` records jobs that have
        // been indexed so a query only backfills new or pre-existing jobs once.
        db.execute_batch("CREATE TABLE IF NOT EXISTS search_units(job_id TEXT NOT NULL, unit_id TEXT NOT NULL, unit_name TEXT NOT NULL, unit_index INTEGER NOT NULL, block_id TEXT, kind TEXT NOT NULL, text TEXT NOT NULL); CREATE INDEX IF NOT EXISTS search_job ON search_units(job_id); CREATE TABLE IF NOT EXISTS search_state(job_id TEXT PRIMARY KEY, indexed_at INTEGER NOT NULL, blocks INTEGER NOT NULL);")?;
        // Reap orphan process groups before redispatching their durable input.
        let mut statement =
            db.prepare("SELECT id FROM jobs WHERE state IN ('running','deleting')")?;
        let stale = statement
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        for id in stale {
            let dir = config.data_dir.join("jobs").join(&id);
            if let Ok(bytes) = std::fs::read(dir.join("worker.pid")) {
                if let Ok(pid) = serde_json::from_slice::<u32>(&bytes) {
                    let output = std::process::Command::new("ps")
                        .args(["-p", &pid.to_string(), "-o", "command="])
                        .output();
                    if output.is_ok_and(|o| {
                        String::from_utf8_lossy(&o.stdout)
                            .contains(&format!("worker --job-dir {}", dir.display()))
                    }) {
                        kill_group(pid);
                    }
                }
            }
            let deleting: bool = db.query_row(
                "SELECT state='deleting' FROM jobs WHERE id=?1",
                [&id],
                |r| r.get(0),
            )?;
            if deleting {
                if dir.exists() {
                    std::fs::remove_dir_all(&dir)?;
                }
                db.execute("DELETE FROM search_units WHERE job_id=?1", [&id])?;
                db.execute("DELETE FROM search_state WHERE job_id=?1", [&id])?;
                db.execute("DELETE FROM jobs WHERE id=?1", [&id])?;
            }
        }
        // A single server owns this database. A lock prevents competing recovery/dispatch.
        db.execute(
            "UPDATE jobs SET state='queued',updated_at=?1 WHERE state='running'",
            [epoch()],
        )?;
        Ok(Self {
            config: Arc::new(config),
            db: Arc::new(Mutex::new(db)),
            running: Arc::new(Mutex::new(HashMap::new())),
            provider_fingerprint: extract::hash(providers::identity().to_string().as_bytes()),
            started_at: epoch(),
            watch_seen: Arc::new(Mutex::new(HashMap::new())),
        })
    }
    fn tenant(&self, headers: &HeaderMap) -> ApiResult<String> {
        if self.config.tokens.is_empty() {
            return Ok("local".into());
        }
        let token = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or_else(|| {
                error(
                    StatusCode::UNAUTHORIZED,
                    "unauthorized",
                    "Bearer token required",
                )
            })?;
        self.config
            .tokens
            .get(token)
            .cloned()
            .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "unauthorized", "Invalid token"))
    }
    fn authorize(&self, id: &str, headers: &HeaderMap) -> ApiResult<()> {
        let tenant = self.tenant(headers)?;
        let db = self.db.lock().unwrap();
        let count: i64 = db.query_row(
            "SELECT count(*) FROM jobs WHERE id=?1 AND tenant=?2 ",
            params![id, tenant],
            |r| r.get(0),
        )?;
        if count == 0 {
            return Err(error(StatusCode::NOT_FOUND, "not_found", "Job not found"));
        }
        Ok(())
    }
    fn dir(&self, id: &str) -> PathBuf {
        self.config.data_dir.join("jobs").join(id)
    }
    pub fn job(&self, id: &str) -> Result<Job> {
        let db = self.db.lock().unwrap();
        let mut job = db.query_row(
            "SELECT id,state,filename,created_at,updated_at,error,options,display_name,collection,tags,favorite FROM jobs WHERE id=?1",
            [id],
            |r| {
                let e: Option<String> = r.get(5)?;
                let options: String = r.get(6)?;
                let tags: Option<String> = r.get(9)?;
                Ok(Job {
                    id: r.get(0)?,
                    state: r.get(1)?,
                    filename: r.get(2)?,
                    created_at: r.get(3)?,
                    updated_at: r.get(4)?,
                    progress: 0,
                    total: 0,
                    options: serde_json::from_str(&options).unwrap_or_default(),
                    display_name: r.get(7)?,
                    collection: r.get(8)?,
                    tags: tags
                        .and_then(|value| serde_json::from_str::<Vec<String>>(&value).ok())
                        .unwrap_or_default(),
                    favorite: r.get::<_, i64>(10)? != 0,
                    result_status: None,
                    error: e.and_then(|s| serde_json::from_str(&s).ok()),
                    warnings: vec![],
                    unit_failures: vec![],
                })
            },
        )?;
        drop(db);
        if let Ok(bytes) = std::fs::read(self.dir(id).join("progress.json")) {
            if let Ok(progress) = serde_json::from_slice::<Progress>(&bytes) {
                job.progress = progress.units;
                job.total = if progress.total > 0 {
                    progress.total
                } else if progress.result_status.is_some() {
                    // Completed progress files written before `total` existed: the unit count
                    // is the final count.
                    progress.units
                } else {
                    0
                };
                job.result_status = progress.result_status;
                job.warnings = progress.warnings;
                job.unit_failures = progress.errors;
            }
        }
        Ok(job)
    }
    /// Build the block-level search rows for one completed job. The canonical result stays
    /// untouched; this is only an index used by the viewer's command palette.
    fn index_job_for_search(&self, id: &str) -> Result<usize> {
        let path = self.dir(id).join("result.json");
        // A multi-hundred-megabyte spreadsheet result must not be parsed a second time in
        // memory just to answer search. Filename search still works for those jobs.
        const SEARCH_INDEX_MAX_BYTES: u64 = 64 * 1024 * 1024;
        if std::fs::metadata(&path)?.len() > SEARCH_INDEX_MAX_BYTES {
            tracing::info!("search: skipping oversized result for {id}");
            let db = self.db.lock().unwrap();
            db.execute(
                "INSERT OR REPLACE INTO search_state(job_id,indexed_at,blocks) VALUES(?1,?2,0)",
                params![id, epoch()],
            )?;
            return Ok(0);
        }
        let bytes = std::fs::read(&path)?;
        let mut document: Document = serde_json::from_slice(&bytes)?;
        apply_review(&mut document, &read_review(&self.dir(id)));
        let mut rows: Vec<(String, String, usize, Option<String>, String, String)> = Vec::new();
        for unit in &document.units {
            let name = unit.name.trim();
            if !name.is_empty() {
                rows.push((
                    unit.id.clone(),
                    unit.name.clone(),
                    unit.index,
                    None,
                    "unit".into(),
                    name.to_string(),
                ));
            }
            for block in &unit.blocks {
                let text = block_search_text(block);
                if text.is_empty() {
                    continue;
                }
                rows.push((
                    unit.id.clone(),
                    unit.name.clone(),
                    unit.index,
                    Some(block.id.clone()),
                    block.kind.clone(),
                    text,
                ));
            }
        }
        let db = self.db.lock().unwrap();
        let tx = db.unchecked_transaction()?;
        tx.execute("DELETE FROM search_units WHERE job_id=?1", [id])?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO search_units(job_id,unit_id,unit_name,unit_index,block_id,kind,text) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            )?;
            for (unit_id, unit_name, unit_index, block_id, kind, text) in &rows {
                insert.execute(params![
                    id,
                    unit_id,
                    unit_name,
                    *unit_index as i64,
                    block_id,
                    kind,
                    text
                ])?;
            }
        }
        tx.execute(
            "INSERT OR REPLACE INTO search_state(job_id,indexed_at,blocks) VALUES(?1,?2,?3)",
            params![id, epoch(), rows.len() as i64],
        )?;
        tx.commit()?;
        Ok(rows.len())
    }

    /// Backfill indexes for completed jobs created before search existed, or jobs whose
    /// completion index failed. Bounded so one search cannot scan an unbounded archive.
    fn index_pending_for_search(&self, tenant: &str, limit: usize) -> usize {
        let ids: Vec<String> = {
            let db = self.db.lock().unwrap();
            let Ok(mut statement) = db.prepare(
                "SELECT id FROM jobs WHERE tenant=?1 AND state='completed' AND id NOT IN (SELECT job_id FROM search_state) ORDER BY updated_at DESC LIMIT ?2",
            ) else {
                return 0;
            };
            statement
                .query_map(params![tenant, limit as i64], |r| r.get::<_, String>(0))
                .and_then(|rows| rows.collect::<std::result::Result<Vec<_>, _>>())
                .unwrap_or_default()
        };
        let mut indexed = 0;
        for id in ids {
            match self.index_job_for_search(&id) {
                Ok(_) => indexed += 1,
                Err(e) => {
                    tracing::warn!("search index failed for {id}: {e:#}");
                    // Record the attempt even on failure. A malformed/old result should not
                    // make every later query retry the same expensive parse.
                    let db = self.db.lock().unwrap();
                    let _ = db.execute(
                        "INSERT OR REPLACE INTO search_state(job_id,indexed_at,blocks) VALUES(?1,?2,0)",
                        params![id, epoch()],
                    );
                }
            }
        }
        indexed
    }

    /// Remove terminal jobs past the configured retention window. Active workers are never
    /// touched; cancel/delete still own their own lifetime handling.
    fn sweep_retention(&self) -> Result<usize> {
        let retention = self.config.limits.retention_seconds;
        if retention == 0 {
            return Ok(0);
        }
        let cutoff = epoch() - retention as i64;
        let active: std::collections::HashSet<String> =
            self.running.lock().unwrap().keys().cloned().collect();
        let db = self.db.lock().unwrap();
        let ids: Vec<String> = db
            .prepare(
                "SELECT id FROM jobs WHERE state IN ('completed','failed','cancelled') AND updated_at < ?1",
            )?
            .query_map([cutoff], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        let mut removed = 0;
        for id in ids {
            if active.contains(&id) {
                continue;
            }
            let dir = self.dir(&id);
            if dir.exists() {
                std::fs::remove_dir_all(&dir)?;
            }
            db.execute("DELETE FROM search_units WHERE job_id=?1", [&id])?;
            db.execute("DELETE FROM search_state WHERE job_id=?1", [&id])?;
            db.execute("DELETE FROM jobs WHERE id=?1", [&id])?;
            removed += 1;
        }
        Ok(removed)
    }
    /// Enqueue deliveries for newly terminal jobs with a callback URL, then attempt due
    /// deliveries. Failures retry with backoff and stay visible in `webhook_deliveries`.
    async fn webhook_cycle(&self) {
        let pending: Vec<(String, String, String, String, String, Option<String>)> = {
            let db = self.db.lock().unwrap();
            let mut statement = match db.prepare(
                "SELECT id,callback_url,state,filename,options,error FROM jobs WHERE callback_url IS NOT NULL AND state IN ('completed','failed','cancelled') AND id NOT IN (SELECT job_id FROM webhook_deliveries) LIMIT 100",
            ) {
                Ok(statement) => statement,
                Err(e) => {
                    tracing::error!("webhook scan failed: {e}");
                    return;
                }
            };
            let rows = statement
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Option<String>>(5)?,
                    ))
                })
                .and_then(|rows| rows.collect::<std::result::Result<Vec<_>, _>>());
            match rows {
                Ok(rows) => rows,
                Err(e) => {
                    tracing::error!("webhook scan failed: {e}");
                    return;
                }
            }
        };
        for (id, url, state, filename, options, error) in pending {
            let result_format = serde_json::from_str::<Options>(&options)
                .map(|o| o.result_format)
                .unwrap_or_else(|_| "json".into());
            let delivery = uuid::Uuid::new_v4().to_string();
            let payload = json!({
                "event": format!("job.{state}"),
                "delivery": delivery,
                "job": {
                    "id": id,
                    "state": state,
                    "filename": filename,
                    "result_format": result_format,
                    "result_url": format!("/v1/extractions/{id}/result"),
                    "archive_url": format!("/v1/extractions/{id}/archive"),
                    "error": error.map(|e| serde_json::from_str::<Value>(&e).unwrap_or(Value::String(e))),
                },
            });
            let db = self.db.lock().unwrap();
            let _ = db.execute(
                "INSERT INTO webhook_deliveries(id,job_id,url,event,payload,attempts,next_at) VALUES(?1,?2,?3,?4,?5,0,?6)",
                params![delivery, id, url, format!("job.{state}"), payload.to_string(), epoch()],
            );
        }
        let due: Vec<(String, String, String, String, String, i64)> = {
            let db = self.db.lock().unwrap();
            let mut statement = match db.prepare(
                "SELECT id,job_id,url,event,payload,attempts FROM webhook_deliveries WHERE delivered_at IS NULL AND attempts < 5 AND next_at <= ?1 ORDER BY next_at LIMIT 8",
            ) {
                Ok(statement) => statement,
                Err(_) => return,
            };
            let rows = statement
                .query_map([epoch()], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, i64>(5)?,
                    ))
                })
                .and_then(|rows| rows.collect::<std::result::Result<Vec<_>, _>>());
            match rows {
                Ok(rows) => rows,
                Err(_) => return,
            }
        };
        for (delivery, job_id, url, event, payload, attempts) in due {
            let secret = self.config.webhook_secret.clone();
            let attempt = (
                url.clone(),
                job_id.clone(),
                event.clone(),
                delivery.clone(),
                payload.clone(),
            );
            let outcome = tokio::task::spawn_blocking(move || {
                deliver_webhook(
                    &attempt.0,
                    &attempt.1,
                    &attempt.2,
                    &attempt.3,
                    &attempt.4,
                    secret.as_deref(),
                )
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
            let db = self.db.lock().unwrap();
            let updated = match outcome {
                Ok(()) => db.execute(
                    "UPDATE webhook_deliveries SET delivered_at=?2,last_error=NULL WHERE id=?1",
                    params![delivery, epoch()],
                ),
                Err(message) => {
                    let backoff = 4i64 << (attempts + 1).min(6);
                    db.execute(
                        "UPDATE webhook_deliveries SET attempts=?2,next_at=?3,last_error=?4 WHERE id=?1",
                        params![delivery, attempts + 1, epoch() + backoff, message],
                    )
                }
            };
            if let Err(e) = updated {
                tracing::error!("webhook update failed: {e}");
            }
        }
        // Deliveries whose job row is gone (DELETE or retention sweep).
        let db = self.db.lock().unwrap();
        let _ = db.execute(
            "DELETE FROM webhook_deliveries WHERE job_id NOT IN (SELECT id FROM jobs)",
            [],
        );
    }
    /// Run the configured `sink_command` once per completed job (for example an upload to
    /// object storage). Failures retry three times; the log stays in the job directory.
    async fn sink_cycle(&self) {
        let command = self.config.sink_command.clone();
        let s3 = self.config.s3.clone();
        if command.is_none() && s3.is_none() {
            return;
        }
        let pending: Vec<(String, String)> = {
            let db = self.db.lock().unwrap();
            let mut statement = match db.prepare(
                "SELECT id,state FROM jobs WHERE state='completed' AND id NOT IN (SELECT job_id FROM sink_runs) LIMIT 100",
            ) {
                Ok(statement) => statement,
                Err(e) => {
                    tracing::error!("sink scan failed: {e}");
                    return;
                }
            };
            let rows = statement
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .and_then(|rows| rows.collect::<std::result::Result<Vec<_>, _>>());
            match rows {
                Ok(rows) => rows,
                Err(_) => return,
            }
        };
        for (id, state) in pending {
            let db = self.db.lock().unwrap();
            let _ = db.execute(
                "INSERT OR IGNORE INTO sink_runs(job_id,state,attempts,next_at) VALUES(?1,?2,0,?3)",
                params![id, state, epoch()],
            );
        }
        let due: Vec<(String, String, String, i64)> = {
            let db = self.db.lock().unwrap();
            let mut statement = match db.prepare(
                "SELECT s.job_id,j.filename,s.state,s.attempts FROM sink_runs s JOIN jobs j ON j.id=s.job_id WHERE s.completed_at IS NULL AND s.attempts < 3 AND s.next_at <= ?1 ORDER BY s.next_at LIMIT 4",
            ) {
                Ok(statement) => statement,
                Err(_) => return,
            };
            let rows = statement
                .query_map([epoch()], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                    ))
                })
                .and_then(|rows| rows.collect::<std::result::Result<Vec<_>, _>>());
            match rows {
                Ok(rows) => rows,
                Err(_) => return,
            }
        };
        for (job_id, filename, state, attempts) in due {
            let dir = self.dir(&job_id);
            let command = command.clone();
            let s3 = s3.clone();
            let id = job_id.clone();
            let outcome =
                tokio::task::spawn_blocking(move || -> std::result::Result<(), String> {
                    match (&command, &s3) {
                        (Some(command), _) => run_sink(command, &id, &state, &filename, &dir),
                        (None, Some(s3)) => upload_to_s3(s3, &id, &dir)
                            .map(|_| ())
                            .map_err(|e| format!("{e:#}")),
                        (None, None) => Ok(()),
                    }
                })
                .await
                .unwrap_or_else(|e| Err(e.to_string()));
            let db = self.db.lock().unwrap();
            let updated = match outcome {
                Ok(()) => db.execute(
                    "UPDATE sink_runs SET completed_at=?2,last_error=NULL WHERE job_id=?1",
                    params![job_id, epoch()],
                ),
                Err(message) => {
                    tracing::warn!("sink command failed for {job_id}: {message}");
                    db.execute(
                        "UPDATE sink_runs SET attempts=?2,next_at=?3,last_error=?4 WHERE job_id=?1",
                        params![job_id, attempts + 1, epoch() + 10 * (attempts + 1), message],
                    )
                }
            };
            if let Err(e) = updated {
                tracing::error!("sink update failed: {e}");
            }
        }
        let db = self.db.lock().unwrap();
        let _ = db.execute(
            "DELETE FROM sink_runs WHERE job_id NOT IN (SELECT id FROM jobs)",
            [],
        );
    }
    /// Ingest files dropped into `watch_dir`: a file whose size/mtime is unchanged since the
    /// previous scan is staged and queued (tenant `local`), then moved to `processed/`.
    async fn watch_cycle(&self) {
        let Some(dir) = self.config.watch_dir.clone() else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        let mut candidates: Vec<PathBuf> = entries
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| !name.to_string_lossy().starts_with('.'))
            })
            .collect();
        candidates.sort();
        for path in candidates {
            let Ok(metadata) = std::fs::metadata(&path) else {
                continue;
            };
            let stamp = (metadata.len(), metadata.modified().ok());
            let stable = {
                let mut seen = self.watch_seen.lock().unwrap();
                seen.insert(path.clone(), stamp) == Some(stamp)
            };
            if !stable {
                continue;
            }
            match self.ingest_watched(&path).await {
                Ok(()) => {
                    self.watch_seen.lock().unwrap().remove(&path);
                }
                Err(e) => tracing::info!("watch: {} not ingested yet: {e:#}", path.display()),
            }
        }
    }
    async fn ingest_watched(&self, path: &std::path::Path) -> Result<()> {
        let Some(name) = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
        else {
            return Ok(());
        };
        let id = uuid::Uuid::new_v4().to_string();
        let job_dir = self.dir(&id);
        tokio::fs::create_dir_all(&job_dir).await?;
        let staged = async {
            tokio::fs::copy(path, job_dir.join("input")).await?;
            let digest_path = job_dir.join("input");
            let digest = tokio::task::spawn_blocking(move || -> Result<String> {
                use sha2::Digest;
                use std::io::Read;
                let mut hasher = sha2::Sha256::new();
                let mut buffer = vec![0u8; 1 << 20];
                let mut file = std::fs::File::open(&digest_path)?;
                loop {
                    let read = file.read(&mut buffer)?;
                    if read == 0 {
                        break;
                    }
                    hasher.update(&buffer[..read]);
                }
                Ok(hex::encode(hasher.finalize()))
            })
            .await??;
            let size = tokio::fs::metadata(path).await?.len();
            let options = self.config.watch_options.clone();
            let options_json = serde_json::to_string(&options)?;
            match queue_job(
                self,
                self.config.watch_tenant.as_deref().unwrap_or("local"),
                &job_dir,
                &id,
                &name,
                size,
                &digest,
                &options,
                &options_json,
                None,
            ) {
                Ok(cached) => Ok(cached),
                Err(e) => {
                    let _ = tokio::fs::remove_dir_all(&job_dir).await;
                    anyhow::bail!("{}: {}", e.0, e.2)
                }
            }
        }
        .await;
        match staged {
            Ok(cached) => {
                if let Some(cached) = cached {
                    let _ = tokio::fs::remove_dir_all(&job_dir).await;
                    tracing::info!("watch: {name} matched cached job {cached}");
                } else {
                    tracing::info!("watch: queued {name} as {id}");
                }
                let processed = path
                    .parent()
                    .unwrap_or(std::path::Path::new("."))
                    .join("processed");
                tokio::fs::create_dir_all(&processed).await?;
                tokio::fs::rename(path, unique_target(&processed, &name)).await?;
                Ok(())
            }
            Err(e) => {
                let _ = tokio::fs::remove_dir_all(&job_dir).await;
                Err(e)
            }
        }
    }
    async fn webhook_worker(self) {
        loop {
            self.webhook_cycle().await;
            self.sink_cycle().await;
            self.watch_cycle().await;
            tokio::time::sleep(Duration::from_millis(1000)).await;
        }
    }
    fn set_state(&self, id: &str, state: &str, failure: Option<Failure>) -> Result<()> {
        self.db.lock().unwrap().execute("UPDATE jobs SET state=?2,updated_at=?3,error=?4 WHERE id=?1 AND state NOT IN ('cancelled','deleting')",params![id,state,epoch(),failure.map(|e|serde_json::to_string(&e)).transpose()?])?;
        Ok(())
    }
    async fn dispatcher(self) {
        loop {
            let slots = self
                .config
                .limits
                .workers
                .saturating_sub(self.running.lock().unwrap().len());
            for _ in 0..slots {
                let id = {
                    let db = self.db.lock().unwrap();
                    let id: Option<String> = db
                        .query_row(
                            "SELECT id FROM jobs WHERE state='queued' ORDER BY created_at LIMIT 1",
                            [],
                            |r| r.get(0),
                        )
                        .ok();
                    if let Some(id) = &id {
                        let _=db.execute("UPDATE jobs SET state='running',updated_at=?2 WHERE id=?1 AND state='queued'",params![id,epoch()]);
                    }
                    id
                };
                let Some(id) = id else { break };
                self.running.lock().unwrap().insert(id.clone(), 0);
                let app = self.clone();
                tokio::spawn(async move {
                    app.run_job(id).await;
                });
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }
    async fn run_job(&self, id: String) {
        let result = self.spawn_worker(&id).await;
        self.running.lock().unwrap().remove(&id);
        match result {
            Ok(()) => {
                if let Err(e) = self.set_state(&id, "completed", None) {
                    tracing::error!("{e}");
                }
                log_job_timings(&self.dir(&id));
                let app = self.clone();
                let index_id = id.clone();
                if let Ok(Err(e)) =
                    tokio::task::spawn_blocking(move || app.index_job_for_search(&index_id)).await
                {
                    tracing::warn!("search index failed for {id}: {e:#}");
                }
            }
            Err(e) => {
                let _ = self.set_state(&id, "failed", Some(extract::failure(&e, None)));
            }
        }
    }
    async fn spawn_worker(&self, id: &str) -> Result<()> {
        let dir = self.dir(id);
        let mut command = worker_command(
            &dir,
            &self.config.limits,
            &self.config.data_dir.join("vision.lock"),
        )?;
        // Serialize launch and cancellation: cancellation sees either queued state or a registered process group.
        let child = {
            let mut running = self.running.lock().unwrap();
            let state = self.job(id)?.state;
            if state == "cancelled" || state == "deleting" {
                anyhow::bail!("cancelled");
            }
            let child = command.spawn()?;
            running.insert(id.into(), child.id().unwrap_or(0));
            child
        };
        let pid = child.id().unwrap_or(0);
        atomic_json(&dir.join("worker.pid"), &pid)?;
        supervise_worker(&dir, &self.config.limits, child).await
    }
}
/// Emit the per-page timing trail to the server log after a job completes. The same data is in
/// the job's `timings.json` and each unit's `timing` object.
fn log_job_timings(dir: &std::path::Path) {
    let Ok(bytes) = std::fs::read(dir.join("timings.json")) else {
        return;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return;
    };
    let Some(pages) = value.get("pages") else {
        return;
    };
    let job = dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("-");
    let number = |key: &str| {
        pages
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    tracing::info!(
        target: "doc_extractor::job",
        job,
        pages = number("pages"),
        native_pages = number("native_pages"),
        ocr_pages = number("ocr_pages"),
        hybrid_pages = number("hybrid_pages"),
        page_total_ms = number("page_total_ms"),
        ocr_ms = number("ocr_ms"),
        render_ms = number("render_ms"),
        wall_ms = number("wall_ms"),
        "job timing"
    );
    let Some(per_page) = pages.get("per_page").and_then(serde_json::Value::as_array) else {
        return;
    };
    for page in per_page {
        let text = |key: &str| {
            page.get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("-")
        };
        let number = |key: &str| {
            page.get(key)
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0)
        };
        tracing::info!(
            target: "doc_extractor::page",
            job,
            unit = text("unit"),
            page = number("page"),
            method = text("method"),
            provider = text("provider"),
            total_ms = number("total_ms"),
            native_ms = number("native_ms"),
            images_ms = number("images_ms"),
            render_ms = number("render_ms"),
            ocr_ms = number("ocr_ms"),
            page_image_ms = number("page_image_ms"),
            chars = number("chars"),
            tables = number("tables"),
            "page timing"
        );
    }
}

fn kill_group(pid: u32) {
    #[cfg(unix)]
    if pid > 0 {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
}

/// Bytes currently retained for a tenant across queued, running and terminal jobs.
fn tenant_usage(data_dir: &std::path::Path, db: &Connection, tenant: &str) -> Result<u64> {
    let ids: Vec<String> = db
        .prepare("SELECT id FROM jobs WHERE tenant=?1")?
        .query_map([tenant], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    let mut total = 0u64;
    for id in ids {
        let dir = data_dir.join("jobs").join(&id);
        if dir.exists() {
            total = total.saturating_add(directory_bytes(&dir)?);
        }
    }
    Ok(total)
}
/// Cap on files per multipart request; each file becomes an independent job.
const MAX_BATCH_FILES: usize = 32;
struct UploadedFile {
    id: String,
    filename: String,
    dir: PathBuf,
    size: u64,
    digest: sha2::Sha256,
}
async fn upload(
    State(app): State<App>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> ApiResult<Response> {
    let tenant = app.tenant(&headers)?;
    let mut created: Vec<(String, PathBuf)> = Vec::new();
    let result = upload_inner(&app, &tenant, &mut created, &mut multipart).await;
    if result.is_err() {
        for (_, dir) in &created {
            let _ = tokio::fs::remove_dir_all(dir).await;
        }
    }
    result
}
async fn upload_inner(
    app: &App,
    tenant: &str,
    created: &mut Vec<(String, PathBuf)>,
    multipart: &mut Multipart,
) -> ApiResult<Response> {
    let mut files: Vec<UploadedFile> = Vec::new();
    let mut options = Options::default();
    let mut seen_options = false;
    let mut callback_url: Option<String> = None;
    let mut seen_callback = false;
    use sha2::Digest;
    use tokio::io::AsyncWriteExt;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| multipart_error(e, "invalid_multipart"))?
    {
        match field.name().unwrap_or("") {
            "file" => {
                if files.len() >= MAX_BATCH_FILES {
                    return Err(error(
                        StatusCode::BAD_REQUEST,
                        "invalid_upload",
                        "Too many files in one request (max 32)",
                    ));
                }
                let id = uuid::Uuid::new_v4().to_string();
                let dir = app.dir(&id);
                tokio::fs::create_dir_all(&dir).await?;
                created.push((id.clone(), dir.clone()));
                let name = field.file_name().unwrap_or("document.txt").to_owned();
                let name = name
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or("document.txt")
                    .to_owned();
                if name.len() > 255 {
                    return Err(error(
                        StatusCode::BAD_REQUEST,
                        "invalid_filename",
                        "Filename too long",
                    ));
                }
                let mut out = tokio::fs::File::create(dir.join("input")).await?;
                let mut size = 0u64;
                let mut digest = sha2::Sha256::new();
                while let Some(chunk) = field
                    .chunk()
                    .await
                    .map_err(|e| multipart_error(e, "invalid_upload"))?
                {
                    size += chunk.len() as u64;
                    if size > app.config.limits.upload_bytes {
                        return Err(error(
                            StatusCode::PAYLOAD_TOO_LARGE,
                            "limit_exceeded",
                            "Upload too large",
                        ));
                    }
                    digest.update(&chunk);
                    out.write_all(&chunk).await?;
                }
                out.sync_all().await?;
                files.push(UploadedFile {
                    id,
                    filename: name,
                    dir,
                    size,
                    digest,
                });
            }
            "options" => {
                if seen_options {
                    return Err(error(
                        StatusCode::BAD_REQUEST,
                        "invalid_options",
                        "Duplicate options",
                    ));
                }
                seen_options = true;
                let b = field
                    .bytes()
                    .await
                    .map_err(|e| multipart_error(e, "invalid_options"))?;
                if b.len() > 8192 {
                    return Err(error(
                        StatusCode::BAD_REQUEST,
                        "invalid_options",
                        "Options too large",
                    ));
                }
                // Multipart clients (Swagger UI included) send an empty part for an
                // optional string field. Treat empty/whitespace as "use defaults".
                if b.iter().any(|byte| !byte.is_ascii_whitespace()) {
                    options = serde_json::from_slice(&b).map_err(|e| {
                        error(StatusCode::BAD_REQUEST, "invalid_options", &e.to_string())
                    })?;
                }
            }
            "callback_url" => {
                if seen_callback {
                    return Err(error(
                        StatusCode::BAD_REQUEST,
                        "invalid_callback",
                        "Duplicate callback_url",
                    ));
                }
                seen_callback = true;
                let b = field
                    .bytes()
                    .await
                    .map_err(|e| multipart_error(e, "invalid_callback"))?;
                if b.len() > 2048 {
                    return Err(error(
                        StatusCode::BAD_REQUEST,
                        "invalid_callback",
                        "Callback URL too long",
                    ));
                }
                let url = String::from_utf8(b.to_vec()).map_err(|_| {
                    error(
                        StatusCode::BAD_REQUEST,
                        "invalid_callback",
                        "Callback URL must be UTF-8",
                    )
                })?;
                if url.trim().is_empty() {
                    callback_url = None;
                } else {
                    validate_callback(url.trim(), app.config.webhook_allow_private)
                        .map_err(|m| error(StatusCode::BAD_REQUEST, "invalid_callback", m))?;
                    callback_url = Some(url.trim().to_owned());
                }
            }
            _ => {
                return Err(error(
                    StatusCode::BAD_REQUEST,
                    "invalid_field",
                    "Expected file and options fields",
                ))
            }
        }
    }
    if files.is_empty() {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_upload",
            "File required",
        ));
    }
    validate_options(&options)?;
    let options_json = serde_json::to_string(&options).map_err(anyhow::Error::from)?;
    let mut results: Vec<Value> = Vec::new();
    for file in files {
        let digest_hex = hex::encode(file.digest.finalize());
        match queue_job(
            app,
            tenant,
            &file.dir,
            &file.id,
            &file.filename,
            file.size,
            &digest_hex,
            &options,
            &options_json,
            callback_url.as_deref(),
        )? {
            Some(cached) => {
                tokio::fs::remove_dir_all(&file.dir).await?;
                results.push(json!({"id":cached,"cached":true,"filename":file.filename}));
            }
            None => results.push(json!({"id":file.id,"cached":false,"filename":file.filename})),
        }
    }
    if results.len() == 1 {
        let one = &results[0];
        Ok((
            StatusCode::ACCEPTED,
            Json(json!({"id":one["id"],"cached":one["cached"]})),
        )
            .into_response())
    } else {
        Ok((
            StatusCode::ACCEPTED,
            Json(json!({"jobs":results,"total":results.len()})),
        )
            .into_response())
    }
}

/// `<dir>/<name>` or `<dir>/<stem>.<n><ext>` so watched files never overwrite each other.
fn unique_target(dir: &std::path::Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let path = std::path::Path::new(name);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| name.to_owned());
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for n in 1..1000 {
        let candidate = dir.join(format!("{stem}.{n}{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{stem}.{}{ext}", epoch()))
}

/// Shared job-queueing path for uploads and the folder watcher: writes `request.json`,
/// computes the cache key and enforces queue/disk limits. `Ok(Some(id))` means a completed
/// job with the same content+options exists and the caller should drop the staged directory.
#[allow(clippy::too_many_arguments)]
fn queue_job(
    app: &App,
    tenant: &str,
    dir: &std::path::Path,
    id: &str,
    filename: &str,
    size: u64,
    digest_hex: &str,
    options: &Options,
    options_json: &str,
    callback_url: Option<&str>,
) -> ApiResult<Option<String>> {
    atomic_json(
        &dir.join("request.json"),
        &json!({"id":id,"filename":filename,"options":options,"limits":app.config.limits,"callback_url":callback_url}),
    )?;
    let key = extract::hash(
        format!(
            "{}:{}:{}:{}:{}",
            digest_hex,
            filename,
            options_json,
            extract::parser_fingerprint(),
            app.provider_fingerprint
        )
        .as_bytes(),
    );
    let db = app.db.lock().unwrap();
    let cached: Option<String> = db
        .query_row(
            "SELECT id FROM jobs WHERE tenant=?1 AND cache_key=?2 AND state='completed' ORDER BY created_at DESC LIMIT 1",
            params![tenant, key],
            |r| r.get(0),
        )
        .ok();
    let cached = if options.vision && std::env::var("DOC_VLM_REVISION").is_err() {
        None
    } else {
        cached
    };
    if cached.is_none() {
        let queued: i64 = db.query_row(
            "SELECT count(*) FROM jobs WHERE state IN ('queued','running')",
            [],
            |r| r.get(0),
        )?;
        if queued >= app.config.limits.queued_jobs as i64 {
            return Err(error(
                StatusCode::TOO_MANY_REQUESTS,
                "queue_full",
                "Job queue full",
            ));
        }
        // Quota applies to newly stored jobs; cache hits above are free. Retained bytes
        // include this upload plus everything already kept for the tenant.
        let used = tenant_usage(&app.config.data_dir, &db, tenant)?;
        if app.config.limits.disk_bytes > 0
            && used.saturating_add(size) > app.config.limits.disk_bytes
        {
            return Err(error(
                StatusCode::INSUFFICIENT_STORAGE,
                "quota_exceeded",
                "Tenant disk quota exceeded; DELETE completed jobs or raise disk_bytes",
            ));
        }
        db.execute("INSERT INTO jobs(id,tenant,filename,cache_key,options,state,created_at,updated_at,callback_url) VALUES(?1,?2,?3,?4,?5,'queued',?6,?6,?7)",params![id,tenant,filename,key,options_json,epoch(),callback_url])?;
    }
    Ok(cached)
}

pub fn validate_options(options: &Options) -> ApiResult<()> {
    if !["direct", "xberg"].contains(&options.parser.as_str()) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_options",
            "Unknown parser",
        ));
    }
    if !["tesseract", "apple_vision", "smoldocling"].contains(&options.ocr_provider.as_str()) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_options",
            "Unknown OCR provider",
        ));
    }
    if options.languages.len() > 100
        || !options
            .languages
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '+')
    {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_options",
            "Invalid OCR languages",
        ));
    }
    if options
        .delimiter
        .is_some_and(|d| !d.is_ascii() || d == '\n' || d == '\r')
    {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_options",
            "Delimiter must be ASCII",
        ));
    }
    if ![
        "json", "markdown", "md", "text", "txt", "jsonl", "ndjson", "html",
    ]
    .contains(&options.result_format.as_str())
    {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_options",
            "Expected result_format json, markdown, text, jsonl or html",
        ));
    }
    if !["none", "failed", "all"].contains(&options.page_images.as_str()) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_options",
            "Expected page_images none, failed or all",
        ));
    }
    if let Some(spec) = &options.pages {
        crate::model::parse_page_range(spec).map_err(|e| {
            error(
                StatusCode::BAD_REQUEST,
                "invalid_options",
                &format!("Invalid page range: {e}"),
            )
        })?;
    }
    if let Some(target) = options.chunk_chars {
        if !(16..=100_000).contains(&target) {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "invalid_options",
                "chunk_chars must be 16..=100000",
            ));
        }
    }
    if let Some(overlap) = options.chunk_overlap {
        let target = options.chunk_chars.ok_or_else(|| {
            error(
                StatusCode::BAD_REQUEST,
                "invalid_options",
                "chunk_overlap requires chunk_chars",
            )
        })?;
        if overlap >= target {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "invalid_options",
                "chunk_overlap must be smaller than chunk_chars",
            ));
        }
    }
    if options.result_format == "chunks" && options.chunk_chars.is_none() {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_options",
            "result_format chunks requires chunk_chars",
        ));
    }
    Ok(())
}
async fn status(
    State(app): State<App>,
    ApiPath(id): ApiPath<String>,
    headers: HeaderMap,
) -> ApiResult<Json<Job>> {
    app.authorize(&id, &headers)?;
    Ok(Json(app.job(&id)?))
}
/// Persist reviewer edits and page verification in a sidecar. The canonical extraction is
/// immutable; `review.json` is an overlay used by the reader and reviewed exports.
async fn update_review(
    State(app): State<App>,
    ApiPath(id): ApiPath<String>,
    headers: HeaderMap,
    Query(query): Query<UpdateQuery>,
    Json(update): Json<ReviewUpdate>,
) -> ApiResult<Response> {
    app.authorize(&id, &headers)?;
    if query.library.unwrap_or(false) {
        let job = app.job(&id)?;
        let display_name = update
            .display_name
            .as_deref()
            .map(|value| value.trim().to_owned())
            .map(|value| if value.is_empty() { None } else { Some(value) })
            .unwrap_or(job.display_name);
        if display_name.as_ref().is_some_and(|name| name.len() > 200) {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "invalid_library",
                "Display name is too long",
            ));
        }
        let collection = update
            .collection
            .as_deref()
            .map(|value| value.trim().to_owned())
            .map(|value| if value.is_empty() { None } else { Some(value) })
            .unwrap_or(job.collection);
        if collection.as_ref().is_some_and(|name| name.len() > 100) {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "invalid_library",
                "Collection name is too long",
            ));
        }
        let mut tags = update.tags.unwrap_or(job.tags);
        tags = tags
            .into_iter()
            .map(|tag| tag.trim().to_owned())
            .filter(|tag| !tag.is_empty())
            .collect();
        tags.sort();
        tags.dedup();
        if tags.len() > 32 || tags.iter().any(|tag| tag.len() > 40) {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "invalid_library",
                "Use at most 32 tags of 40 characters or less",
            ));
        }
        let favorite = update.favorite.unwrap_or(job.favorite);
        app.db.lock().unwrap().execute(
            "UPDATE jobs SET display_name=?2,collection=?3,tags=?4,favorite=?5,updated_at=?6 WHERE id=?1",
            params![
                id,
                display_name,
                collection,
                serde_json::to_string(&tags).unwrap_or_else(|_| "[]".into()),
                if favorite { 1i64 } else { 0i64 },
                epoch()
            ],
        )?;
        return Ok(Json(app.job(&id)?).into_response());
    }
    if query.redaction.unwrap_or(false) {
        if app.job(&id)?.state != "completed" {
            return Err(error(
                StatusCode::CONFLICT,
                "not_ready",
                "Redaction is available after extraction completes",
            ));
        }
        let redactions = if update.clear_redactions.unwrap_or(false) {
            Vec::new()
        } else {
            update.redactions.unwrap_or_default()
        };
        if redactions.len() > 500 {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "invalid_redaction",
                "At most 500 redactions can be stored",
            ));
        }
        for spec in &redactions {
            let prefix = format!("{}-b", spec.unit_id);
            if !valid_part(&spec.unit_id, 'u')
                || !spec.block_id.starts_with(&prefix)
                || spec.block_id.len() > 96
                || !spec
                    .block_id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
                || spec.value.is_empty()
                || spec.value.len() > 500
                || spec.category.len() > 40
                || spec.row.is_some() != spec.column.is_some()
            {
                return Err(error(
                    StatusCode::BAD_REQUEST,
                    "invalid_redaction",
                    "Invalid redaction target",
                ));
            }
        }
        let value = json!({
            "schema_version": "redaction1",
            "updated_at": epoch(),
            "redactions": redactions,
        });
        atomic_json(&app.dir(&id).join("redaction.json"), &value)?;
        return Ok(Json(value).into_response());
    }
    if app.job(&id)?.state != "completed" {
        return Err(error(
            StatusCode::CONFLICT,
            "not_ready",
            "Review is available after extraction completes",
        ));
    }
    let unit_id = update
        .unit_id
        .clone()
        .filter(|unit| valid_part(unit, 'u'))
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "invalid_review", "Unknown unit"))?;
    if let Some(block) = &update.block_id {
        let prefix = format!("{unit_id}-b");
        if !block.starts_with(&prefix)
            || block.len() > 96
            || !block.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "invalid_review",
                "Unknown block",
            ));
        }
        if update.text.is_none() && !update.revert {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "invalid_review",
                "Block updates need text or revert=true",
            ));
        }
    } else if update.text.is_some() || update.revert {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_review",
            "block_id is required for text corrections",
        ));
    }
    if update.cell_row.is_some() != update.cell_column.is_some() {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_review",
            "cell_row and cell_column must be supplied together",
        ));
    }
    if update.cell_row.is_some() && update.block_id.is_none() {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_review",
            "block_id is required for cell corrections",
        ));
    }
    if update
        .text
        .as_ref()
        .or(update.original.as_ref())
        .is_some_and(|text| text.len() > 200_000)
    {
        return Err(error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "limit_exceeded",
            "Corrected text is too large",
        ));
    }
    if update.note.as_ref().is_some_and(|note| note.len() > 4_000) {
        return Err(error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "limit_exceeded",
            "Review note is too large",
        ));
    }
    if let Some(status) = &update.status {
        if !["reviewed", "needs_review", "clear"].contains(&status.as_str()) {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "invalid_review",
                "Expected status reviewed, needs_review or clear",
            ));
        }
    }
    if update.text.is_none() && !update.revert && update.status.is_none() && update.note.is_none() {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_review",
            "No review changes supplied",
        ));
    }
    let dir = app.dir(&id);
    let path = dir.join("review.json");
    if tokio::fs::metadata(&path)
        .await
        .map(|meta| meta.len() > 4 * 1024 * 1024)
        .unwrap_or(false)
    {
        return Err(error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "limit_exceeded",
            "Review sidecar is too large",
        ));
    }
    let mut review = read_review(&dir);
    if !review.get("units").is_some_and(Value::is_object) {
        review["units"] = json!({});
    }
    let units = review["units"].as_object_mut().unwrap();
    let unit = units.entry(unit_id.clone()).or_insert_with(|| json!({}));
    if !unit.is_object() {
        *unit = json!({});
    }
    if let Some(block_id) = &update.block_id {
        if let (Some(row), Some(column)) = (update.cell_row, update.cell_column) {
            if !unit.get("cells").is_some_and(Value::is_object) {
                unit["cells"] = json!({});
            }
            if !unit["cells"].get(block_id).is_some_and(Value::is_object) {
                unit["cells"][block_id.as_str()] = json!({});
            }
            let key = format!("{row},{column}");
            if update.revert {
                if let Some(cells) = unit["cells"]
                    .get_mut(block_id)
                    .and_then(Value::as_object_mut)
                {
                    cells.remove(&key);
                }
            } else if let Some(text) = &update.text {
                let mut entry = json!({"text":text,"updated_at":epoch()});
                if let Some(original) = &update.original {
                    entry["original"] = json!(original);
                }
                unit["cells"][block_id.as_str()][key.as_str()] = entry;
            }
        } else if update.revert {
            if let Some(blocks) = unit.get_mut("blocks").and_then(Value::as_object_mut) {
                blocks.remove(block_id);
            }
        } else if let Some(text) = &update.text {
            if !unit.get("blocks").is_some_and(Value::is_object) {
                unit["blocks"] = json!({});
            }
            let mut entry = json!({"text":text,"updated_at":epoch()});
            if let Some(original) = &update.original {
                entry["original"] = json!(original);
            }
            unit["blocks"][block_id.as_str()] = entry;
        }
        // A correction invalidates an earlier verification unless the same request
        // explicitly sets the status afterwards.
        if unit.get("status").and_then(Value::as_str) == Some("reviewed") && update.status.is_none()
        {
            unit["status"] = json!("needs_review");
        }
    }
    if let Some(status) = &update.status {
        if status == "clear" {
            if let Some(object) = unit.as_object_mut() {
                object.remove("status");
            }
        } else {
            unit["status"] = json!(status);
        }
    }
    if let Some(note) = &update.note {
        unit["note"] = json!(note);
    }
    unit["updated_at"] = json!(epoch());
    review["updated_at"] = json!(epoch());
    atomic_json(&path, &review)?;
    let worker = app.clone();
    let index_id = id.clone();
    tokio::spawn(async move {
        let _ = tokio::task::spawn_blocking(move || worker.index_job_for_search(&index_id)).await;
    });
    Ok(Json(review).into_response())
}
#[derive(Deserialize)]
struct Export {
    format: Option<String>,
    /// Apply the reviewed-text overlay when generating markdown/text/html.
    review: Option<bool>,
    /// Apply saved redactions to human-facing exports.
    redact: Option<bool>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RedactionSpec {
    unit_id: String,
    block_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    row: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    column: Option<u32>,
    category: String,
    value: String,
}
#[derive(Debug, Serialize)]
struct RedactionMatch {
    unit_id: String,
    unit_name: String,
    block_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    row: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    column: Option<u32>,
    category: String,
    value: String,
    preview: String,
    context: String,
}
#[derive(Deserialize)]
struct AskRequest {
    question: String,
    #[serde(default)]
    max_sources: Option<usize>,
}

#[derive(Deserialize)]
struct ReviewUpdate {
    #[serde(default)]
    unit_id: Option<String>,
    #[serde(default)]
    block_id: Option<String>,
    #[serde(default)]
    text: Option<String>,
    /// Raw text before the correction, kept so the UI can revert without re-reading the unit.
    #[serde(default)]
    original: Option<String>,
    /// When present with `cell_row`, the correction targets one table cell instead of a
    /// whole block.
    #[serde(default)]
    cell_row: Option<u32>,
    #[serde(default)]
    cell_column: Option<u32>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    revert: bool,
    // Library metadata updates (`?library=true`).
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    collection: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    favorite: Option<bool>,
    // Redaction updates (`?redaction=true`).
    #[serde(default)]
    redactions: Option<Vec<RedactionSpec>>,
    #[serde(default)]
    clear_redactions: Option<bool>,
}
#[derive(Deserialize)]
struct UpdateQuery {
    library: Option<bool>,
    redaction: Option<bool>,
}
/// `result_format` recorded at upload time; used when the request has no `?format=`.
async fn job_result_format(app: &App, id: &str) -> Option<String> {
    let bytes = tokio::fs::read(app.dir(id).join("request.json"))
        .await
        .ok()?;
    let request: Value = serde_json::from_slice(&bytes).ok()?;
    request
        .get("options")?
        .get("result_format")?
        .as_str()
        .map(str::to_owned)
}
fn empty_review() -> Value {
    json!({"schema_version":"review1","updated_at":0,"units":{}})
}

fn read_review(dir: &std::path::Path) -> Value {
    std::fs::read(dir.join("review.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .filter(Value::is_object)
        .unwrap_or_else(empty_review)
}

fn review_has_corrections(review: &Value) -> bool {
    review
        .get("units")
        .and_then(Value::as_object)
        .is_some_and(|units| {
            units.values().any(|unit| {
                unit.get("blocks")
                    .and_then(Value::as_object)
                    .is_some_and(|blocks| !blocks.is_empty())
                    || unit
                        .get("cells")
                        .and_then(Value::as_object)
                        .is_some_and(|cells| !cells.is_empty())
            })
        })
}

/// Apply reviewer text corrections to a copy of the canonical document. The canonical
/// `result.json` is never modified; this overlay is used only for human-facing exports.
fn apply_review(document: &mut Document, review: &Value) {
    let Some(units) = review.get("units").and_then(Value::as_object) else {
        return;
    };
    for unit in &mut document.units {
        let Some(reviewed) = units.get(&unit.id) else {
            continue;
        };
        if let Some(blocks) = reviewed.get("blocks").and_then(Value::as_object) {
            for block in &mut unit.blocks {
                if let Some(text) = blocks
                    .get(&block.id)
                    .and_then(|entry| entry.get("text"))
                    .and_then(Value::as_str)
                {
                    block.text = text.to_owned();
                }
            }
        }
        let Some(cells) = reviewed.get("cells").and_then(Value::as_object) else {
            continue;
        };
        for block in &mut unit.blocks {
            let Some(table) = &mut block.table else {
                continue;
            };
            let Some(overrides) = cells.get(&block.id).and_then(Value::as_object) else {
                continue;
            };
            for cell in &mut table.cells {
                let key = format!("{},{}", cell.row, cell.column);
                if let Some(text) = overrides
                    .get(&key)
                    .and_then(|entry| entry.get("text"))
                    .and_then(Value::as_str)
                {
                    cell.raw = text.to_owned();
                    cell.value = json!(text);
                }
            }
        }
    }
}

fn reviewed_html(document: &Document) -> String {
    let mut out = String::new();
    out.push_str("<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">");
    out.push_str(&format!(
        "<title>{}</title>\n<style>body{{font:14px/1.5 -apple-system,system-ui,sans-serif;margin:2rem auto;max-width:70rem;padding:0 1rem}}table{{border-collapse:collapse;margin:.6rem 0}}td,th{{border:1px solid #bbb;padding:.2rem .45rem;vertical-align:top;max-width:28rem;overflow-wrap:anywhere}}tbody tr:nth-child(even){{background:rgba(127,127,127,.09)}}pre,p{{overflow-wrap:anywhere}}</style>\n</head>\n<body>\n<h1>{}</h1>\n<p>Status: {} · reviewed text</p>\n",
        crate::model::html_escape(&document.filename),
        crate::model::html_escape(&document.filename),
        crate::model::html_escape(&document.status)
    ));
    for unit in &document.units {
        crate::model::unit_html(unit, &mut out);
    }
    out.push_str("</body>\n</html>\n");
    out
}

fn pii_patterns() -> &'static [(&'static str, Regex)] {
    static PATTERNS: std::sync::OnceLock<Vec<(&'static str, Regex)>> = std::sync::OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            (
                "email",
                Regex::new(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}").unwrap(),
            ),
            ("ssn", Regex::new(r"\b\d{3}-\d{2}-\d{4}\b").unwrap()),
            ("card", Regex::new(r"\b\d(?:[ -]?\d){12,18}\b").unwrap()),
            (
                "aadhaar",
                Regex::new(r"\b\d{4}[\s-]\d{4}[\s-]\d{4}\b").unwrap(),
            ),
            ("pan", Regex::new(r"\b[A-Z]{5}[0-9]{4}[A-Z]\b").unwrap()),
            (
                "gstin",
                Regex::new(r"\b\d{2}[A-Z]{5}\d{4}[A-Z][A-Z0-9]Z[A-Z0-9]\b").unwrap(),
            ),
            (
                "iban",
                Regex::new(r"\b[A-Z]{2}\d{2}[A-Z0-9]{11,30}\b").unwrap(),
            ),
            ("phone", Regex::new(r"\+?\d[\d ().-]{6,18}\d").unwrap()),
        ]
    })
}

fn luhn_valid(value: &str) -> bool {
    let digits: Vec<u32> = value
        .chars()
        .filter(|c| c.is_ascii_digit())
        .filter_map(|c| c.to_digit(10))
        .collect();
    if digits.len() < 13 || digits.len() > 19 {
        return false;
    }
    let mut sum = 0u32;
    let parity = digits.len() % 2;
    for (index, digit) in digits.iter().enumerate() {
        let mut value = *digit;
        if index % 2 == parity {
            value *= 2;
            if value > 9 {
                value -= 9;
            }
        }
        sum += value;
    }
    sum % 10 == 0
}

fn mask_pii(category: &str, value: &str) -> String {
    match category {
        "email" => {
            let (local, domain) = value.split_once('@').unwrap_or((value, ""));
            format!("{}***@{domain}", local.chars().next().unwrap_or('*'))
        }
        "card" => {
            let digits: String = value.chars().filter(|c| c.is_ascii_digit()).collect();
            let tail: String = digits
                .chars()
                .rev()
                .take(4)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            format!("**** **** **** {tail}")
        }
        "phone" => {
            let digits: String = value.chars().filter(|c| c.is_ascii_digit()).collect();
            let tail: String = digits
                .chars()
                .rev()
                .take(2)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            format!("*** *** {tail}")
        }
        "ssn" => {
            let tail: String = value
                .chars()
                .rev()
                .take(4)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            format!("***-**-{tail}")
        }
        _ => {
            let chars: Vec<char> = value.chars().collect();
            let head: String = chars.iter().take(2).collect();
            let tail: String = chars
                .iter()
                .rev()
                .take(2)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            format!("{head}••••{tail}")
        }
    }
}

fn pii_spans(text: &str) -> Vec<(usize, usize, String, String)> {
    let mut candidates: Vec<(usize, usize, &str, u8)> = Vec::new();
    for (priority, (category, pattern)) in pii_patterns().iter().enumerate() {
        for found in pattern.find_iter(text) {
            let value = found.as_str();
            if *category == "card" && !luhn_valid(value) {
                continue;
            }
            if *category == "phone" {
                let digits = value.chars().filter(|c| c.is_ascii_digit()).count();
                if !(8..=15).contains(&digits) {
                    continue;
                }
            }
            candidates.push((found.start(), found.end(), category, priority as u8));
        }
    }
    candidates.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.3.cmp(&b.3))
            .then((b.1 - b.0).cmp(&(a.1 - a.0)))
    });
    let mut accepted: Vec<(usize, usize, String, String)> = Vec::new();
    let mut last_end = 0usize;
    for (start, end, category, _) in candidates {
        if start < last_end {
            continue;
        }
        accepted.push((start, end, category.to_owned(), text[start..end].to_owned()));
        last_end = end;
    }
    accepted
}

fn detect_redactions(document: &Document) -> Vec<RedactionMatch> {
    let mut matches = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut scan =
        |unit: &Unit, block_id: &str, row: Option<u32>, column: Option<u32>, text: &str| {
            let spans = pii_spans(text);
            for (start, end, category, value) in &spans {
                let key = (
                    unit.id.clone(),
                    block_id.to_owned(),
                    row,
                    column,
                    category.clone(),
                    value.clone(),
                );
                if !seen.insert(key) {
                    continue;
                }
                // Context must not leak the *other* identifiers on the same line: replace
                // every detected span before rendering the snippet.
                let mut prefix = text[..*start].to_owned();
                for (span_start, _, other_category, other_value) in &spans {
                    if span_start < start {
                        prefix =
                            prefix.replace(other_value, &mask_pii(other_category, other_value));
                    }
                }
                let mut suffix = text[*end..].to_owned();
                for (span_start, _, other_category, other_value) in &spans {
                    if span_start >= end {
                        suffix =
                            suffix.replace(other_value, &mask_pii(other_category, other_value));
                    }
                }
                let prefix: String = prefix
                    .chars()
                    .rev()
                    .take(35)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
                let suffix: String = suffix.chars().take(35).collect();
                let context = format!("{prefix}{}{suffix}", mask_pii(category, value))
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                matches.push(RedactionMatch {
                    unit_id: unit.id.clone(),
                    unit_name: unit.name.clone(),
                    block_id: block_id.to_owned(),
                    row,
                    column,
                    category: category.clone(),
                    value: value.clone(),
                    preview: mask_pii(category, value),
                    context,
                });
                if matches.len() >= 2000 {
                    return;
                }
            }
        };
    for unit in &document.units {
        for block in &unit.blocks {
            if let Some(table) = &block.table {
                for cell in &table.cells {
                    if !cell.raw.is_empty() {
                        scan(
                            unit,
                            &block.id,
                            Some(cell.row),
                            Some(cell.column),
                            &cell.raw,
                        );
                    }
                }
            } else if !block.text.is_empty() {
                scan(unit, &block.id, None, None, &block.text);
            }
        }
    }
    matches
}

fn empty_redactions() -> Value {
    json!({"schema_version":"redaction1","updated_at":0,"redactions":[]})
}

fn read_redactions(dir: &std::path::Path) -> Value {
    std::fs::read(dir.join("redaction.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .filter(Value::is_object)
        .unwrap_or_else(empty_redactions)
}

fn redaction_specs(value: &Value) -> Vec<RedactionSpec> {
    value
        .get("redactions")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| serde_json::from_value::<RedactionSpec>(item.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn apply_redactions(document: &mut Document, specs: &[RedactionSpec]) {
    for spec in specs {
        let Some(unit) = document
            .units
            .iter_mut()
            .find(|unit| unit.id == spec.unit_id)
        else {
            continue;
        };
        let Some(block) = unit
            .blocks
            .iter_mut()
            .find(|block| block.id == spec.block_id)
        else {
            continue;
        };
        if let (Some(row), Some(column)) = (spec.row, spec.column) {
            if let Some(table) = &mut block.table {
                if let Some(cell) = table
                    .cells
                    .iter_mut()
                    .find(|cell| cell.row == row && cell.column == column)
                {
                    if !spec.value.is_empty() {
                        cell.raw = cell.raw.replace(&spec.value, "[REDACTED]");
                        cell.value = json!(cell.raw);
                    }
                }
            }
        } else if !spec.value.is_empty() {
            block.text = block.text.replace(&spec.value, "[REDACTED]");
        }
    }
}

async fn review_export(
    dir: &std::path::Path,
    format: &str,
    review: &Value,
    redactions: &[RedactionSpec],
) -> ApiResult<Response> {
    let bytes = tokio::fs::read(dir.join("result.json")).await?;
    let mut document: Document = serde_json::from_slice(&bytes).map_err(anyhow::Error::from)?;
    apply_review(&mut document, review);
    apply_redactions(&mut document, redactions);
    let (body, mime) = match format {
        "markdown" | "md" => (
            crate::model::markdown(&document),
            "text/markdown; charset=utf-8",
        ),
        "text" | "txt" => (
            crate::model::plaintext(&document),
            "text/plain; charset=utf-8",
        ),
        "html" => (reviewed_html(&document), "text/html; charset=utf-8"),
        _ => {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "invalid_format",
                "Reviewed export supports markdown, text or html",
            ))
        }
    };
    Ok((
        [
            (header::CONTENT_TYPE, mime.to_owned()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".into()),
        ],
        body,
    )
        .into_response())
}

/// Self-contained, printable evidence report. Page images are embedded as data URIs so the
/// file can be opened or archived without the service or a sidecar asset directory.
async fn report_export(
    dir: &std::path::Path,
    review: &Value,
    redactions: &[RedactionSpec],
) -> ApiResult<Response> {
    let bytes = tokio::fs::read(dir.join("result.json")).await?;
    let mut document: Document = serde_json::from_slice(&bytes).map_err(anyhow::Error::from)?;
    apply_review(&mut document, review);
    apply_redactions(&mut document, redactions);
    let redacted = !redactions.is_empty();
    const MAX_IMAGE_BYTES: u64 = 32 * 1024 * 1024;
    let mut embedded = 0u64;
    let mut out = String::new();
    out.push_str("<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">");
    out.push_str(&format!(
        "<title>{} — extraction report</title><style>",
        crate::model::html_escape(&document.filename)
    ));
    out.push_str(":root{--ink:#102229;--muted:#61757d;--line:#d7e2e3;--accent:#0d6a72;--warm:#8a5a1e}*{box-sizing:border-box}body{margin:0;background:#eef4f4;color:var(--ink);font:14px/1.6 -apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif}.report{max-width:960px;margin:0 auto;padding:36px 20px 70px}h1,h2,h3{font-family:'Iowan Old Style','Palatino Linotype',Palatino,Georgia,serif;font-weight:500;letter-spacing:-.02em}h1{font-size:38px;margin:0 0 6px}.meta{color:var(--muted);font-size:12px;margin-bottom:22px}.summary{display:grid;grid-template-columns:repeat(4,minmax(0,1fr));gap:8px;margin:18px 0 26px}.stat{padding:11px 13px;background:#fff;border:1px solid var(--line);border-radius:10px}.stat strong{display:block;font-size:20px}.stat span{color:var(--muted);font-size:10px;text-transform:uppercase;letter-spacing:.06em}.toc{display:flex;flex-wrap:wrap;gap:6px;margin-bottom:24px}.toc a{padding:4px 8px;color:var(--accent);font-size:11px;text-decoration:none;background:#fff;border:1px solid var(--line);border-radius:999px}.page{padding:18px 20px;margin-bottom:16px;background:#fff;border:1px solid var(--line);border-radius:14px;break-inside:avoid}.page>h2{font-size:22px;margin:0 0 12px;padding-bottom:8px;border-bottom:1px solid var(--line)}.badges{display:flex;gap:5px;flex-wrap:wrap;margin-bottom:12px;font-size:10px}.badge{padding:3px 7px;border:1px solid var(--line);border-radius:999px;color:var(--muted)}.badge.ok{color:#177a5b;background:#e2f3eb}.badge.warn{color:var(--warm);background:#fbf0dd}.page img{display:block;max-width:100%;max-height:720px;margin:0 auto 14px;border:1px solid var(--line);border-radius:8px}.unit{font-size:13px}.unit h2{display:none}.unit table{border-collapse:collapse;width:100%;margin:8px 0;font-size:11px}.unit td,.unit th{border:1px solid var(--line);padding:4px 6px;text-align:left;vertical-align:top}.unit p{white-space:pre-wrap;overflow-wrap:anywhere}.warning{padding:7px 9px;margin:6px 0;color:#6b4a1f;background:#fbf0dd;border-left:3px solid var(--warm);border-radius:6px;font-size:11px}.footer{margin-top:24px;color:var(--muted);font-size:10px;text-align:center}@media print{body{background:#fff}.report{max-width:none;padding:0}.page{border:0;padding:0;margin:0 0 24px;break-after:page}.toc{display:none}.summary{grid-template-columns:repeat(4,1fr)}}@media(max-width:640px){.summary{grid-template-columns:repeat(2,1fr)}h1{font-size:30px}}");
    out.push_str("</style></head><body><main class=\"report\">");
    out.push_str(&format!(
        "<h1>{}</h1><div class=\"meta\">Extraction report · {} · {} · {} unit(s)</div>",
        crate::model::html_escape(&document.filename),
        crate::model::html_escape(&document.status),
        document.schema_version,
        document.units.len()
    ));
    if redacted {
        out.push_str("<div class=\"meta\" style=\"color:#8a5a1e;margin-top:-12px\">Redacted report: page images are omitted because image content cannot be redacted here.</div>");
    }
    let corrected = review_has_corrections(review);
    let reviewed_units = review
        .get("units")
        .and_then(Value::as_object)
        .map(|units| {
            units
                .values()
                .filter(|unit| unit.get("status").and_then(Value::as_str) == Some("reviewed"))
                .count()
        })
        .unwrap_or(0);
    let table_count: usize = document
        .units
        .iter()
        .flat_map(|unit| unit.blocks.iter())
        .filter(|block| block.table.is_some())
        .count();
    out.push_str(&format!(
        "<div class=\"summary\"><div class=\"stat\"><strong>{}</strong><span>Pages</span></div><div class=\"stat\"><strong>{}</strong><span>Tables</span></div><div class=\"stat\"><strong>{}</strong><span>Verified</span></div><div class=\"stat\"><strong>{}</strong><span>Review edits</span></div></div>",
        document.units.len(),
        table_count,
        reviewed_units,
        if corrected { "Yes" } else { "No" }
    ));
    if document.units.len() > 1 {
        out.push_str("<nav class=\"toc\">");
        for unit in &document.units {
            out.push_str(&format!(
                "<a href=\"#{}\">{}</a>",
                crate::model::html_escape(&unit.id),
                crate::model::html_escape(&unit.name)
            ));
        }
        out.push_str("</nav>");
    }
    for unit in &document.units {
        out.push_str("<section class=\"page\">");
        let image_id = unit
            .blocks
            .iter()
            .find_map(|block| {
                block
                    .attributes
                    .get("page_image")
                    .and_then(|meta| meta.get("asset_id"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .or_else(|| {
                unit.blocks.iter().find_map(|block| {
                    if block.kind == "figure" {
                        block.asset_id.clone()
                    } else {
                        None
                    }
                })
            });
        if !redacted {
            if let Some(asset_id) = image_id {
                if let Some(asset) = document
                    .assets
                    .iter()
                    .find(|asset| asset.id == asset_id && asset.mime.starts_with("image/"))
                {
                    if embedded + asset.bytes <= MAX_IMAGE_BYTES {
                        if let Ok(data) = tokio::fs::read(dir.join("assets").join(&asset.id)).await
                        {
                            embedded += data.len() as u64;
                            out.push_str(&format!(
                                "<img src=\"data:{};base64,{}\" alt=\"{}\">",
                                asset.mime,
                                base64::engine::general_purpose::STANDARD.encode(data),
                                crate::model::html_escape(&unit.name)
                            ));
                        }
                    }
                }
            }
        }
        let review_state = review.get("units").and_then(|units| units.get(&unit.id));
        let status = review_state
            .and_then(|state| state.get("status"))
            .and_then(Value::as_str);
        let edits = review_state
            .map(|state| {
                state
                    .get("blocks")
                    .and_then(Value::as_object)
                    .map(|blocks| blocks.len())
                    .unwrap_or(0)
                    + state
                        .get("cells")
                        .and_then(Value::as_object)
                        .map(|cells| {
                            cells
                                .values()
                                .filter_map(Value::as_object)
                                .map(|cells| cells.len())
                                .sum::<usize>()
                        })
                        .unwrap_or(0)
            })
            .unwrap_or(0);
        out.push_str("<div class=\"badges\">");
        if let Some(timing) = &unit.timing {
            out.push_str(&format!(
                "<span class=\"badge\">{} · {} ms</span>",
                crate::model::html_escape(&timing.method),
                timing.total_ms
            ));
        }
        if status == Some("reviewed") {
            out.push_str("<span class=\"badge ok\">Verified</span>");
        }
        if edits > 0 {
            out.push_str(&format!(
                "<span class=\"badge warn\">{edits} edit(s)</span>"
            ));
        }
        out.push_str("</div>");
        crate::model::unit_html(unit, &mut out);
        for warning in &unit.warnings {
            if warning.starts_with("Scanned page:") || warning.starts_with("Poppler reading order")
            {
                continue;
            }
            out.push_str(&format!(
                "<div class=\"warning\">{}</div>",
                crate::model::html_escape(warning)
            ));
        }
        out.push_str("</section>");
    }
    out.push_str(&format!(
        "<div class=\"footer\">Generated by doc-extractor · {} · self-contained report</div></main></body></html>",
        crate::model::html_escape(&document.id)
    ));
    Ok((
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_owned()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
        ],
        out,
    )
        .into_response())
}

struct AskChunk {
    unit_id: String,
    unit_name: String,
    block_id: String,
    page: usize,
    order: usize,
    text: String,
    score: usize,
}

fn ask_chunks(document: &Document, question: &str, max_sources: usize) -> Vec<AskChunk> {
    let lower_question = question.to_lowercase();
    let stop = [
        "the", "and", "for", "with", "from", "that", "this", "what", "which", "where", "when",
        "how", "why", "does", "did", "are", "was", "were", "can", "could", "would", "should",
        "about", "into", "than", "then", "there", "their", "have", "has", "had", "you", "your",
        "document", "page",
    ];
    let terms: Vec<String> = lower_question
        .split(|c: char| !c.is_alphanumeric())
        .filter(|term| term.len() >= 3 && !stop.contains(term))
        .map(str::to_owned)
        .collect();
    let broad = terms.len() <= 1
        || [
            "summary",
            "summarize",
            "overview",
            "main points",
            "key points",
        ]
        .iter()
        .any(|word| lower_question.contains(word));
    let mut chunks = Vec::new();
    let mut order = 0usize;
    for unit in &document.units {
        for block in &unit.blocks {
            let mut text = if let Some(table) = &block.table {
                let cells: std::collections::HashMap<_, _> = table
                    .cells
                    .iter()
                    .map(|cell| ((cell.row, cell.column), cell.raw.as_str()))
                    .collect();
                let mut out = String::new();
                for row in 0..table.rows.min(30) {
                    for column in 0..table.columns {
                        if column > 0 {
                            out.push('\t');
                        }
                        out.push_str(
                            cells
                                .get(&(row as u32, column as u32))
                                .copied()
                                .unwrap_or(""),
                        );
                    }
                    out.push('\n');
                    if out.len() > 2400 {
                        break;
                    }
                }
                out
            } else {
                block.text.clone()
            };
            if text.trim().is_empty() {
                continue;
            }
            if text.chars().count() > 2400 {
                text = text.chars().take(2400).collect();
            }
            let text_lower = text.to_lowercase();
            let mut score = 0usize;
            for term in &terms {
                score += text_lower.matches(term).count().min(6);
                if unit.name.to_lowercase().contains(term) {
                    score += 2;
                }
            }
            chunks.push(AskChunk {
                unit_id: unit.id.clone(),
                unit_name: unit.name.clone(),
                block_id: block.id.clone(),
                page: unit.index + 1,
                order,
                text,
                score,
            });
            order += 1;
        }
    }
    if chunks.is_empty() {
        return chunks;
    }
    let max_sources = max_sources.clamp(3, 20);
    if broad && chunks.len() > max_sources {
        let step = (chunks.len() / max_sources).max(1);
        let mut sampled: Vec<AskChunk> = chunks
            .into_iter()
            .enumerate()
            .filter(|(index, _)| index % step == 0)
            .map(|(_, chunk)| chunk)
            .take(max_sources)
            .collect();
        if sampled.is_empty() {
            return Vec::new();
        }
        sampled.sort_by_key(|chunk| chunk.order);
        return sampled;
    }
    chunks.sort_by(|a, b| b.score.cmp(&a.score).then(a.order.cmp(&b.order)));
    chunks.truncate(max_sources);
    chunks.sort_by_key(|chunk| chunk.order);
    chunks
}

async fn ask_result(
    State(app): State<App>,
    ApiPath(id): ApiPath<String>,
    headers: HeaderMap,
    Json(request): Json<AskRequest>,
) -> ApiResult<Response> {
    app.authorize(&id, &headers)?;
    if app.job(&id)?.state != "completed" {
        return Err(error(
            StatusCode::CONFLICT,
            "not_ready",
            "Questions are available after extraction completes",
        ));
    }
    let question = request.question.trim();
    if question.len() < 2 || question.chars().count() > 2000 {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_question",
            "Ask a question between 2 and 2000 characters",
        ));
    }
    let dir = app.dir(&id);
    let bytes = tokio::fs::read(dir.join("result.json")).await?;
    let mut document: Document = serde_json::from_slice(&bytes).map_err(anyhow::Error::from)?;
    apply_review(&mut document, &read_review(&dir));
    let chunks = ask_chunks(
        &document,
        question,
        request.max_sources.unwrap_or(10).clamp(3, 20),
    );
    if chunks.is_empty() {
        return Ok(Json(json!({
            "configured": providers::llm_configured(),
            "question": question,
            "answer": null,
            "sources": [],
            "prompt": "",
            "message": "This document has no extracted text to answer from.",
        }))
        .into_response());
    }
    let mut sources = Vec::new();
    let mut prompt = format!(
        "Question about one local document: {question}\n\nUse only these sources. Cite each claim as [S1], [S2], and so on.\n\n"
    );
    let mut context_chars = 0usize;
    for (index, chunk) in chunks.iter().enumerate() {
        let id = format!("S{}", index + 1);
        prompt.push_str(&format!(
            "[{id}] {} · page {} · {}\n{}\n\n",
            chunk.unit_name, chunk.page, chunk.block_id, chunk.text
        ));
        context_chars += chunk.text.len();
        let preview: String = chunk
            .text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(240)
            .collect();
        sources.push(json!({
            "id": id,
            "unit_id": chunk.unit_id,
            "unit_name": chunk.unit_name,
            "block_id": chunk.block_id,
            "page": chunk.page,
            "preview": preview,
        }));
    }
    let configured = providers::llm_configured();
    let (answer, model, error_message) = if configured {
        let prompt_for_model = prompt.clone();
        match tokio::task::spawn_blocking(move || providers::llm_answer(&prompt_for_model)).await {
            Ok(Ok((answer, model))) => (Some(answer), Some(model), None),
            Ok(Err(e)) => (None, None, Some(format!("{e:#}"))),
            Err(e) => (None, None, Some(e.to_string())),
        }
    } else {
        (None, None, None)
    };
    let answered = answer.is_some();
    Ok(Json(json!({
        "configured": configured,
        "question": question,
        "answer": answer,
        "model": model,
        "error": error_message,
        "sources": sources,
        "prompt": prompt,
        "context_chars": context_chars,
        "message": if !configured {
            "No local text model is configured. Copy the prompt into a local model, or set DOC_LLM_URL."
        } else if !answered {
            "The local model could not answer. You can copy the prompt into the model directly."
        } else {
            "Answer generated locally from the extracted pages."
        }
    }))
    .into_response())
}

async fn result(
    State(app): State<App>,
    ApiPath(id): ApiPath<String>,
    headers: HeaderMap,
    Query(export): Query<Export>,
) -> ApiResult<Response> {
    app.authorize(&id, &headers)?;
    let default_format = job_result_format(&app, &id).await;
    let format = export
        .format
        .as_deref()
        .or(default_format.as_deref())
        .unwrap_or("json");
    // The staged upload is kept for retries and as the user's original copy. It is available
    // even while a job is queued or failed, which is exactly when "retry" needs it.
    if format == "review" {
        let path = app.dir(&id).join("review.json");
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return stream_file(path, "application/json").await;
        }
        return Ok(Json(empty_review()).into_response());
    }
    if format == "original" {
        let job = app.job(&id)?;
        let path = app.dir(&id).join("input");
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Err(error(
                StatusCode::NOT_FOUND,
                "not_found",
                "Original upload is no longer stored for this job",
            ));
        }
        let mut response = stream_file(path, mime_for_filename(&job.filename)).await?;
        let ascii_name: String = job
            .filename
            .chars()
            .map(|c| {
                if c.is_ascii() && c != '"' && c != '\\' && c != '\r' && c != '\n' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let disposition = format!("attachment; filename=\"{ascii_name}\"");
        response.headers_mut().insert(
            header::CONTENT_DISPOSITION,
            axum::http::HeaderValue::from_str(&disposition)
                .unwrap_or_else(|_| axum::http::HeaderValue::from_static("attachment")),
        );
        return Ok(response);
    }
    if app.job(&id)?.state != "completed" {
        return Err(error(
            StatusCode::CONFLICT,
            "not_ready",
            "Result is not complete; use unit endpoint for checkpoints",
        ));
    }
    if format == "redactions" {
        let dir = app.dir(&id);
        let bytes = tokio::fs::read(dir.join("result.json")).await?;
        let mut document: Document = serde_json::from_slice(&bytes).map_err(anyhow::Error::from)?;
        apply_review(&mut document, &read_review(&dir));
        let matches = detect_redactions(&document);
        let saved = read_redactions(&dir);
        let mut counts = serde_json::Map::new();
        for item in &matches {
            let count = counts
                .entry(item.category.clone())
                .or_insert_with(|| json!(0))
                .as_u64()
                .unwrap_or(0);
            counts.insert(item.category.clone(), json!(count + 1));
        }
        return Ok(Json(json!({
            "matches": matches,
            "saved": saved.get("redactions").cloned().unwrap_or_else(|| json!([])),
            "counts": counts,
            "limitations": "Pattern matching is best-effort. Review every match and inspect the final export before sharing."
        }))
        .into_response());
    }
    let review = read_review(&app.dir(&id));
    let redactions = if export.redact.unwrap_or(false) {
        redaction_specs(&read_redactions(&app.dir(&id)))
    } else {
        Vec::new()
    };
    if format == "report" {
        return report_export(&app.dir(&id), &review, &redactions).await;
    }
    if ["markdown", "md", "text", "txt", "html"].contains(&format)
        && ((export.review.unwrap_or(false) && review_has_corrections(&review))
            || !redactions.is_empty())
    {
        return review_export(&app.dir(&id), format, &review, &redactions).await;
    }
    let (file, mime) =
        match format {
            "json" => ("result.json", "application/json"),
            "markdown" | "md" => ("result.md", "text/markdown; charset=utf-8"),
            "text" | "txt" => ("result.txt", "text/plain; charset=utf-8"),
            "jsonl" | "ndjson" => ("result.jsonl", "application/x-ndjson; charset=utf-8"),
            "html" => ("result.html", "text/html; charset=utf-8"),
            "chunks" => ("result.chunks.jsonl", "application/x-ndjson; charset=utf-8"),
            _ => return Err(error(
                StatusCode::BAD_REQUEST,
                "invalid_format",
                "Expected json, markdown, text, jsonl, html, chunks, report, review or original",
            )),
        };
    let path = app.dir(&id).join(file);
    if tokio::fs::try_exists(&path).await.unwrap_or(false) {
        stream_file(path, mime).await
    } else {
        Err(error(
            StatusCode::NOT_FOUND,
            "not_found",
            "This representation is not stored for the job; re-run it to create the artifact",
        ))
    }
}
/// HMAC-SHA256 over the signed string, hex encoded (`X-DocSmith-Signature: sha256=...`).
fn hmac_sha256_hex(secret: &str, message: &[u8]) -> String {
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts any key length");
    mac.update(message);
    hex::encode(mac.finalize().into_bytes())
}
/// POST one delivery. Success is any 2xx; the body is the JSON payload and the signature
/// covers `"{timestamp}.{body}"` so replays with a different timestamp do not verify.
fn deliver_webhook(
    url: &str,
    job_id: &str,
    event: &str,
    delivery: &str,
    payload: &str,
    secret: Option<&str>,
) -> std::result::Result<(), String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let timestamp = epoch().to_string();
    let mut request = client
        .post(url)
        .header("content-type", "application/json")
        .header("x-docsmith-event", event)
        .header("x-docsmith-job-id", job_id)
        .header("x-docsmith-delivery", delivery)
        .header("x-docsmith-timestamp", &timestamp);
    if let Some(secret) = secret {
        request = request.header(
            "x-docsmith-signature",
            format!(
                "sha256={}",
                hmac_sha256_hex(secret, format!("{timestamp}.{payload}").as_bytes())
            ),
        );
    }
    let response = request
        .body(payload.to_owned())
        .send()
        .map_err(|e| e.to_string())?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("status {}", response.status()))
    }
}
/// http(s) only; loopback/private targets are refused unless the operator opts in, so a
/// token-authenticated client cannot use webhooks to probe the internal network.
fn validate_callback(url: &str, allow_private: bool) -> std::result::Result<(), &'static str> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .ok_or("Callback URL must start with http:// or https://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let after_user = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(rest) = after_user.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        after_user.split(':').next().unwrap_or("")
    };
    if host.is_empty() {
        return Err("Callback URL has no host");
    }
    if !allow_private && is_private_host(host) {
        return Err(
            "Callback URL points at a private address; set webhook_allow_private to allow it",
        );
    }
    Ok(())
}
fn is_private_host(host: &str) -> bool {
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") || lower == "::1" || lower == "0.0.0.0"
    {
        return true;
    }
    match lower.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => {
            v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified()
        }
        Ok(std::net::IpAddr::V6(v6)) => v6.is_loopback() || v6.is_unspecified(),
        Err(_) => false,
    }
}
/// Upload the job's result artifact (and optionally the zip archive) to S3 using static
/// credentials from config or the standard `AWS_*` environment variables.
fn upload_to_s3(
    config: &crate::s3::S3Config,
    job_id: &str,
    dir: &std::path::Path,
) -> Result<Vec<String>> {
    let access_key = config
        .access_key
        .clone()
        .or_else(|| std::env::var("AWS_ACCESS_KEY_ID").ok())
        .context("s3.access_key or AWS_ACCESS_KEY_ID is required")?;
    let secret_key = config
        .secret_key
        .clone()
        .or_else(|| std::env::var("AWS_SECRET_ACCESS_KEY").ok())
        .context("s3.secret_key or AWS_SECRET_ACCESS_KEY is required")?;
    let session_token = config
        .session_token
        .clone()
        .or_else(|| std::env::var("AWS_SESSION_TOKEN").ok());
    if config.bucket.trim().is_empty() {
        anyhow::bail!("s3.bucket is required");
    }
    let base = crate::s3::S3Upload {
        bucket: config.bucket.trim(),
        prefix: &config.prefix,
        region: if config.region.trim().is_empty() {
            "us-east-1"
        } else {
            config.region.trim()
        },
        endpoint: config.endpoint.as_deref(),
        path_style: config.path_style,
        access_key: &access_key,
        secret_key: &secret_key,
        session_token: session_token.as_deref(),
    };
    let prefix = normalized_prefix(&config.prefix);
    let (result, _format) = result_artifact(dir);
    let name = result
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "result.json".into());
    let mut uploaded = vec![crate::s3::put_object(
        &base,
        &format!("{prefix}{job_id}/{name}"),
        &result,
    )?];
    if config.upload_archive {
        let archive = build_archive(dir, false)?;
        uploaded.push(crate::s3::put_object(
            &base,
            &format!("{prefix}{job_id}/archive.zip"),
            &archive,
        )?);
    }
    Ok(uploaded)
}
fn normalized_prefix(prefix: &str) -> String {
    let trimmed = prefix.trim_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}/")
    }
}

/// Stored artifact matching the job's `result_format` (falls back to canonical JSON).
fn result_artifact(dir: &std::path::Path) -> (PathBuf, String) {
    let format = std::fs::read(dir.join("request.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|request| {
            request
                .get("options")?
                .get("result_format")?
                .as_str()
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "json".into());
    let file = match format.as_str() {
        "markdown" | "md" => "result.md",
        "text" | "txt" => "result.txt",
        "jsonl" | "ndjson" => "result.jsonl",
        _ => "result.json",
    };
    let path = dir.join(file);
    if path.is_file() {
        (path, format)
    } else {
        (dir.join("result.json"), "json".into())
    }
}
/// Expand placeholders, export DOC_* variables and run the sink command with the job
/// directory as cwd. Output is appended to `sink-<state>.log` for troubleshooting.
fn run_sink(
    command: &str,
    job_id: &str,
    state: &str,
    filename: &str,
    dir: &std::path::Path,
) -> std::result::Result<(), String> {
    let (result, format) = result_artifact(dir);
    let expanded = command
        .replace("{id}", job_id)
        .replace("{name}", filename)
        .replace("{state}", state)
        .replace("{dir}", &dir.display().to_string())
        .replace("{result}", &result.display().to_string())
        .replace("{format}", &format);
    let log_path = dir.join(format!("sink-{state}.log"));
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| e.to_string())?;
    let log_err = log.try_clone().map_err(|e| e.to_string())?;
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(&expanded)
        .current_dir(dir)
        .env("DOC_ID", job_id)
        .env("DOC_STATE", state)
        .env("DOC_FILENAME", filename)
        .env("DOC_DIR", dir)
        .env("DOC_RESULT", &result)
        .env("DOC_FORMAT", &format)
        .stdin(std::process::Stdio::null())
        .stdout(log)
        .stderr(log_err)
        .spawn()
        .map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + Duration::from_secs(300);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if std::time::Instant::now() > deadline {
                    let _ = child.kill();
                    return Err("sink command timed out after 300s".into());
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    if status.success() {
        Ok(())
    } else {
        let tail = std::fs::read_to_string(&log_path)
            .unwrap_or_default()
            .chars()
            .rev()
            .take(300)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>();
        Err(format!(
            "exit {}: {}",
            status.code().unwrap_or(-1),
            tail.trim()
        ))
    }
}
fn valid_part(part: &str, prefix: char) -> bool {
    part.starts_with(prefix)
        && part.len() > 1
        && part.len() <= 64
        && part.chars().all(|c| c.is_ascii_alphanumeric())
}
/// Render a single PDF page on demand (and cache it) so digital documents can show the
/// original page even when `page_images` was not requested for the whole job.
async fn render_unit_page(app: &App, id: &str, unit: &str, dpi: u32) -> ApiResult<Response> {
    let job = app.job(id)?;
    if !job.filename.to_ascii_lowercase().ends_with(".pdf") {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "unsupported_preview",
            "On-demand page previews are only available for PDF jobs",
        ));
    }
    if !providers::available("pdftoppm") {
        return Err(error(
            StatusCode::SERVICE_UNAVAILABLE,
            "preview_unavailable",
            "pdftoppm is not installed",
        ));
    }
    let dir = app.dir(id);
    let unit_path = dir.join("units").join(format!("{unit}.json"));
    let bytes = tokio::fs::read(&unit_path).await.map_err(|_| {
        error(
            StatusCode::NOT_FOUND,
            "not_found",
            "Unit checkpoint not found",
        )
    })?;
    let value: Value = serde_json::from_slice(&bytes).map_err(anyhow::Error::from)?;
    let page = value
        .get("index")
        .and_then(Value::as_u64)
        .map(|index| index + 1)
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "not_found", "Unit has no page index"))?
        as u32;
    let preview_dir = dir.join("preview");
    let final_path = preview_dir.join(format!("{unit}-{dpi}.png"));
    if tokio::fs::try_exists(&final_path).await.unwrap_or(false) {
        return stream_file(final_path, "image/png").await;
    }
    let input = dir.join("input");
    if !tokio::fs::try_exists(&input).await.unwrap_or(false) {
        return Err(error(
            StatusCode::NOT_FOUND,
            "not_found",
            "Original PDF is no longer stored",
        ));
    }
    let max_pixels = app.config.limits.page_pixels;
    let temp_prefix = preview_dir.join(format!(".{unit}-{dpi}-{}", uuid::Uuid::new_v4()));
    let render_prefix = temp_prefix.clone();
    let target = final_path.clone();
    let page_arg = page.to_string();
    let dpi_arg = dpi.to_string();
    let outcome = tokio::task::spawn_blocking(move || -> std::result::Result<(), String> {
        std::fs::create_dir_all(&preview_dir).map_err(|e| e.to_string())?;
        let output = std::process::Command::new("pdftoppm")
            .args([
                "-f",
                &page_arg,
                "-l",
                &page_arg,
                "-r",
                &dpi_arg,
                "-png",
                "-singlefile",
            ])
            .arg(&input)
            .arg(&render_prefix)
            .output()
            .map_err(|e| e.to_string())?;
        let rendered = temp_prefix.with_extension("png");
        if !output.status.success() {
            let _ = std::fs::remove_file(&rendered);
            return Err(String::from_utf8_lossy(&output.stderr)
                .trim()
                .chars()
                .take(300)
                .collect());
        }
        let (width, height) = image::image_dimensions(&rendered).map_err(|e| e.to_string())?;
        if u64::from(width).saturating_mul(u64::from(height)) > max_pixels {
            let _ = std::fs::remove_file(&rendered);
            return Err(format!(
                "limit_exceeded: rendered preview is {width}x{height} pixels"
            ));
        }
        std::fs::rename(&rendered, &target).map_err(|e| e.to_string())
    })
    .await
    .map_err(anyhow::Error::from)?;
    match outcome {
        Ok(()) => stream_file(final_path, "image/png").await,
        Err(message) if message.starts_with("limit_exceeded") => Err(error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "limit_exceeded",
            &message,
        )),
        Err(message) => Err(error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "preview_failed",
            &message,
        )),
    }
}

#[derive(Deserialize)]
struct UnitQuery {
    from: Option<usize>,
    count: Option<usize>,
    /// Render one PDF page on demand and cache it under `preview/`.
    render: Option<bool>,
    dpi: Option<u32>,
}
async fn unit(
    State(app): State<App>,
    ApiPath((id, unit)): ApiPath<(String, String)>,
    headers: HeaderMap,
    Query(query): Query<UnitQuery>,
) -> ApiResult<Response> {
    app.authorize(&id, &headers)?;
    if !valid_part(&unit, 'u') {
        return Err(error(StatusCode::NOT_FOUND, "not_found", "Unit not found"));
    }
    if query.render.unwrap_or(false) {
        let dpi = query.dpi.unwrap_or(150).clamp(72, 300);
        return render_unit_page(&app, &id, &unit, dpi).await;
    }
    let path = app.dir(&id).join("units").join(format!("{unit}.json"));
    if query.from.is_none() && query.count.is_none() {
        return stream_file(path, "application/json")
            .await
            .map_err(|_| error(StatusCode::NOT_FOUND, "not_found", "Unit not found"));
    }
    let from = query.from.unwrap_or(0);
    let count = query.count.unwrap_or(500).clamp(1, 10_000);
    let sliced = tokio::task::spawn_blocking(move || -> Result<Value> {
        let unit: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
        Ok(slice_unit(unit, from, count))
    })
    .await
    .map_err(anyhow::Error::from)?
    .map_err(|_| error(StatusCode::NOT_FOUND, "not_found", "Unit not found"))?;
    Ok(Json(sliced).into_response())
}
/// `{"slice": {...}, "unit": {...}}` with table rows limited to `from..from+count`.
/// Original row indices are preserved so returned cells stay addressable in the full unit.
fn slice_unit(mut unit: Value, from: usize, count: usize) -> Value {
    let mut rows_returned = 0usize;
    if let Some(blocks) = unit["blocks"].as_array_mut() {
        for block in blocks {
            let Some(table) = block.get_mut("table") else {
                continue;
            };
            let total = table["rows"].as_u64().unwrap_or(0) as usize;
            let start = from.min(total);
            let end = (start + count).min(total);
            rows_returned = rows_returned.max(end - start);
            if let Some(cells) = table["cells"].as_array_mut() {
                cells.retain(|cell| {
                    let row = cell["row"].as_u64().unwrap_or(0) as usize;
                    row >= start && row < end
                });
            }
            table["rows"] = json!(end - start);
        }
    }
    json!({"slice": {"from": from, "count": count, "rows": rows_returned}, "unit": unit})
}
async fn asset(
    State(app): State<App>,
    ApiPath((id, asset)): ApiPath<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    app.authorize(&id, &headers)?;
    if !valid_part(&asset, 'a') {
        return Err(error(StatusCode::NOT_FOUND, "not_found", "Asset not found"));
    }
    let manifest = tokio::fs::read(app.dir(&id).join("assets.json"))
        .await
        .map_err(|_| error(StatusCode::NOT_FOUND, "not_found", "Asset not found"))?;
    let assets: Vec<Asset> = serde_json::from_slice(&manifest).map_err(anyhow::Error::from)?;
    let found = assets
        .into_iter()
        .find(|a| a.id == asset)
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "not_found", "Asset not found"))?;
    let mut response = stream_file(app.dir(&id).join("assets").join(asset), &found.mime).await?;
    response.headers_mut().insert(
        axum::http::header::CONTENT_DISPOSITION,
        axum::http::HeaderValue::from_static("attachment"),
    );
    Ok(response)
}
async fn cancel(
    State(app): State<App>,
    ApiPath(id): ApiPath<String>,
    headers: HeaderMap,
) -> ApiResult<Json<Job>> {
    app.authorize(&id, &headers)?;
    cancel_job(&app, &id)?;
    Ok(Json(app.job(&id)?))
}
fn cancel_job(app: &App, id: &str) -> ApiResult<()> {
    let running = app.running.lock().unwrap();
    app.db.lock().unwrap().execute("UPDATE jobs SET state='cancelled',updated_at=?2 WHERE id=?1 AND state IN ('queued','running')",params![id,epoch()])?;
    if let Some(pid) = running.get(id) {
        kill_group(*pid);
    }
    Ok(())
}
const WEB_INDEX: &str = include_str!("../assets/web/index.html");
const WEB_CSS: &str = include_str!("../assets/web/app.css");
const WEB_JS: &str = include_str!("../assets/web/app.js");
const WEB_WASM_GZ: &[u8] = include_bytes!("../assets/web/doc_extractor.wasm.gz");

async fn landing() -> Html<&'static str> {
    Html(WEB_INDEX)
}

async fn web_css() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        WEB_CSS,
    )
        .into_response()
}

async fn web_js() -> Response {
    (
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        WEB_JS,
    )
        .into_response()
}

fn wasm_binary() -> &'static [u8] {
    static WASM: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    WASM.get_or_init(|| {
        use std::io::Read;
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(WEB_WASM_GZ)
            .read_to_end(&mut out)
            .expect("embedded doc_extractor.wasm.gz is valid gzip");
        out
    })
}

async fn web_wasm() -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/wasm"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        wasm_binary(),
    )
        .into_response()
}
#[derive(Deserialize)]
struct ListQuery {
    state: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
    /// Optional search query over filenames, unit names and block text of completed jobs.
    q: Option<String>,
}
async fn list(
    State(app): State<App>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    let tenant = app.tenant(&headers)?;
    let limit = query.limit.unwrap_or(50).min(200) as i64;
    let offset = query.offset.unwrap_or(0) as i64;
    let q = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| text.chars().take(200).collect::<String>());
    if q.is_some() {
        // Old archives predate the index. Index a bounded batch before answering so the first
        // search is still useful without making every query walk every result.json.
        let app = app.clone();
        let tenant = tenant.clone();
        let _ =
            tokio::task::spawn_blocking(move || app.index_pending_for_search(&tenant, 40)).await;
    }
    let (ids, total, matches) = if let Some(q) = &q {
        let (ids, total) = {
            let db = app.db.lock().unwrap();
            let total: i64 = db.query_row(
                "SELECT count(*) FROM jobs j WHERE j.tenant=?1 AND (?2 IS NULL OR j.state=?2) AND (instr(lower(j.filename),lower(?3))>0 OR instr(lower(COALESCE(j.display_name,'')),lower(?3))>0 OR EXISTS(SELECT 1 FROM search_units s WHERE s.job_id=j.id AND instr(lower(s.text),lower(?3))>0))",
                params![tenant, query.state, q],
                |r| r.get(0),
            )?;
            let mut statement = db.prepare(
                "SELECT j.id FROM jobs j WHERE j.tenant=?1 AND (?2 IS NULL OR j.state=?2) AND (instr(lower(j.filename),lower(?3))>0 OR instr(lower(COALESCE(j.display_name,'')),lower(?3))>0 OR EXISTS(SELECT 1 FROM search_units s WHERE s.job_id=j.id AND instr(lower(s.text),lower(?3))>0)) ORDER BY j.created_at DESC LIMIT ?4 OFFSET ?5",
            )?;
            let ids = statement
                .query_map(params![tenant, query.state, q, limit, offset], |r| {
                    r.get::<_, String>(0)
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            (ids, total)
        };
        let mut matches = Vec::new();
        {
            let db = app.db.lock().unwrap();
            let mut statement = db.prepare(
                "SELECT s.job_id,j.filename,j.display_name,s.unit_id,s.unit_name,s.unit_index,s.block_id,s.kind,instr(lower(s.text),lower(?3)),s.text FROM search_units s JOIN jobs j ON j.id=s.job_id WHERE j.tenant=?1 AND (?2 IS NULL OR j.state=?2) AND instr(lower(s.text),lower(?3))>0 ORDER BY j.created_at DESC,s.unit_index ASC,s.block_id ASC LIMIT 120",
            )?;
            let mut rows = statement.query(params![tenant, query.state, q])?;
            let mut seen: HashSet<(String, String, Option<String>)> = HashSet::new();
            while let Some(row) = rows.next()? {
                let job_id: String = row.get(0)?;
                let filename: String = row.get(1)?;
                let display_name: Option<String> = row.get(2)?;
                let unit_id: String = row.get(3)?;
                let unit_name: String = row.get(4)?;
                let unit_index: i64 = row.get(5)?;
                let block_id: Option<String> = row.get(6)?;
                let kind: String = row.get(7)?;
                let position: i64 = row.get(8)?;
                let text: String = row.get(9)?;
                if !seen.insert((job_id.clone(), unit_id.clone(), block_id.clone())) {
                    continue;
                }
                matches.push(json!({
                    "job_id": job_id,
                    "filename": filename,
                    "display_name": display_name,
                    "unit_id": unit_id,
                    "unit_name": unit_name,
                    "page": unit_index + 1,
                    "block_id": block_id,
                    "kind": kind,
                    "snippet": search_snippet(&text, position.max(1) as usize),
                }));
            }
        }
        (ids, total, matches)
    } else {
        let (ids, total) = {
            let db = app.db.lock().unwrap();
            let total: i64 = db.query_row(
                "SELECT count(*) FROM jobs WHERE tenant=?1 AND (?2 IS NULL OR state=?2)",
                params![tenant, query.state],
                |r| r.get(0),
            )?;
            let mut statement = db.prepare(
                "SELECT id FROM jobs WHERE tenant=?1 AND (?2 IS NULL OR state=?2) ORDER BY created_at DESC LIMIT ?3 OFFSET ?4",
            )?;
            let ids = statement
                .query_map(params![tenant, query.state, limit, offset], |r| {
                    r.get::<_, String>(0)
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            (ids, total)
        };
        (ids, total, Vec::new())
    };
    let jobs = ids
        .iter()
        .filter_map(|id| app.job(id).ok())
        .collect::<Vec<_>>();
    Ok(Json(json!({
        "jobs": jobs,
        "total": total,
        "limit": limit,
        "offset": offset,
        "q": q,
        "matches": matches,
    })))
}
async fn stats(State(app): State<App>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let tenant = app.tenant(&headers)?;
    let (by_state, webhooks_pending, sinks_pending) = {
        let db = app.db.lock().unwrap();
        let mut by_state = serde_json::Map::new();
        let mut statement =
            db.prepare("SELECT state,count(*) FROM jobs WHERE tenant=?1 GROUP BY state")?;
        for row in statement.query_map([&tenant], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })? {
            let (state, count) = row?;
            by_state.insert(state, json!(count));
        }
        let webhooks_pending: i64 = db
            .query_row(
                "SELECT count(*) FROM webhook_deliveries d JOIN jobs j ON j.id=d.job_id WHERE j.tenant=?1 AND d.delivered_at IS NULL AND d.attempts < 5",
                [&tenant],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let sinks_pending: i64 = db
            .query_row(
                "SELECT count(*) FROM sink_runs s JOIN jobs j ON j.id=s.job_id WHERE j.tenant=?1 AND s.completed_at IS NULL AND s.attempts < 3",
                [&tenant],
                |r| r.get(0),
            )
            .unwrap_or(0);
        (by_state, webhooks_pending, sinks_pending)
    };
    let dir = app.config.data_dir.join("jobs");
    let disk_bytes = tokio::task::spawn_blocking(move || dir_bytes(&dir))
        .await
        .unwrap_or(0);
    Ok(Json(json!({
        "tenant": tenant,
        "uptime_seconds": epoch() - app.started_at,
        "jobs": by_state,
        "workers": {"slots": app.config.limits.workers, "active": app.running.lock().unwrap().len()},
        "webhooks_pending": webhooks_pending,
        "sinks_pending": sinks_pending,
        "disk_bytes": disk_bytes,
        "data_dir": app.config.data_dir,
        "retention_seconds": app.config.limits.retention_seconds,
    })))
}
/// Prometheus text exposition (gauges, instance-wide, no document content).
async fn metrics(State(app): State<App>) -> ApiResult<Response> {
    let (by_state, webhooks_pending, sinks_pending) = {
        let db = app.db.lock().unwrap();
        let mut by_state: Vec<(String, i64)> = Vec::new();
        let mut statement = db.prepare("SELECT state,count(*) FROM jobs GROUP BY state")?;
        for row in statement.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
            by_state.push(row?);
        }
        let webhooks_pending: i64 = db
            .query_row(
                "SELECT count(*) FROM webhook_deliveries WHERE delivered_at IS NULL AND attempts < 5",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let sinks_pending: i64 = db
            .query_row(
                "SELECT count(*) FROM sink_runs WHERE completed_at IS NULL AND attempts < 3",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        (by_state, webhooks_pending, sinks_pending)
    };
    let dir = app.config.data_dir.join("jobs");
    let disk_bytes = tokio::task::spawn_blocking(move || dir_bytes(&dir))
        .await
        .unwrap_or(0);
    let mut body = String::from("# HELP doc_jobs Current jobs by state\n# TYPE doc_jobs gauge\n");
    for (state, count) in by_state {
        body.push_str(&format!("doc_jobs{{state=\"{state}\"}} {count}\n"));
    }
    body.push_str("# TYPE doc_workers_active gauge\n");
    body.push_str(&format!(
        "doc_workers_active {}\n",
        app.running.lock().unwrap().len()
    ));
    body.push_str("# TYPE doc_workers_slots gauge\n");
    body.push_str(&format!(
        "doc_workers_slots {}\n",
        app.config.limits.workers
    ));
    body.push_str("# TYPE doc_webhooks_pending gauge\n");
    body.push_str(&format!("doc_webhooks_pending {webhooks_pending}\n"));
    body.push_str("# TYPE doc_sinks_pending gauge\n");
    body.push_str(&format!("doc_sinks_pending {sinks_pending}\n"));
    body.push_str("# TYPE doc_disk_bytes gauge\n");
    body.push_str(&format!("doc_disk_bytes {disk_bytes}\n"));
    body.push_str("# TYPE doc_uptime_seconds gauge\n");
    body.push_str(&format!(
        "doc_uptime_seconds {}\n",
        epoch() - app.started_at
    ));
    Ok((
        [(
            "content-type",
            "text/plain; version=0.0.4; charset=utf-8".to_owned(),
        )],
        body,
    )
        .into_response())
}
/// Bounded recursive size so `/v1/stats` cannot walk a pathological tree forever.
fn dir_bytes(path: &std::path::Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    let mut budget = 100_000usize;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            budget = budget.saturating_sub(1);
            if budget == 0 {
                return total;
            }
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => stack.push(entry.path()),
                Ok(kind) if kind.is_file() => {
                    total += entry.metadata().map(|m| m.len()).unwrap_or(0);
                }
                _ => {}
            }
        }
    }
    total
}
/// Server-sent events: one `job` event per change until the job reaches a terminal state.
async fn events(
    State(app): State<App>,
    ApiPath(id): ApiPath<String>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    app.authorize(&id, &headers)?;
    app.job(&id)?;
    let (sender, receiver) =
        tokio::sync::mpsc::channel::<std::result::Result<Event, std::convert::Infallible>>(8);
    let worker = app.clone();
    tokio::spawn(async move {
        let mut last = String::new();
        loop {
            let Ok(job) = worker.job(&id) else { break };
            let body = serde_json::to_string(&job).unwrap_or_default();
            if body != last {
                last = body.clone();
                if sender
                    .send(Ok(Event::default().event("job").data(body)))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            if ["completed", "failed", "cancelled"].contains(&job.state.as_str()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    });
    Ok(Sse::new(ReceiverStream::new(receiver))
        .keep_alive(KeepAlive::default())
        .into_response())
}
#[derive(Deserialize)]
struct ArchiveQuery {
    units: Option<bool>,
    /// Comma-separated job ids for a multi-document export packet. The path id remains the
    /// authorization anchor and must belong to the caller.
    jobs: Option<String>,
}
async fn view() -> Html<&'static str> {
    Html(WEB_INDEX)
}
async fn archive(
    State(app): State<App>,
    ApiPath(id): ApiPath<String>,
    headers: HeaderMap,
    Query(query): Query<ArchiveQuery>,
) -> ApiResult<Response> {
    app.authorize(&id, &headers)?;
    let include_units = query.units.unwrap_or(false);
    if let Some(raw) = query
        .jobs
        .as_deref()
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
    {
        let mut ids: Vec<String> = raw
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect();
        ids.sort();
        ids.dedup();
        if ids.len() > 50 {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "invalid_bundle",
                "A bundle can contain at most 50 jobs",
            ));
        }
        let mut jobs = Vec::with_capacity(ids.len());
        for job_id in ids {
            app.authorize(&job_id, &headers)?;
            let job = app.job(&job_id)?;
            if job.state != "completed" {
                return Err(error(
                    StatusCode::CONFLICT,
                    "not_ready",
                    "Every bundled job must have completed",
                ));
            }
            jobs.push(job);
        }
        let data_dir = app.config.data_dir.clone();
        let count = jobs.len();
        let path =
            tokio::task::spawn_blocking(move || build_bundle(&data_dir, &jobs, include_units))
                .await
                .map_err(anyhow::Error::from)??;
        let mut response = stream_file(path, "application/zip").await?;
        response.headers_mut().insert(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"library-{count}-documents.zip\"")
                .parse()
                .expect("static header"),
        );
        return Ok(response);
    }
    if app.job(&id)?.state != "completed" {
        return Err(error(
            StatusCode::CONFLICT,
            "not_ready",
            "Archive requires a completed job",
        ));
    }
    let dir = app.dir(&id);
    let path = tokio::task::spawn_blocking(move || build_archive(&dir, include_units))
        .await
        .map_err(anyhow::Error::from)??;
    let mut response = stream_file(path, "application/zip").await?;
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{id}.zip\"")
            .parse()
            .expect("static header"),
    );
    Ok(response)
}
type ArchiveZip = zip::ZipWriter<std::io::BufWriter<std::fs::File>>;

/// Add one job's artifacts to an open zip under `prefix` (empty for a single-job archive).
fn add_archive_entries(
    zip: &mut ArchiveZip,
    dir: &std::path::Path,
    prefix: &str,
    include_units: bool,
) -> Result<()> {
    use std::io::Write;
    let options = zip::write::SimpleFileOptions::default();
    for name in [
        "result.json",
        "result.md",
        "result.txt",
        "result.jsonl",
        "request.json",
        "identity.json",
        "timings.json",
        "progress.json",
        "assets.json",
        "review.json",
        "redaction.json",
    ] {
        let source = dir.join(name);
        if source.is_file() {
            zip.start_file(format!("{prefix}{name}"), options)?;
            std::io::copy(&mut std::fs::File::open(&source)?, zip)?;
        }
    }
    // Keep the original upload in the archive under a readable name. The staged `input` file
    // has no extension, so recover the filename from request.json.
    if let Ok(bytes) = std::fs::read(dir.join("request.json")) {
        if let Ok(request) = serde_json::from_slice::<Value>(&bytes) {
            if let Some(name) = request.get("filename").and_then(Value::as_str) {
                let safe = std::path::Path::new(name)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("original");
                let input = dir.join("input");
                if input.is_file() {
                    zip.start_file(format!("{prefix}original/{safe}"), options)?;
                    std::io::copy(&mut std::fs::File::open(&input)?, zip)?;
                }
            }
        }
    }
    let review = read_review(dir);
    if review_has_corrections(&review) {
        if let Ok(bytes) = std::fs::read(dir.join("result.json")) {
            if let Ok(mut document) = serde_json::from_slice::<Document>(&bytes) {
                apply_review(&mut document, &review);
                zip.start_file(format!("{prefix}reviewed/result.md"), options)?;
                zip.write_all(crate::model::markdown(&document).as_bytes())?;
                zip.start_file(format!("{prefix}reviewed/result.txt"), options)?;
                zip.write_all(crate::model::plaintext(&document).as_bytes())?;
                zip.start_file(format!("{prefix}reviewed/result.html"), options)?;
                zip.write_all(reviewed_html(&document).as_bytes())?;
            }
        }
    }
    let redactions = redaction_specs(&read_redactions(dir));
    if !redactions.is_empty() {
        if let Ok(bytes) = std::fs::read(dir.join("result.json")) {
            if let Ok(mut document) = serde_json::from_slice::<Document>(&bytes) {
                apply_review(&mut document, &review);
                apply_redactions(&mut document, &redactions);
                zip.start_file(format!("{prefix}redacted/result.md"), options)?;
                zip.write_all(crate::model::markdown(&document).as_bytes())?;
                zip.start_file(format!("{prefix}redacted/result.txt"), options)?;
                zip.write_all(crate::model::plaintext(&document).as_bytes())?;
                zip.start_file(format!("{prefix}redacted/result.html"), options)?;
                zip.write_all(reviewed_html(&document).as_bytes())?;
            }
        }
    }
    for (folder, sub) in [("assets", "assets"), ("pages", "pages"), ("units", "units")] {
        if folder == "units" && !include_units {
            continue;
        }
        let source = dir.join(folder);
        if !source.is_dir() {
            continue;
        }
        let mut names = std::fs::read_dir(&source)?
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|k| k.is_file()))
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect::<Vec<_>>();
        names.sort();
        for name in names {
            zip.start_file(format!("{prefix}{sub}/{name}"), options)?;
            std::io::copy(&mut std::fs::File::open(source.join(&name))?, zip)?;
        }
    }
    Ok(())
}

/// Zip the result artifacts (and optionally unit checkpoints) into a temp file inside the
/// job directory; the file streams back and is replaced on the next archive request.
fn build_archive(dir: &std::path::Path, include_units: bool) -> Result<PathBuf> {
    use std::io::{BufWriter, Write};
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("archive.zip.tmp-") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    let path = dir.join(format!("archive.zip.tmp-{}", uuid::Uuid::new_v4()));
    let mut zip = zip::ZipWriter::new(BufWriter::new(std::fs::File::create(&path)?));
    add_archive_entries(&mut zip, dir, "", include_units)?;
    let mut writer = zip.finish()?;
    writer.flush()?;
    Ok(path)
}

/// Bundle several completed jobs into one zip with a manifest and per-job folders.
fn build_bundle(data_dir: &std::path::Path, jobs: &[Job], include_units: bool) -> Result<PathBuf> {
    use std::io::{BufWriter, Write};
    if let Ok(entries) = std::fs::read_dir(data_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("bundle.zip.tmp-") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    let path = data_dir.join(format!("bundle.zip.tmp-{}", uuid::Uuid::new_v4()));
    let mut zip = zip::ZipWriter::new(BufWriter::new(std::fs::File::create(&path)?));
    let options = zip::write::SimpleFileOptions::default();
    zip.start_file("manifest.json", options)?;
    zip.write_all(
        serde_json::to_string_pretty(&json!({
            "schema_version": "bundle1",
            "created_at": epoch(),
            "count": jobs.len(),
            "jobs": jobs,
        }))?
        .as_bytes(),
    )?;
    for job in jobs {
        add_archive_entries(
            &mut zip,
            &data_dir.join("jobs").join(&job.id),
            &format!("jobs/{}/", job.id),
            include_units,
        )?;
    }
    let mut writer = zip.finish()?;
    writer.flush()?;
    Ok(path)
}
async fn delete(
    State(app): State<App>,
    ApiPath(id): ApiPath<String>,
    headers: HeaderMap,
) -> ApiResult<StatusCode> {
    app.authorize(&id, &headers)?;
    cancel_job(&app, &id)?;
    app.db
        .lock()
        .unwrap()
        .execute("UPDATE jobs SET state='deleting' WHERE id=?1", [&id])?;
    for _ in 0..100 {
        if !app.running.lock().unwrap().contains_key(&id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    if app.running.lock().unwrap().contains_key(&id) {
        return Err(error(
            StatusCode::CONFLICT,
            "worker_stopping",
            "Worker is stopping; retry deletion",
        ));
    }
    if app.dir(&id).exists() {
        tokio::fs::remove_dir_all(app.dir(&id)).await?;
    }
    {
        let db = app.db.lock().unwrap();
        db.execute("DELETE FROM search_units WHERE job_id=?1", [&id])?;
        db.execute("DELETE FROM search_state WHERE job_id=?1", [&id])?;
        db.execute("DELETE FROM jobs WHERE id=?1", [&id])?;
    }
    Ok(StatusCode::NO_CONTENT)
}
pub fn capabilities() -> Value {
    json!({"engine":ENGINE_VERSION,"xberg":{"configured":std::env::var("DOC_XBERG").is_ok(),"required_version":"1.3.8","mapping":"partial"},"native_formats":["txt","md","json","xml","html","csv","tsv","xlsx","xls","xlsm","xlsb","ods","docx","pptx","odt","odp","epub","eml","rtf","drawio","svg","mermaid","dot","plantuml","dbml","doctags","vsdx"],"pdf":{"available":providers::available("pdfinfo")&&providers::available("pdftotext")&&providers::available("pdfimages"),"backend":"poppler","tables":"unvalidated","page_geometry":true,"page_images":{"available":providers::available("pdftoppm"),"dpi":150},"vector_source_preserved":true},"ocr":{"tesseract":providers::available("tesseract"),"languages":providers::languages(),"apple_vision":cfg!(target_os="macos")&&std::env::var("DOC_APPLE_VISION").is_ok(),"smoldocling":std::env::var("DOC_SMOLDOCLING_URL").is_ok()||std::env::var("DOC_VLM_URL").is_ok(),"paddle_onnx":false},"vision":{"configured":std::env::var("DOC_VLM_URL").is_ok(),"model":std::env::var("DOC_VLM_MODEL").ok(),"automatic_downloads":false},"llm":{"configured":providers::llm_configured(),"model":std::env::var("DOC_LLM_MODEL").or_else(|_| std::env::var("DOC_VLM_MODEL")).ok(),"endpoint":"local-only"},"offline_default":true,"limitations":["Logical Word/ODT sections are not physical pages","Legacy DOC/PPT/VSD are unsupported","Visio/SmartArt source preserved; semantics unsupported","Text diagram parsers implement documented subsets","Spreadsheet formulas are not recalculated"]})
}
pub fn openapi() -> Value {
    let doc_schema = serde_json::to_value(schemars::schema_for!(Document)).unwrap();
    let options_schema = serde_json::to_value(schemars::schema_for!(Options)).unwrap();
    let job_schema = serde_json::to_value(schemars::schema_for!(Job)).unwrap();
    let mut paths = serde_json::Map::new();
    for (path, method, summary, code, schema) in [
        (
            "/v1/extractions",
            "get",
            "List jobs for the caller",
            "200",
            json!({
                "type": "object",
                "required": ["jobs", "total"],
                "properties": {
                    "jobs": {"type": "array", "items": job_schema.clone()},
                    "total": {"type": "integer"},
                    "limit": {"type": "integer"},
                    "offset": {"type": "integer"},
                    "q": {"type": ["string", "null"]},
                    "matches": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "job_id": {"type": "string"},
                                "filename": {"type": "string"},
                                "display_name": {"type": ["string", "null"]},
                                "unit_id": {"type": "string"},
                                "unit_name": {"type": "string"},
                                "page": {"type": "integer"},
                                "block_id": {"type": ["string", "null"]},
                                "kind": {"type": "string"},
                                "snippet": {"type": "string"}
                            }
                        }
                    }
                }
            }),
        ),
        (
            "/v1/extractions",
            "post",
            "Upload one file and JSON options",
            "202",
            json!({"type":"object","required":["id","cached"],"properties":{"id":{"type":"string"},"cached":{"type":"boolean"}}}),
        ),
        (
            "/v1/stats",
            "get",
            "Queue, job and disk statistics",
            "200",
            json!({"type":"object"}),
        ),
        (
            "/v1/extractions/{id}/events",
            "get",
            "Server-sent job events until terminal",
            "200",
            json!({"type":"string"}),
        ),
        (
            "/v1/extractions/{id}/archive",
            "get",
            "Zip archive of result artifacts",
            "200",
            json!({"type":"string","format":"binary"}),
        ),
        (
            "/v1/extractions/{id}",
            "get",
            "Job state and checkpoint progress",
            "200",
            job_schema,
        ),
        (
            "/v1/extractions/{id}/result",
            "get",
            "Canonical JSON or derived export",
            "200",
            doc_schema,
        ),
        (
            "/v1/extractions/{id}/result",
            "post",
            "Ask a grounded question over extracted pages (optional local model)",
            "200",
            json!({"type":"object","properties":{"configured":{"type":"boolean"},"question":{"type":"string"},"answer":{"type":["string","null"]},"model":{"type":["string","null"]},"error":{"type":["string","null"]},"prompt":{"type":"string"},"context_chars":{"type":"integer"},"sources":{"type":"array","items":{"type":"object"}}}}),
        ),
        (
            "/v1/extractions/{id}/units/{unit_id}",
            "get",
            "Completed unit checkpoint",
            "200",
            serde_json::to_value(schemars::schema_for!(Unit)).unwrap(),
        ),
        (
            "/v1/extractions/{id}/assets/{asset_id}",
            "get",
            "Original asset bytes",
            "200",
            json!({"type":"string","format":"binary"}),
        ),
        (
            "/v1/extractions/{id}/cancel",
            "post",
            "Cancel queued or running job",
            "200",
            serde_json::to_value(schemars::schema_for!(Job)).unwrap(),
        ),
        (
            "/v1/extractions/{id}",
            "delete",
            "Cancel then delete retained data",
            "204",
            json!({}),
        ),
        (
            "/v1/extractions/{id}",
            "patch",
            "Save a reviewer correction or page verification",
            "200",
            json!({"type":"object","description":"The updated review sidecar"}),
        ),
        (
            "/v1/capabilities",
            "get",
            "Runtime configured capabilities",
            "200",
            json!({"type":"object"}),
        ),
        (
            "/health/live",
            "get",
            "Process liveness",
            "200",
            json!({"type":"object"}),
        ),
        (
            "/health/ready",
            "get",
            "Database readiness",
            "200",
            json!({"type":"object"}),
        ),
    ] {
        let mut params = vec![];
        for parameter in ["id", "unit_id", "asset_id"] {
            if path.contains(&format!("{{{parameter}}}")) {
                params.push(json!({"name":parameter,"in":"path","required":true,"schema":{"type":"string"}}));
            }
        }
        if path.ends_with("/result") {
            params.push(json!({"name":"format","in":"query","description":"Export format; `original` returns the uploaded file, `review` returns the correction sidecar, other values default to the job's result_format option","schema":{"type":"string","enum":["json","markdown","text","jsonl","html","chunks","report","review","redactions","original"]}}));
            params.push(json!({"name":"review","in":"query","description":"Apply reviewer text corrections to markdown, text or HTML","schema":{"type":"boolean"}}));
            params.push(json!({"name":"redact","in":"query","description":"Apply saved redactions to markdown, text, HTML or the report","schema":{"type":"boolean"}}));
        }
        if path.ends_with("/units/{unit_id}") {
            params.push(json!({"name":"from","in":"query","description":"First table row of the slice (used with count)","schema":{"type":"integer","minimum":0}}));
            params.push(json!({"name":"count","in":"query","description":"Maximum table rows in the slice (default 500, max 10000)","schema":{"type":"integer","minimum":1,"maximum":10000}}));
            params.push(json!({"name":"render","in":"query","description":"Render this PDF page to a cached PNG preview","schema":{"type":"boolean"}}));
            params.push(json!({"name":"dpi","in":"query","description":"Preview render resolution (default 150, 72..300)","schema":{"type":"integer","minimum":72,"maximum":300}}));
        }
        if path.ends_with("/archive") {
            params.push(json!({"name":"units","in":"query","description":"Include unit checkpoints in the archive","schema":{"type":"boolean"}}));
            params.push(json!({"name":"jobs","in":"query","description":"Comma-separated completed job ids for a multi-document bundle with manifest.json","schema":{"type":"string"}}));
        }
        if path == "/v1/extractions/{id}" && method == "patch" {
            params.push(json!({"name":"library","in":"query","description":"Update display name, collection, tags and favorite instead of review data","schema":{"type":"boolean"}}));
            params.push(json!({"name":"redaction","in":"query","description":"Replace saved redaction spans instead of review data","schema":{"type":"boolean"}}));
        }
        if path == "/v1/extractions" && method == "get" {
            params.push(json!({"name":"state","in":"query","schema":{"type":"string","enum":["queued","running","completed","failed","cancelled"]}}));
            params.push(json!({"name":"q","in":"query","description":"Search filenames, unit names and extracted text; completed results are indexed on first use","schema":{"type":"string","maxLength":200}}));
            params.push(json!({"name":"limit","in":"query","schema":{"type":"integer","minimum":1,"maximum":200}}));
            params.push(
                json!({"name":"offset","in":"query","schema":{"type":"integer","minimum":0}}),
            );
        }
        let mut responses =
            json!({code:{"description":summary,"content":{"application/json":{"schema":schema}}}});
        for (code, description) in [
            ("400", "Invalid request/options"),
            ("401", "Authentication required"),
            ("404", "Resource not found"),
            ("409", "Not ready or worker stopping"),
            ("413", "Upload limit exceeded"),
            ("429", "Queue full"),
            ("500", "Internal failure"),
        ] {
            responses[code] = json!({"description":description,"content":{"application/json":{"schema":{"type":"object","properties":{"error":{"$ref":"#/components/schemas/Failure"}}}}}});
        }
        if path.contains("/assets/") {
            responses[code]["content"] =
                json!({"application/octet-stream":{"schema":{"type":"string","format":"binary"}}});
        }
        if path.ends_with("/result") {
            responses[code]["content"]["text/markdown"] = json!({"schema":{"type":"string"}});
            responses[code]["content"]["text/plain"] = json!({"schema":{"type":"string"}});
            responses[code]["content"]["application/x-ndjson"] =
                json!({"schema":{"type":"string"}});
        }
        if path.ends_with("/events") {
            responses[code]["content"] = json!({"text/event-stream":{"schema":{"type":"string"}}});
        }
        if path.ends_with("/archive") {
            responses[code]["content"] =
                json!({"application/zip":{"schema":{"type":"string","format":"binary"}}});
        }
        if code == "204" {
            responses[code].as_object_mut().unwrap().remove("content");
        }
        if path == "/health/ready" {
            responses["503"] = json!({"description":"Database not ready"});
        }
        let security = if path.starts_with("/health/") || path == "/v1/capabilities" {
            json!([])
        } else {
            json!([{"bearerAuth":[]},{}])
        };
        let mut operation = json!({"summary":summary,"parameters":params,"responses":responses,"security":security});
        if path == "/v1/extractions" && method == "post" {
            operation["requestBody"] = json!({"required":true,"content":{"multipart/form-data":{"schema":{"type":"object","required":["file"],"properties":{"file":{"type":"string","format":"binary","description":"Repeat the field to submit up to 32 files in one request; each becomes a job and the response then contains a jobs array"},"options":{"type":"string","description":"JSON encoded Options (empty uses defaults)","example":"{\"ocr\":false,\"vision\":false}"},"callback_url":{"type":"string","description":"Optional http(s) URL notified when the job reaches a terminal state (signed when webhook_secret is set)","example":"https://example.com/hooks/docsmith"}}}}}});
        }
        if path == "/v1/extractions/{id}" && method == "patch" {
            operation["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"type":"object","properties":{"unit_id":{"type":"string","example":"u1"},"block_id":{"type":"string","example":"u1-b2"},"text":{"type":"string","description":"Corrected block or cell text"},"original":{"type":"string","description":"Raw text before the correction, used by the UI to revert"},"cell_row":{"type":"integer","description":"Zero-based table row when correcting one cell"},"cell_column":{"type":"integer","description":"Zero-based table column when correcting one cell"},"status":{"type":"string","enum":["reviewed","needs_review","clear"]},"note":{"type":"string"},"revert":{"type":"boolean","description":"Remove the block or cell correction"},"display_name":{"type":"string","description":"Library title; empty clears it (with ?library=true)"},"collection":{"type":"string","description":"Collection name; empty clears it"},"tags":{"type":"array","items":{"type":"string"},"description":"Replacement tag list"},"favorite":{"type":"boolean"},"redactions":{"type":"array","description":"Saved redaction targets (with ?redaction=true)","items":{"type":"object","properties":{"unit_id":{"type":"string"},"block_id":{"type":"string"},"row":{"type":"integer"},"column":{"type":"integer"},"category":{"type":"string"},"value":{"type":"string"}}}},"clear_redactions":{"type":"boolean"}}}}}});
        }
        if path.ends_with("/result") && method == "post" {
            operation["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"type":"object","required":["question"],"properties":{"question":{"type":"string","maxLength":2000},"max_sources":{"type":"integer","minimum":3,"maximum":20}}}}}});
        }
        paths.entry(path).or_insert(json!({}))[method] = operation;
    }
    let mut spec = json!({"openapi":"3.1.0","info":{"title":"Document Extractor","version":env!("CARGO_PKG_VERSION")},"paths":paths,"components":{"securitySchemes":{"bearerAuth":{"type":"http","scheme":"bearer"}},"schemas":{"Options":options_schema,"Failure":serde_json::to_value(schemars::schema_for!(Failure)).unwrap()}}});
    let mut definitions = serde_json::Map::new();
    lift_definitions(&mut spec, &mut definitions);
    for (name, schema) in definitions {
        spec["components"]["schemas"][name] = schema;
    }
    spec
}
/// OpenAPI document served to clients: the live contract. Resource operations always declare
/// the bearer scheme (with an anonymous alternative on loopback); once tokens are configured
/// the requirement becomes strict.
pub fn openapi_document(config: &Config) -> Value {
    let mut spec = openapi();
    if !config.tokens.is_empty() {
        for (path, item) in spec["paths"].as_object_mut().unwrap() {
            if !path.starts_with("/health/") && path != "/v1/capabilities" {
                for operation in item.as_object_mut().unwrap().values_mut() {
                    operation["security"] = json!([{"bearerAuth":[]}]);
                }
            }
        }
    }
    spec
}
pub fn router(app: App) -> Router {
    let limit = app.config.limits.upload_bytes as usize + 16384;
    Router::new()
        .route("/", get(landing))
        .route("/view/{id}", get(view))
        .route("/assets/web/app.css", get(web_css))
        .route("/assets/web/app.js", get(web_js))
        .route("/wasm/doc_extractor.wasm", get(web_wasm))
        .route("/v1/extractions", get(list).post(upload))
        .route("/v1/stats", get(stats))
        .route("/metrics", get(metrics))
        .route(
            "/v1/extractions/{id}",
            get(status).delete(delete).patch(update_review),
        )
        .route("/v1/extractions/{id}/result", get(result).post(ask_result))
        .route("/v1/extractions/{id}/units/{unit_id}", get(unit))
        .route("/v1/extractions/{id}/assets/{asset_id}", get(asset))
        .route("/v1/extractions/{id}/cancel", post(cancel))
        .route("/v1/extractions/{id}/events", get(events))
        .route("/v1/extractions/{id}/archive", get(archive))
        .route(
            "/v1/capabilities",
            get(|| async {
                Json(
                    tokio::task::spawn_blocking(capabilities)
                        .await
                        .unwrap_or_else(|_| json!({"error":"capability_probe_failed"})),
                )
            }),
        )
        .route(
            "/openapi.json",
            get(|State(app): State<App>| async move { Json(openapi_document(&app.config)) }),
        )
        .route(
            "/health/live",
            get(|| async { Json(json!({"status":"live"})) }),
        )
        .route(
            "/health/ready",
            get(|State(app): State<App>| async move {
                let ok = app
                    .db
                    .lock()
                    .unwrap()
                    .query_row("SELECT 1", [], |r| r.get::<_, i64>(0))
                    .is_ok();
                (
                    if ok {
                        StatusCode::OK
                    } else {
                        StatusCode::SERVICE_UNAVAILABLE
                    },
                    Json(json!({"status":if ok {"ready"}else{"not_ready"}})),
                )
            }),
        )
        // Interactive API console. The UI assets are compiled into the binary, so `docs`
        // works offline; the spec it loads is the same live contract as `/openapi.json`.
        .merge(
            SwaggerUi::new("/docs")
                .external_url_unchecked("/docs/openapi.json", openapi_document(&app.config)),
        )
        .layer(DefaultBodyLimit::max(limit))
        .with_state(app)
}
pub async fn serve(config: Config) -> Result<()> {
    let addr: std::net::SocketAddr = config.bind.parse()?;
    if !addr.ip().is_loopback() && config.tokens.is_empty() {
        anyhow::bail!("Bearer token mapping is required for a non-loopback bind");
    }
    std::fs::create_dir_all(&config.data_dir)?;
    // POSIX advisory lock held for server lifetime; prevents multiple servers sharing a queue.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(config.data_dir.join("server.lock"))?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            anyhow::bail!("Another server owns this data directory");
        }
    }
    let app = App::open(config)?;
    // Opt-in retention sweep: removes terminal jobs older than retention_seconds.
    let sweep_interval = match app.config.limits.retention_seconds {
        0 => Duration::from_secs(30),
        seconds => Duration::from_secs(seconds.clamp(1, 15)),
    };
    let sweeper = {
        let app = app.clone();
        tokio::spawn(async move {
            loop {
                match app.sweep_retention() {
                    Ok(0) => {}
                    Ok(removed) => tracing::info!("retention sweep removed {removed} jobs"),
                    Err(e) => tracing::error!("retention sweep failed: {e:#}"),
                }
                tokio::time::sleep(sweep_interval).await;
            }
        })
    };
    let dispatcher = tokio::spawn(app.clone().dispatcher());
    let webhooks = tokio::spawn(app.clone().webhook_worker());
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Listening on {}", listener.local_addr()?);
    let running = app.running.clone();
    axum::serve(listener, router(app))
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            for pid in running.lock().unwrap().values() {
                kill_group(*pid);
            }
        })
        .await?;
    dispatcher.abort();
    sweeper.abort();
    webhooks.abort();
    drop(lock);
    Ok(())
}
pub fn worker(dir: &std::path::Path) -> Result<()> {
    atomic_json(&dir.join("worker.pid"), &std::process::id())?;
    let req: Value = serde_json::from_slice(&std::fs::read(dir.join("request.json"))?)?;
    let options: Options = serde_json::from_value(req["options"].clone())?;
    let limits: Limits = serde_json::from_value(req["limits"].clone())?;
    extract::extract(
        &dir.join("input"),
        req["filename"].as_str().context("filename")?,
        dir,
        options,
        limits,
        req["id"].as_str().context("id")?,
    )?;
    Ok(())
}

fn lift_definitions(value: &mut Value, definitions: &mut serde_json::Map<String, Value>) {
    match value {
        Value::Object(map) => {
            map.remove("$schema");
            if let Some(Value::Object(defs)) = map.remove("$defs") {
                for (name, mut schema) in defs {
                    lift_definitions(&mut schema, definitions);
                    definitions.insert(name, schema);
                }
            }
            if let Some(Value::String(reference)) = map.get_mut("$ref") {
                if let Some(name) = reference.strip_prefix("#/$defs/") {
                    *reference = format!("#/components/schemas/{name}");
                }
            }
            for child in map.values_mut() {
                lift_definitions(child, definitions);
            }
        }
        Value::Array(array) => {
            for child in array {
                lift_definitions(child, definitions);
            }
        }
        _ => {}
    }
}

async fn process_tree_rss(root: u32) -> Result<u64> {
    let mut command = Command::new("ps");
    command.args(["-axo", "pid=,ppid=,rss="]).kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(1), command.output()).await??;
    let rows = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let v = line.split_whitespace().collect::<Vec<_>>();
            if v.len() == 3 {
                Some((
                    v[0].parse::<u32>().ok()?,
                    v[1].parse::<u32>().ok()?,
                    v[2].parse::<u64>().ok()? * 1024,
                ))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let mut ids = std::collections::HashSet::from([root]);
    loop {
        let before = ids.len();
        for (pid, parent, _) in &rows {
            if ids.contains(parent) {
                ids.insert(*pid);
            }
        }
        if before == ids.len() {
            break;
        }
    }
    Ok(rows
        .iter()
        .filter(|(pid, _, _)| ids.contains(pid))
        .map(|(_, _, rss)| rss)
        .sum())
}
fn directory_bytes(path: &std::path::Path) -> Result<u64> {
    let mut total = 0u64;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        total = total.saturating_add(if metadata.is_dir() {
            directory_bytes(&entry.path())?
        } else {
            metadata.len()
        });
    }
    Ok(total)
}

pub fn worker_command(
    dir: &std::path::Path,
    limits: &Limits,
    lock_path: &std::path::Path,
) -> Result<Command> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("worker")
        .arg("--job-dir")
        .arg(dir)
        .kill_on_drop(true);
    command.env(
        "DOC_VLM_LOCK",
        std::env::var_os("DOC_VLM_LOCK").unwrap_or_else(|| lock_path.as_os_str().to_os_string()),
    );
    // Machine-wide OCR budget: every worker process contends on the same slot files next to the
    // vision lock (the service data dir), so concurrent jobs share `limits.ocr_threads`.
    let slots = std::env::var_os("DOC_OCR_SLOTS_DIR").unwrap_or_else(|| {
        lock_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("ocr-slots")
            .into_os_string()
    });
    command.env("DOC_OCR_SLOTS_DIR", slots);
    command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::from(std::fs::File::create(
            dir.join("worker.log"),
        )?));
    let memory = limits.memory_bytes;
    let cpu = limits.timeout_seconds;
    // Headroom above the checked output limit so oversized results fail with a clean error from
    // the size check instead of SIGXFSZ during the atomic temp write.
    let file = limits.output_bytes.saturating_add(64 * 1024 * 1024);
    #[cfg(unix)]
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let limits = [(libc::RLIMIT_CPU, cpu), (libc::RLIMIT_FSIZE, file)];
            for (resource, value) in limits {
                let lim = libc::rlimit {
                    rlim_cur: value as libc::rlim_t,
                    rlim_max: value as libc::rlim_t,
                };
                if libc::setrlimit(resource, &lim) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            #[cfg(target_os = "linux")]
            {
                let lim = libc::rlimit {
                    rlim_cur: memory as libc::rlim_t,
                    rlim_max: memory as libc::rlim_t,
                };
                if libc::setrlimit(libc::RLIMIT_AS, &lim) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            let _ = memory;
            Ok(())
        });
    }

    Ok(command)
}

struct ProcessGroup(u32);
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        kill_group(self.0);
    }
}

pub async fn supervise_worker(
    dir: &std::path::Path,
    limits: &Limits,
    mut child: tokio::process::Child,
) -> Result<()> {
    let pid = child.id().unwrap_or(0);
    let _group = ProcessGroup(pid);
    let memory = limits.memory_bytes;
    let cpu = limits.timeout_seconds;
    let file = limits.output_bytes;
    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            kill_group(pid);
            if !status.success() {
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    if status
                        .signal()
                        .is_some_and(|s| s == libc::SIGXCPU || s == libc::SIGXFSZ)
                    {
                        anyhow::bail!("limit_exceeded: worker CPU/file size limit ({status})");
                    }
                }
                anyhow::bail!("worker_failed: exit {status}; see retained worker.log");
            }
            let metadata = std::fs::metadata(dir.join("result.json"))?;
            if metadata.len() > file {
                anyhow::bail!("limit_exceeded: result bytes");
            }
            return Ok(());
        }
        let mut reason = None;
        if start.elapsed() >= Duration::from_secs(cpu) {
            reason = Some("worker timeout");
        }
        let rss = process_tree_rss(pid).await.unwrap_or(0);
        if rss > memory {
            reason = Some("worker process tree memory");
        }
        let scratch_limit = limits
            .upload_bytes
            .saturating_add(limits.archive_bytes)
            .saturating_add(file.saturating_mul(3));
        if directory_bytes(dir).unwrap_or(0) > scratch_limit {
            reason = Some("worker scratch bytes");
        }
        if let Some(reason) = reason {
            kill_group(pid);
            let _ = child.kill().await;
            let _ = child.wait().await;
            anyhow::bail!("limit_exceeded: {reason}");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn multipart_error(e: axum::extract::multipart::MultipartError, code: &str) -> ApiError {
    let status = e.status();
    error(
        status,
        if status == StatusCode::PAYLOAD_TOO_LARGE {
            "limit_exceeded"
        } else {
            code
        },
        &e.to_string(),
    )
}

fn mime_for_filename(name: &str) -> &'static str {
    match name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "tif" | "tiff" => "image/tiff",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "txt" | "log" | "csv" | "tsv" => "text/plain; charset=utf-8",
        "md" | "markdown" => "text/markdown; charset=utf-8",
        "html" | "htm" => "text/html; charset=utf-8",
        "json" => "application/json",
        "xml" => "application/xml",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

async fn stream_file(path: PathBuf, mime: &str) -> ApiResult<Response> {
    let file = tokio::fs::File::open(path).await?;
    let length = file.metadata().await?.len();
    let stream = tokio_util::io::ReaderStream::new(file);
    let body = axum::body::Body::from_stream(stream);
    Ok((
        [
            ("content-type", mime.to_owned()),
            ("content-length", length.to_string()),
            ("x-content-type-options", "nosniff".into()),
        ],
        body,
    )
        .into_response())
}
