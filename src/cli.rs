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
    /// Generate backend/domain library code.
    GenLib(GenerateArgs),
    /// Generate documentation output.
    GenDoc(GenerateArgs),
    /// Generate model/frontend output.
    GenModel(GenerateArgs),
    /// Generate Rust workspace output.
    GenWorkspace(GenerateArgs),
    /// Ping the TeaQL service: runs a built-in demo model and prints detailed diagnostics.
    Ping(ServiceArgs),
    /// Show TeaQL service version information.
    Version(ServiceArgs),
    /// Show effective local config.
    ShowConfig,
    /// Configure TeaQL in the current workspace.
    Config,
    /// Install symlink aliases for cargo-style command names.
    InstallLinks(InstallLinksArgs),
    /// Evaluate a KSML model input and report diagnostics.
    Eval(EvalArgs),
}

#[derive(Debug, Args)]
pub struct EvalArgs {
    /// Model file, directory, or zip to evaluate.
    pub input: PathBuf,

    /// Server base URL. Defaults to the configured TeaQL API URL.
    #[arg(long, alias = "server")]
    pub endpoint_prefix: Option<String>,

    /// Override TeaQL service URL. Deprecated: use --endpoint-prefix.
    #[arg(long)]
    pub service_url: Option<String>,

    /// Output format. Defaults to text.
    #[arg(long, default_value = "text")]
    pub format: String,

    /// Write the raw JSON report to a file.
    #[arg(long)]
    pub output: Option<PathBuf>,

    /// Exit non-zero when warnings exist.
    #[arg(long)]
    pub fail_on_warning: bool,

    /// Override request timeout in seconds.
    #[arg(long)]
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Args)]
pub struct GenerateArgs {
    /// Model file or directory to upload.
    pub input: PathBuf,

    /// Override TeaQL endpoint prefix, for example https://api.teaql.io/latest/.
    #[arg(long)]
    pub endpoint_prefix: Option<String>,

    /// Override TeaQL service URL. Deprecated: use --endpoint-prefix.
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

#[derive(Debug, Args)]
pub struct ServiceArgs {
    /// Override TeaQL endpoint prefix, for example https://api.teaql.io/latest/.
    #[arg(long)]
    pub endpoint_prefix: Option<String>,

    /// Override TeaQL service URL. Deprecated: use --endpoint-prefix.
    #[arg(long)]
    pub service_url: Option<String>,

    /// Override license file.
    #[arg(long)]
    pub license_file: Option<PathBuf>,

    /// Override request timeout in seconds.
    #[arg(long)]
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Args)]
pub struct InstallLinksArgs {
    /// Directory where symlinks should be created. Defaults to the current executable directory.
    #[arg(long)]
    pub dir: Option<PathBuf>,

    /// Replace existing files or symlinks when needed.
    #[arg(long)]
    pub force: bool,
}
