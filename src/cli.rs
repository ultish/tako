use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "tako", about = "Multi-service Gradle + Skaffold control plane (蛸)")]
pub struct Cli {
    /// Override the config directory (defaults to ~/.config/tako).
    #[arg(long, value_name = "PATH")]
    pub config_dir: Option<PathBuf>,
}
