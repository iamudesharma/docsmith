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
use hmac::{Hmac, Mac};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
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
    pub progress: usize,
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
        // Additive migration: databases created before webhooks lack this column.
        let _ = db.execute("ALTER TABLE jobs ADD COLUMN callback_url TEXT", []);
        db.execute_batch("CREATE TABLE IF NOT EXISTS webhook_deliveries(id TEXT PRIMARY KEY, job_id TEXT NOT NULL, url TEXT NOT NULL, event TEXT NOT NULL, payload TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, next_at INTEGER NOT NULL, delivered_at INTEGER, last_error TEXT); CREATE INDEX IF NOT EXISTS webhook_pending ON webhook_deliveries(delivered_at,next_at); CREATE INDEX IF NOT EXISTS webhook_job ON webhook_deliveries(job_id);")?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS sink_runs(job_id TEXT PRIMARY KEY, state TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, next_at INTEGER NOT NULL, completed_at INTEGER, last_error TEXT); CREATE INDEX IF NOT EXISTS sink_pending ON sink_runs(completed_at,next_at);")?;
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
            "SELECT id,state,filename,created_at,updated_at,error FROM jobs WHERE id=?1",
            [id],
            |r| {
                let e: Option<String> = r.get(5)?;
                Ok(Job {
                    id: r.get(0)?,
                    state: r.get(1)?,
                    filename: r.get(2)?,
                    created_at: r.get(3)?,
                    updated_at: r.get(4)?,
                    progress: 0,
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
                job.result_status = progress.result_status;
                job.warnings = progress.warnings;
                job.unit_failures = progress.errors;
            }
        }
        Ok(job)
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
#[derive(Deserialize)]
struct Export {
    format: Option<String>,
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
async fn result(
    State(app): State<App>,
    ApiPath(id): ApiPath<String>,
    headers: HeaderMap,
    Query(export): Query<Export>,
) -> ApiResult<Response> {
    app.authorize(&id, &headers)?;
    if app.job(&id)?.state != "completed" {
        return Err(error(
            StatusCode::CONFLICT,
            "not_ready",
            "Result is not complete; use unit endpoint for checkpoints",
        ));
    }
    let default_format = job_result_format(&app, &id).await;
    let format = export
        .format
        .as_deref()
        .or(default_format.as_deref())
        .unwrap_or("json");
    let (file, mime) = match format {
        "json" => ("result.json", "application/json"),
        "markdown" | "md" => ("result.md", "text/markdown; charset=utf-8"),
        "text" | "txt" => ("result.txt", "text/plain; charset=utf-8"),
        "jsonl" | "ndjson" => ("result.jsonl", "application/x-ndjson; charset=utf-8"),
        "html" => ("result.html", "text/html; charset=utf-8"),
        "chunks" => ("result.chunks.jsonl", "application/x-ndjson; charset=utf-8"),
        _ => {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "invalid_format",
                "Expected json, markdown, text or jsonl",
            ))
        }
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
#[derive(Deserialize)]
struct UnitQuery {
    from: Option<usize>,
    count: Option<usize>,
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
}
async fn list(
    State(app): State<App>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    let tenant = app.tenant(&headers)?;
    let limit = query.limit.unwrap_or(50).min(200) as i64;
    let offset = query.offset.unwrap_or(0) as i64;
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
    let jobs = ids
        .iter()
        .filter_map(|id| app.job(id).ok())
        .collect::<Vec<_>>();
    Ok(Json(
        json!({"jobs": jobs, "total": total, "limit": limit, "offset": offset}),
    ))
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
    if app.job(&id)?.state != "completed" {
        return Err(error(
            StatusCode::CONFLICT,
            "not_ready",
            "Archive requires a completed job",
        ));
    }
    let dir = app.dir(&id);
    let include_units = query.units.unwrap_or(false);
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
    ] {
        let source = dir.join(name);
        if source.is_file() {
            zip.start_file(name, options)?;
            std::io::copy(&mut std::fs::File::open(&source)?, &mut zip)?;
        }
    }
    for (folder, prefix) in [("assets", "assets"), ("pages", "pages"), ("units", "units")] {
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
            zip.start_file(format!("{prefix}/{name}"), options)?;
            std::io::copy(&mut std::fs::File::open(source.join(&name))?, &mut zip)?;
        }
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
    app.db
        .lock()
        .unwrap()
        .execute("DELETE FROM jobs WHERE id=?1", [&id])?;
    Ok(StatusCode::NO_CONTENT)
}
pub fn capabilities() -> Value {
    json!({"engine":ENGINE_VERSION,"xberg":{"configured":std::env::var("DOC_XBERG").is_ok(),"required_version":"1.3.8","mapping":"partial"},"native_formats":["txt","md","json","xml","html","csv","tsv","xlsx","xls","xlsm","xlsb","ods","docx","pptx","odt","odp","epub","eml","rtf","drawio","svg","mermaid","dot","plantuml","dbml","doctags","vsdx"],"pdf":{"available":providers::available("pdfinfo")&&providers::available("pdftotext")&&providers::available("pdfimages"),"backend":"poppler","tables":"unvalidated","page_geometry":true,"page_images":{"available":providers::available("pdftoppm"),"dpi":150},"vector_source_preserved":true},"ocr":{"tesseract":providers::available("tesseract"),"languages":providers::languages(),"apple_vision":cfg!(target_os="macos")&&std::env::var("DOC_APPLE_VISION").is_ok(),"smoldocling":std::env::var("DOC_SMOLDOCLING_URL").is_ok()||std::env::var("DOC_VLM_URL").is_ok(),"paddle_onnx":false},"vision":{"configured":std::env::var("DOC_VLM_URL").is_ok(),"model":std::env::var("DOC_VLM_MODEL").ok(),"automatic_downloads":false},"offline_default":true,"limitations":["Logical Word/ODT sections are not physical pages","Legacy DOC/PPT/VSD are unsupported","Visio/SmartArt source preserved; semantics unsupported","Text diagram parsers implement documented subsets","Spreadsheet formulas are not recalculated"]})
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
            json!({"type":"object","required":["jobs","total"],"properties":{"jobs":{"type":"array","items":job_schema.clone()},"total":{"type":"integer"},"limit":{"type":"integer"},"offset":{"type":"integer"}}}),
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
            params.push(json!({"name":"format","in":"query","description":"Export format; defaults to the job's result_format option","schema":{"type":"string","enum":["json","markdown","text","jsonl","html","chunks"]}}));
        }
        if path.ends_with("/units/{unit_id}") {
            params.push(json!({"name":"from","in":"query","description":"First table row of the slice (used with count)","schema":{"type":"integer","minimum":0}}));
            params.push(json!({"name":"count","in":"query","description":"Maximum table rows in the slice (default 500, max 10000)","schema":{"type":"integer","minimum":1,"maximum":10000}}));
        }
        if path.ends_with("/archive") {
            params.push(json!({"name":"units","in":"query","description":"Include unit checkpoints in the archive","schema":{"type":"boolean"}}));
        }
        if path == "/v1/extractions" && method == "get" {
            params.push(json!({"name":"state","in":"query","schema":{"type":"string","enum":["queued","running","completed","failed","cancelled"]}}));
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
        .route("/v1/extractions/{id}", get(status).delete(delete))
        .route("/v1/extractions/{id}/result", get(result))
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
