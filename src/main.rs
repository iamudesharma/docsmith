use clap::{Parser, Subcommand};
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
        options: Option<PathBuf>,
    },
    Capabilities,
    Schema,
    Openapi,
    #[command(hide = true)]
    Worker {
        #[arg(long)]
        job_dir: PathBuf,
    },
}
fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
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
                    ..Default::default()
                }
            };
            let input = std::fs::canonicalize(input)?;
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
            println!("{}", output.join("result.json").display());
        }
        Commands::Worker { job_dir } => service::worker(&job_dir)?,
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
