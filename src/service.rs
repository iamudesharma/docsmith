use crate::{
    extract::{self, atomic_json, ENGINE_VERSION},
    model::*,
    providers,
};
use anyhow::{Context, Result};
use axum::{
    extract::{DefaultBodyLimit, Multipart, Path as ApiPath, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
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
use utoipa_swagger_ui::SwaggerUi;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub data_dir: PathBuf,
    pub bind: String,
    pub limits: Limits,
    pub tokens: HashMap<String, String>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from("data"),
            bind: "127.0.0.1:8787".into(),
            limits: Limits::default(),
            tokens: HashMap::new(),
        }
    }
}
#[derive(Clone)]
pub struct App {
    pub config: Arc<Config>,
    db: Arc<Mutex<Connection>>,
    running: Arc<Mutex<HashMap<String, u32>>>,
    provider_fingerprint: String,
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
async fn upload(
    State(app): State<App>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> ApiResult<Response> {
    let tenant = app.tenant(&headers)?;
    let id = uuid::Uuid::new_v4().to_string();
    let dir = app.dir(&id);
    tokio::fs::create_dir_all(&dir).await?;
    let result = upload_inner(&app, &headers, &tenant, &id, &dir, &mut multipart).await;
    if result.is_err() {
        let _ = tokio::fs::remove_dir_all(dir).await;
    }
    result
}
async fn upload_inner(
    app: &App,
    _headers: &HeaderMap,
    tenant: &str,
    id: &str,
    dir: &std::path::Path,
    multipart: &mut Multipart,
) -> ApiResult<Response> {
    let mut filename = None;
    let mut options = Options::default();
    let mut seen_options = false;
    let mut uploaded = 0u64;
    let mut digest = sha2::Sha256::new();
    use sha2::Digest;
    use tokio::io::AsyncWriteExt;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| multipart_error(e, "invalid_multipart"))?
    {
        match field.name().unwrap_or("") {
            "file" => {
                if filename.is_some() {
                    return Err(error(
                        StatusCode::BAD_REQUEST,
                        "invalid_upload",
                        "Exactly one file required",
                    ));
                }
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
                uploaded = size;
                filename = Some(name);
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
            _ => {
                return Err(error(
                    StatusCode::BAD_REQUEST,
                    "invalid_field",
                    "Expected file and options fields",
                ))
            }
        }
    }
    let filename = filename
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "invalid_upload", "File required"))?;
    validate_options(&options)?;
    let options_json = serde_json::to_string(&options).map_err(anyhow::Error::from)?;
    let key = extract::hash(
        format!(
            "{}:{}:{}:{}:{}",
            hex::encode(digest.finalize()),
            filename,
            options_json,
            extract::parser_fingerprint(),
            app.provider_fingerprint
        )
        .as_bytes(),
    );
    atomic_json(
        &dir.join("request.json"),
        &json!({"id":id,"filename":filename,"options":options,"limits":app.config.limits}),
    )?;
    let cached = {
        let db = app.db.lock().unwrap();
        let cached:Option<String>=db.query_row("SELECT id FROM jobs WHERE tenant=?1 AND cache_key=?2 AND state='completed' ORDER BY created_at DESC LIMIT 1",params![tenant,key],|r|r.get(0)).ok();
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
                && used.saturating_add(uploaded) > app.config.limits.disk_bytes
            {
                return Err(error(
                    StatusCode::INSUFFICIENT_STORAGE,
                    "quota_exceeded",
                    "Tenant disk quota exceeded; DELETE completed jobs or raise disk_bytes",
                ));
            }
            db.execute("INSERT INTO jobs(id,tenant,filename,cache_key,options,state,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,'queued',?6,?6)",params![id,tenant,filename,key,options_json,epoch()])?;
        }
        cached
    };
    if let Some(cached) = cached {
        tokio::fs::remove_dir_all(dir).await?;
        return Ok((
            StatusCode::ACCEPTED,
            Json(json!({"id":cached,"cached":true})),
        )
            .into_response());
    }
    Ok((StatusCode::ACCEPTED, Json(json!({"id":id,"cached":false}))).into_response())
}

