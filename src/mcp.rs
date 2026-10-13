//! MCP (Model Context Protocol) server over stdio.
//!
//! Exposes the extractor to agent clients (Claude Code, Cursor, ...) as typed tools.
//! Offline mode (default) runs the same CLI/worker path in a subprocess — it never adds a
//! second extraction path. With `--url` the tools proxy a running service instead.
//!
//! Protocol: JSON-RPC 2.0, one message per line (the stdio transport defined by MCP).
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;

pub struct McpConfig {
    /// Service base URL; `None` runs extraction locally via the CLI.
    pub url: Option<String>,
    pub token: Option<String>,
}

const DEFAULT_MAX_CHARS: usize = 20_000;
const PROTOCOL_VERSION: &str = "2025-06-18";

pub fn run(url: Option<String>) -> Result<()> {
    let config = McpConfig {
        url,
        token: std::env::var("DOC_TOKEN").ok(),
    };
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(e) => {
                writeln!(
                    out,
                    "{}",
                    json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":format!("parse error: {e}")}})
                )?;
                out.flush()?;
                continue;
            }
        };
        if let Some(response) = handle_request(&request, &config) {
            writeln!(out, "{response}")?;
            out.flush()?;
        }
    }
    Ok(())
}

/// Handle one JSON-RPC message. Notifications (no `id`) return `None`.
pub fn handle_request(request: &Value, config: &McpConfig) -> Option<Value> {
    let id = request.get("id").cloned();
    let method = request["method"].as_str().unwrap_or("");
    let respond = |result: Value| {
        id.as_ref()
            .map(|id| json!({"jsonrpc":"2.0","id":id,"result":result}))
    };
    match method {
        "initialize" => respond(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "doc-extractor", "version": env!("CARGO_PKG_VERSION")},
            "instructions": "Offline document extraction. Use `extract` with a local file path; \
                set --url (or DOC_BASE_URL) to use a running service, which also enables \
                job_status/get_result/list_jobs/stats.",
        })),
        "ping" => respond(json!({})),
        "notifications/initialized" | "notifications/cancelled" => None,
        "tools/list" => respond(json!({"tools": tools()})),
        "tools/call" => {
            let name = request["params"]["name"].as_str().unwrap_or("");
            let arguments = request["params"]["arguments"].clone();
            Some(match call_tool(name, &arguments, config) {
                Ok(text) => json!({"jsonrpc":"2.0","id":id?, "result": {"content":[{"type":"text","text":text}],"isError":false}}),
                Err(e) => json!({"jsonrpc":"2.0","id":id?, "result": {"content":[{"type":"text","text":format!("{e:#}")}],"isError":true}}),
            })
        }
        _ => id.map(|id| {
            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":format!("unknown method: {method}")}})
        }),
    }
}

fn tools() -> Vec<Value> {
    let format = json!({"type":"string","enum":["text","markdown","json","jsonl"],"default":"text","description":"Result representation"});
    vec![
        json!({
        "name": "extract",
        "description": "Extract one local document (PDF, Office, CSV, image, ...). Runs the offline extractor and returns the requested representation, truncated to max_chars.",
        "inputSchema": {"type":"object","required":["path"],"properties":{
            "path": {"type":"string","description":"Absolute or relative path to the document"},
            "format": format,
            "options": {"type":"object","description":"Extractor options, for example {\"ocr\":true}"},
            "max_chars": {"type":"integer","default":DEFAULT_MAX_CHARS}
        }}}),
        json!({
            "name": "capabilities",
            "description": "Supported formats and optional OCR/vision providers.",
            "inputSchema": {"type":"object","properties":{}},
        }),
        json!({
            "name": "job_status",
            "description": "Job state and progress (requires a service URL).",
            "inputSchema": {"type":"object","required":["id"],"properties":{"id":{"type":"string"}}},
        }),
        json!({
            "name": "get_result",
            "description": "Stored result of a completed job (requires a service URL).",
            "inputSchema": {"type":"object","required":["id"],"properties":{"id":{"type":"string"},"format":format,"max_chars":{"type":"integer","default":DEFAULT_MAX_CHARS}}},
        }),
        json!({
            "name": "list_jobs",
            "description": "List jobs, newest first (requires a service URL).",
            "inputSchema": {"type":"object","properties":{"state":{"type":"string"},"limit":{"type":"integer","default":20}}},
        }),
        json!({
            "name": "stats",
            "description": "Queue, job and disk statistics (requires a service URL).",
            "inputSchema": {"type":"object","properties":{}},
        }),
    ]
}

