use std::path::PathBuf;
use std::ffi::OsString;

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
    /// Show effective local config.
    ShowConfig,
    /// Configure TeaQL in the current workspace.
    Config,
    /// Ping the TeaQL service.
    Ping(ServiceArgs),
    /// Install symlink aliases for cargo-style command names.
    InstallLinks(InstallLinksArgs),
    /// Evaluate a KSML model input and report diagnostics.
    Eval(EvalArgs),
    /// Run cargo check and map any compiler errors back to the source KSML (XML) file.
    Check(CheckArgs),
    
    #[command(external_subcommand)]
    Dynamic(Vec<OsString>),
}

#[derive(Debug, Parser)]
#[command(no_binary_name = true)]
pub struct DynamicArgs {
    /// The input model file or directory (if omitted, sends a GET request)
    pub input: Option<PathBuf>,

    /// Override TeaQL endpoint prefix.
    #[arg(long)]
    pub endpoint_prefix: Option<String>,

    /// Override TeaQL service URL. Deprecated: use --endpoint-prefix.
    #[arg(long)]
    pub service_url: Option<String>,

    /// Override API Key.
    #[arg(long)]
    pub api_key: Option<String>,

    /// Override output directory.
    #[arg(long)]
    pub output: Option<PathBuf>,

    /// Override request timeout in seconds.
    #[arg(long)]
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Args)]
pub struct CheckArgs {
    /// Pass additional arguments to cargo check (e.g. --workspace, --tests).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub cargo_args: Vec<String>,
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

    /// Write the raw Markdown report to a file.
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

    /// Override API Key. (Default: Built-in free tier OOTB key)
    #[arg(long)]
    pub api_key: Option<String>,

    /// Override output directory.
    #[arg(long)]
    pub output: Option<PathBuf>,

    /// Override request timeout in seconds.
    #[arg(long)]
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Args)]
pub struct GenServiceArgs {
    #[command(flatten)]
    pub generate_args: GenerateArgs,

    /// The target service to generate (e.g. rust-app-console)
    #[arg(long, short = 's')]
    pub service: String,
}

#[derive(Debug, Args)]
pub struct ServiceArgs {
    /// Override TeaQL endpoint prefix, for example https://api.teaql.io/latest/.
    #[arg(long)]
    pub endpoint_prefix: Option<String>,

    /// Override TeaQL service URL. Deprecated: use --endpoint-prefix.
    #[arg(long)]
    pub service_url: Option<String>,

    /// Override API Key. (Default: Built-in free tier OOTB key)
    #[arg(long)]
    pub api_key: Option<String>,

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