pub fn validate_options(options: &Options) -> ApiResult<()> {
    if !["direct", "xberg"].contains(&options.parser.as_str()) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_options",
            "Unknown parser",
        ));
    }
    if !["tesseract", "apple_vision"].contains(&options.ocr_provider.as_str()) {
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
    if !["json", "markdown", "md", "text", "txt"].contains(&options.result_format.as_str()) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_options",
            "Expected result_format json, markdown or text",
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
    match export
        .format
        .as_deref()
        .or(default_format.as_deref())
        .unwrap_or("json")
    {
        "json" => stream_file(app.dir(&id).join("result.json"), "application/json").await,
        "markdown" | "md" => {
            stream_file(
                app.dir(&id).join("result.md"),
                "text/markdown; charset=utf-8",
            )
            .await
        }
        "text" | "txt" => {
            stream_file(app.dir(&id).join("result.txt"), "text/plain; charset=utf-8").await
        }
        _ => Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_format",
            "Expected json, markdown or text",
        )),
    }
}
fn valid_part(part: &str, prefix: char) -> bool {
    part.starts_with(prefix)
        && part.len() > 1
        && part.len() <= 64
        && part.chars().all(|c| c.is_ascii_alphanumeric())
}
async fn unit(
    State(app): State<App>,
    ApiPath((id, unit)): ApiPath<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    app.authorize(&id, &headers)?;
    if !valid_part(&unit, 'u') {
        return Err(error(StatusCode::NOT_FOUND, "not_found", "Unit not found"));
    }
    stream_file(
        app.dir(&id).join("units").join(format!("{unit}.json")),
        "application/json",
    )
    .await
    .map_err(|_| error(StatusCode::NOT_FOUND, "not_found", "Unit not found"))
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
    json!({"engine":ENGINE_VERSION,"xberg":{"configured":std::env::var("DOC_XBERG").is_ok(),"required_version":"1.3.8","mapping":"partial"},"native_formats":["txt","md","json","xml","html","csv","tsv","xlsx","xls","xlsm","xlsb","ods","docx","pptx","odt","odp","epub","eml","rtf","drawio","svg","mermaid","dot","plantuml","dbml","vsdx"],"pdf":{"available":providers::available("pdfinfo")&&providers::available("pdftotext")&&providers::available("pdfimages"),"backend":"poppler","tables":"unvalidated","page_geometry":true,"vector_source_preserved":true},"ocr":{"tesseract":providers::available("tesseract"),"languages":providers::languages(),"apple_vision":cfg!(target_os="macos")&&std::env::var("DOC_APPLE_VISION").is_ok(),"paddle_onnx":false},"vision":{"configured":std::env::var("DOC_VLM_URL").is_ok(),"model":std::env::var("DOC_VLM_MODEL").ok(),"automatic_downloads":false},"offline_default":true,"limitations":["Logical Word/ODT sections are not physical pages","Legacy DOC/PPT/VSD are unsupported","Visio/SmartArt source preserved; semantics unsupported","Text diagram parsers implement documented subsets","Spreadsheet formulas are not recalculated"]})
}
pub fn openapi() -> Value {
    let doc_schema = serde_json::to_value(schemars::schema_for!(Document)).unwrap();
    let options_schema = serde_json::to_value(schemars::schema_for!(Options)).unwrap();
    let job_schema = serde_json::to_value(schemars::schema_for!(Job)).unwrap();
    let mut paths = serde_json::Map::new();
    for (path, method, summary, code, schema) in [
        (
            "/v1/extractions",
            "post",
            "Upload one file and JSON options",
            "202",
            json!({"type":"object","required":["id","cached"],"properties":{"id":{"type":"string"},"cached":{"type":"boolean"}}}),
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
            params.push(json!({"name":"format","in":"query","description":"Export format; defaults to the job's result_format option","schema":{"type":"string","enum":["json","markdown","text"]}}));
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
        if path == "/v1/extractions" {
            operation["requestBody"] = json!({"required":true,"content":{"multipart/form-data":{"schema":{"type":"object","required":["file"],"properties":{"file":{"type":"string","format":"binary"},"options":{"type":"string","description":"JSON encoded Options (empty uses defaults)","example":"{\"ocr\":false,\"vision\":false}"}}}}}});
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
        .route("/v1/extractions", post(upload))
        .route("/v1/extractions/{id}", get(status).delete(delete))
        .route("/v1/extractions/{id}/result", get(result))
        .route("/v1/extractions/{id}/units/{unit_id}", get(unit))
        .route("/v1/extractions/{id}/assets/{asset_id}", get(asset))
        .route("/v1/extractions/{id}/cancel", post(cancel))
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
