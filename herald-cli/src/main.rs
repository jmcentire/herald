mod client;
mod config;
mod error;
mod handler;
mod poller;
mod streamer;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::client::HeraldClient;
use crate::config::{Config, ConnectionMode};

#[derive(Parser)]
#[command(name = "herald", about = "Local daemon for Herald webhook relay")]
struct Cli {
    /// Path to config file
    #[arg(short, long)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the daemon (poll or stream based on config)
    Run,

    /// Validate the config file
    Check,

    /// Show the resolved config
    Show,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let config_path = cli.config.unwrap_or_else(Config::default_path);

    match cli.command {
        Commands::Check => {
            match Config::load(&config_path) {
                Ok(config) => {
                    println!("Config valid: {}", config_path.display());
                    println!("  Server: {}", config.server);
                    println!("  Connection: {:?}", config.connection);
                    println!("  Handlers: {}", config.handlers.len());
                    for (name, handler) in &config.handlers {
                        println!("    {name}: {} {}", handler.command, handler.args.join(" "));
                    }
                }
                Err(e) => {
                    eprintln!("Config error: {e}");
                    std::process::exit(1);
                }
            }
        }

        Commands::Show => {
            match Config::load(&config_path) {
                Ok(config) => {
                    println!("{}", serde_yaml::to_string(&serde_json::to_value(&config).unwrap_or_default()).unwrap_or_default());
                }
                Err(e) => {
                    eprintln!("Config error: {e}");
                    std::process::exit(1);
                }
            }
        }

        Commands::Run => {
            let config = match Config::load(&config_path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Failed to load config from {}: {e}", config_path.display());
                    eprintln!("Create a config file at {} or use --config <path>", Config::default_path().display());
                    std::process::exit(1);
                }
            };

            if config.handlers.is_empty() {
                eprintln!("No handlers configured. Add handlers to your config file.");
                std::process::exit(1);
            }

            tracing::info!(
                server = %config.server,
                connection = ?config.connection,
                handlers = config.handlers.len(),
                "starting herald-cli"
            );

            match config.connection {
                ConnectionMode::Poll => {
                    let client = HeraldClient::new(&config.server, &config.api_key);
                    if let Err(e) = poller::run(&config, &client).await {
                        tracing::error!(error = %e, "poller error");
                        std::process::exit(1);
                    }
                }
                ConnectionMode::Websocket => {
                    if let Err(e) = streamer::run(&config).await {
                        tracing::error!(error = %e, "streamer error");
                        std::process::exit(1);
                    }
                }
            }
        }
    }
}