fn call_tool(name: &str, arguments: &Value, config: &McpConfig) -> Result<String> {
    let max_chars = arguments["max_chars"]
        .as_u64()
        .map(|n| n as usize)
        .unwrap_or(DEFAULT_MAX_CHARS)
        .clamp(200, 200_000);
    let format = arguments["format"].as_str().unwrap_or("text").to_owned();
    match name {
        "capabilities" => match &config.url {
            Some(url) => Ok(http_get(url, config, "/v1/capabilities", None)?),
            None => Ok(serde_json::to_string_pretty(
                &crate::service::capabilities(),
            )?),
        },
        "extract" => {
            let path = arguments["path"]
                .as_str()
                .context("extract requires a `path` argument")?
                .to_owned();
            if !PathBuf::from(&path).is_file() {
                bail!("no such file: {path}");
            }
            let options = arguments.get("options").filter(|v| !v.is_null()).cloned();
            match &config.url {
                Some(url) => extract_via_service(url, config, &path, &format, options, max_chars),
                None => extract_via_cli(&path, &format, options, max_chars),
            }
        }
        "job_status" => {
            let id = arguments["id"]
                .as_str()
                .context("job_status requires `id`")?;
            http_get(
                require_url(config)?,
                config,
                &format!("/v1/extractions/{id}"),
                None,
            )
        }
        "get_result" => {
            let id = arguments["id"]
                .as_str()
                .context("get_result requires `id`")?;
            let url = require_url(config)?;
            let body = http_get(
                url,
                config,
                &format!("/v1/extractions/{id}/result"),
                Some(&format),
            )?;
            Ok(truncate(body, max_chars))
        }
        "list_jobs" => {
            let state = arguments["state"].as_str();
            let limit = arguments["limit"].as_u64().unwrap_or(20).clamp(1, 200);
            let query = match state {
                Some(state) => format!("/v1/extractions?state={state}&limit={limit}"),
                None => format!("/v1/extractions?limit={limit}"),
            };
            http_get(require_url(config)?, config, &query, None)
        }
        "stats" => http_get(require_url(config)?, config, "/v1/stats", None),
        other => bail!("unknown tool: {other}"),
    }
}

fn require_url(config: &McpConfig) -> Result<&str> {
    config
        .url
        .as_deref()
        .context("this tool needs a service; start it with `--url http://127.0.0.1:8787`")
}

fn http_get(base: &str, config: &McpConfig, path: &str, format: Option<&str>) -> Result<String> {
    let url = match format {
        Some(format) => format!("{}{}?format={format}", base.trim_end_matches('/'), path),
        None => format!("{}{}", base.trim_end_matches('/'), path),
    };
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()?;
    let mut request = client.get(&url);
    if let Some(token) = &config.token {
        request = request.bearer_auth(token);
    }
    let response = request.send()?;
    let status = response.status();
    let body = response.text()?;
    if !status.is_success() {
        bail!("service returned {status}: {}", body.trim());
    }
    Ok(body)
}

