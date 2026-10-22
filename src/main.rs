use clap::{CommandFactory, Parser, Subcommand};
use doc_extractor::{extract, model::*, service};
use std::path::PathBuf;
#[derive(Parser)]
#[command(version, about = "Offline document extraction backend")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}
#[derive(Subcommand)]
enum Commands {
    Serve {
        #[arg(long, default_value = "127.0.0.1:8787", env = "DOC_BIND")]
        bind: String,
        #[arg(long, default_value = "data", env = "DOC_DATA_DIR")]
        data_dir: PathBuf,
        #[arg(long)]
        config: Option<PathBuf>,
    },
    Extract {
        input: PathBuf,
        #[arg(long, default_value = "data/cli-result")]
        output: PathBuf,
        #[arg(long, value_parser = ["direct", "xberg"])]
        parser: Option<String>,
        #[arg(long)]
        ocr: bool,
        #[arg(long, default_value = "eng")]
        languages: String,
        #[arg(long)]
        vision: bool,
        #[arg(long)]
        interpret_diagrams: bool,
        #[arg(long)]
        compact_tables: bool,
        /// Result representation: json (default), markdown, text, jsonl, html or chunks.
        #[arg(long, value_parser = ["json", "markdown", "text", "jsonl", "html", "chunks"])]
        format: Option<String>,
        /// Opt-in RAG chunking: write result.chunks.jsonl windows of about this many characters.
        #[arg(long)]
        chunk_chars: Option<usize>,
        /// Overlap between chunks in characters (default chunk_chars / 8).
        #[arg(long)]
        chunk_overlap: Option<usize>,
        /// Keep rendered page images for vision/LLM extraction: failed pages or all pages.
        #[arg(long, value_parser = ["none", "failed", "all"])]
        page_images: Option<String>,
        /// Stream the selected result artifact to stdout (the path goes to stderr).
        #[arg(long)]
        stdout: bool,
        #[arg(long)]
        options: Option<PathBuf>,
    },
    Capabilities,
    Schema,
    Openapi,
    /// MCP server over stdio for agent clients (offline, or proxy a service with --url).
    Mcp {
        #[arg(long, env = "DOC_BASE_URL")]
        url: Option<String>,
    },
    /// Generate shell completions (bash, zsh, fish, elvish, powershell).
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    #[command(hide = true)]
    Worker {
        #[arg(long)]
        job_dir: PathBuf,
    },
}
fn main() -> anyhow::Result<()> {
    let filter = if std::env::var_os("RUST_LOG").is_some() {
        tracing_subscriber::EnvFilter::from_default_env()
    } else {
        // Keep dependency noise quiet but always expose the extraction/timing trail.
        tracing_subscriber::EnvFilter::new("doc_extractor=info")
    };
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);
    if std::env::var("DOC_LOG_JSON").is_ok_and(|value| value == "1" || value == "true") {
        builder.json().init();
    } else {
        builder.init();
    }
    match Cli::parse().command {
        Commands::Serve {
            bind,
            data_dir,
            config,
        } => {
            let mut config = if let Some(path) = config {
                serde_json::from_slice::<service::Config>(&std::fs::read(path)?)?
            } else {
                service::Config {
                    bind,
                    data_dir,
                    ..Default::default()
                }
            };
            if let Ok(tokens) = std::env::var("DOC_TOKENS") {
                config.tokens = serde_json::from_str(&tokens)?;
            }
            if let Ok(secret) = std::env::var("DOC_WEBHOOK_SECRET") {
                config.webhook_secret = Some(secret);
            }
            if std::env::var("DOC_WEBHOOK_ALLOW_PRIVATE").is_ok_and(|v| v == "1" || v == "true") {
                config.webhook_allow_private = true;
            }
            if let Ok(command) = std::env::var("DOC_SINK_CMD") {
                config.sink_command = Some(command);
            }
            tokio::runtime::Runtime::new()?.block_on(service::serve(config))?;
        }
        Commands::Extract {
            input,
            output,
            parser,
            ocr,
            languages,
            vision,
            interpret_diagrams,
            compact_tables,
            format,
            chunk_chars,
            chunk_overlap,
            page_images,
            stdout,
            options,
        } => {
            let opts = if let Some(path) = options {
                serde_json::from_slice(&std::fs::read(path)?)?
            } else {
                Options {
                    parser: parser.unwrap_or_else(|| Options::default().parser),
                    ocr,
                    languages,
                    vision,
                    interpret_diagrams,
                    compact_tables,
                    result_format: format.unwrap_or_else(|| Options::default().result_format),
                    chunk_chars,
                    chunk_overlap,
                    page_images: page_images.unwrap_or_else(|| Options::default().page_images),
                    ..Default::default()
                }
            };
            let input = std::fs::canonicalize(input)?;
            if opts.result_format == "chunks" && opts.chunk_chars.is_none() {
                anyhow::bail!("--format chunks requires --chunk-chars");
            }
            std::fs::create_dir_all(&output)?;
            let output = std::fs::canonicalize(output)?;
            let id = uuid::Uuid::new_v4().to_string();
            extract::atomic_json(
                &output.join("request.json"),
                &serde_json::json!({"id":id,"filename":input.file_name().unwrap().to_string_lossy(),"options":opts,"limits":Limits::default()}),
            )?;
            std::fs::copy(input, output.join("input"))?;
            // CLI uses exactly the service's isolated worker path, including resource limits and process tree cleanup.
            run_cli_worker(&output)?;
            print_page_timings(&output);
            let artifact = match opts.result_format.as_str() {
                "markdown" | "md" => "result.md",
                "text" | "txt" => "result.txt",
                "jsonl" | "ndjson" => "result.jsonl",
                "html" => "result.html",
                "chunks" => "result.chunks.jsonl",
                _ => "result.json",
            };
            let path = output.join(artifact);
            if stdout {
                // Bytes go to stdout so the artifact can be piped (for example to object storage);
                // the path is only informational and goes to stderr.
                use std::io::{copy, Write};
                let mut file = std::fs::File::open(&path)?;
                let mut out = std::io::stdout().lock();
                copy(&mut file, &mut out)?;
                out.flush()?;
                eprintln!("{}", path.display());
            } else {
                println!("{}", path.display());
            }
        }
        Commands::Worker { job_dir } => service::worker(&job_dir)?,
        Commands::Mcp { url } => doc_extractor::mcp::run(url)?,
        Commands::Completions { shell } => {
            let mut command = Cli::command();
            clap_complete::generate(shell, &mut command, "doc-extractor", &mut std::io::stdout());
        }
        Commands::Capabilities => println!(
            "{}",
            serde_json::to_string_pretty(&service::capabilities())?
        ),
        Commands::Schema => println!(
            "{}",
            serde_json::to_string_pretty(&schemars::schema_for!(Document))?
        ),
        Commands::Openapi => println!("{}", serde_json::to_string_pretty(&service::openapi())?),
    }
    Ok(())
}
/// Print the per-page timing trail from `timings.json` to stderr. stdout stays reserved for the
/// artifact path or piped bytes.
fn print_page_timings(output: &std::path::Path) {
    let Ok(bytes) = std::fs::read(output.join("timings.json")) else {
        return;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return;
    };
    let Some(pages) = value.get("pages") else {
        return;
    };
    let number = |key: &str| {
        pages
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    if let Some(per_page) = pages.get("per_page").and_then(serde_json::Value::as_array) {
        eprintln!(
            "{:>4}  {:<4}  {:<7} {:>9} {:>9} {:>9}  {:>7} {:>6}",
            "page", "unit", "method", "total", "render", "ocr", "chars", "tables"
        );
        for page in per_page {
            let num = |key: &str| {
                page.get(key)
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0)
            };
            let text = |key: &str| {
                page.get(key)
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("-")
            };
            eprintln!(
                "{:>4}  {:<4}  {:<7} {:>8.2}s {:>8.2}s {:>8.2}s  {:>7} {:>6}",
                num("page"),
                text("unit"),
                text("method"),
                num("total_ms") as f64 / 1000.0,
                num("render_ms") as f64 / 1000.0,
                num("ocr_ms") as f64 / 1000.0,
                num("chars"),
                num("tables")
            );
        }
    }
    eprintln!(
        "timing: {} pages · {} OCR · {:.1}s page time ({:.1}s OCR) · wall {:.1}s",
        number("pages"),
        number("ocr_pages"),
        number("page_total_ms") as f64 / 1000.0,
        number("ocr_ms") as f64 / 1000.0,
        number("wall_ms") as f64 / 1000.0
    );
}

fn run_cli_worker(output: &std::path::Path) -> anyhow::Result<()> {
    let limits = Limits::default();
    tokio::runtime::Runtime::new()?.block_on(async {
        let mut command = service::worker_command(
            output,
            &limits,
            &output.parent().unwrap_or(output).join("vision.lock"),
        )?;
        let child = command.spawn()?;
        service::supervise_worker(output, &limits, child).await
    })
}
