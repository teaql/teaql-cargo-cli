use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "teaql", version, about = "TeaQL toolchain")]
pub struct Cli {
    #[arg(long, global = true, default_value = ".")]
    pub cwd: PathBuf,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Generate backend/domain code.
    GenCode(GenerateArgs),
    /// Generate documentation output.
    GenDoc(GenerateArgs),
    /// Generate model/frontend output.
    GenModel(GenerateArgs),
    /// Show effective local config.
    ShowConfig,
    /// Configure TeaQL in the current workspace.
    Config,
}

#[derive(Debug, Args)]
pub struct GenerateArgs {
    /// Model file or directory to upload.
    pub input: PathBuf,

    /// Override service URL.
    #[arg(long)]
    pub service_url: Option<String>,

    /// Override license file.
    #[arg(long)]
    pub license_file: Option<PathBuf>,

    /// Override output directory.
    #[arg(long)]
    pub output: Option<PathBuf>,

    /// Override request timeout in seconds.
    #[arg(long)]
    pub timeout_seconds: Option<u64>,
}