fn extract_via_service(
    base: &str,
    config: &McpConfig,
    path: &str,
    format: &str,
    options: Option<Value>,
    max_chars: usize,
) -> Result<String> {
    let mut form = reqwest::blocking::multipart::Form::new().file("file", path)?;
    if let Some(options) = options {
        form = form.text("options", options.to_string());
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()?;
    let mut request = client
        .post(format!("{}/v1/extractions", base.trim_end_matches('/')))
        .multipart(form);
    if let Some(token) = &config.token {
        request = request.bearer_auth(token);
    }
    let response = request.send()?;
    let status = response.status();
    let job: Value = response.json().context("service did not return a job")?;
    if !status.is_success() {
        bail!("upload failed ({status}): {job}");
    }
    let id = job["id"].as_str().context("job has no id")?.to_owned();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3600);
    loop {
        let state: Value = serde_json::from_str(&http_get(
            base,
            config,
            &format!("/v1/extractions/{id}"),
            None,
        )?)?;
        let state_name = state["state"].as_str().unwrap_or("").to_owned();
        if ["completed", "failed", "cancelled"].contains(&state_name.as_str()) {
            if state_name != "completed" {
                bail!("job {id} ended {state_name}: {state}");
            }
            break;
        }
        if std::time::Instant::now() > deadline {
            bail!("job {id} still {state_name} after 3600s");
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    let body = http_get(
        base,
        config,
        &format!("/v1/extractions/{id}/result"),
        Some(format),
    )?;
    Ok(format!(
        "job {id} completed ({format}); {}\n\n{}",
        if body.len() > max_chars {
            "truncated"
        } else {
            "full result"
        },
        truncate(body, max_chars)
    ))
}

fn extract_via_cli(
    path: &str,
    format: &str,
    options: Option<Value>,
    max_chars: usize,
) -> Result<String> {
    let exe = std::env::current_exe().context("current executable")?;
    let dir = std::env::temp_dir().join(format!(
        "doc-extractor-mcp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir)?;
    let mut command = std::process::Command::new(exe);
    command
        .arg("extract")
        .arg(path)
        .arg("--format")
        .arg(format)
        .arg("--stdout")
        .arg("--output")
        .arg(&dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(options) = &options {
        let options_path = dir.join("options.json");
        std::fs::write(&options_path, serde_json::to_vec(options)?)?;
        command.arg("--options").arg(&options_path);
    }
    let mut child = command.spawn().context("spawn extractor")?;
    let mut stdout = child.stdout.take().context("stdout")?;
    let mut stderr = child.stderr.take().context("stderr")?;
    let errors = std::thread::spawn(move || {
        let mut buffer = String::new();
        let _ = stderr.read_to_string(&mut buffer);
        buffer
    });
    let mut body = Vec::new();
    let mut chunk = [0u8; 16384];
    let mut truncated = false;
    loop {
        let read = stdout.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
        if body.len() > max_chars {
            truncated = true;
            let _ = child.kill();
            break;
        }
    }
    let status = child.wait()?;
    let stderr = errors.join().unwrap_or_default();
    let progress = std::fs::read(dir.join("progress.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .unwrap_or(Value::Null);
    let _ = std::fs::remove_dir_all(&dir);
    if !status.success() && !truncated {
        bail!("extraction failed: {}", stderr.trim());
    }
    let document_status = progress["result_status"]
        .as_str()
        .unwrap_or("unknown")
        .to_owned();
    let text = String::from_utf8_lossy(&body).into_owned();
    Ok(format!(
        "status {document_status} ({format}); {}\n\n{}",
        if truncated {
            "truncated"
        } else {
            "full result"
        },
        truncate(text, max_chars)
    ))
}

fn truncate(text: String, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text;
    }
    let mut end = max_chars;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n\n[... truncated at {max_chars} of {} bytes; raise max_chars or use get_result]",
        &text[..end],
        text.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offline() -> McpConfig {
        McpConfig {
            url: None,
            token: None,
        }
    }

    #[test]
    fn initialize_and_ping() {
        let request = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});
        let response = handle_request(&request, &offline()).unwrap();
        assert_eq!(response["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(response["result"]["serverInfo"]["name"], "doc-extractor");
        assert!(
            handle_request(&json!({"jsonrpc":"2.0","id":2,"method":"ping"}), &offline()).is_some()
        );
        assert!(handle_request(
            &json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            &offline()
        )
        .is_none());
    }

    #[test]
    fn tools_list_and_capabilities() {
        let request = json!({"jsonrpc":"2.0","id":3,"method":"tools/list"});
        let response = handle_request(&request, &offline()).unwrap();
        let names = response["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert!(names.contains(&"extract".to_string()));
        assert!(names.contains(&"capabilities".to_string()));
        let call = json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"capabilities","arguments":{}}});
        let response = handle_request(&call, &offline()).unwrap();
        assert_eq!(response["result"]["isError"], false);
        assert!(response["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("native_formats"));
    }

    #[test]
    fn errors_are_reported() {
        let unknown =
            handle_request(&json!({"jsonrpc":"2.0","id":5,"method":"nope"}), &offline()).unwrap();
        assert_eq!(unknown["error"]["code"], -32601);
        let missing = handle_request(
            &json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"extract","arguments":{"path":"/definitely/not/here.pdf"}}}),
            &offline(),
        )
        .unwrap();
        assert_eq!(missing["result"]["isError"], true);
        assert!(missing["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("no such file"));
        let service_only = handle_request(
            &json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"stats","arguments":{}}}),
            &offline(),
        )
        .unwrap();
        assert_eq!(service_only["result"]["isError"], true);
        assert!(service_only["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("--url"));
    }
}
