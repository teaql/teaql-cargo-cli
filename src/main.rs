mod cli;
mod config;
mod generator;

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands, GenerateArgs};
use config::{ConfigOverrides, TeaqlConfig, config_file_path};

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Config => {
            let config_path = config_file_path()?;
            let existing = TeaqlConfig::load()?;
            let updated = config::run_wizard(existing)?;
            updated.save(&config_path)?;
            println!("saved config to {}", config_path.display());
        }
        Commands::ShowConfig => {
            let config_path = config_file_path()?;
            let config = TeaqlConfig::load()?;
            println!("config_path: {}", config_path.display());
            println!("{}", serde_yaml::to_string(&config)?);
        }
        Commands::GenCode(args) => run_generate(args, None, cli.cwd)?,
        Commands::GenDoc(args) => run_generate(args, Some("doc"), cli.cwd)?,
        Commands::GenModel(args) => run_generate(args, Some("frontend"), cli.cwd)?,
    }

    Ok(())
}

fn run_generate(args: GenerateArgs, scope: Option<&str>, cwd: PathBuf) -> Result<()> {
    let config = TeaqlConfig::load()?;
    let overrides = ConfigOverrides {
        service_url: args.service_url,
        license_file: args.license_file,
        build_dir: args.output,
        timeout_seconds: args.timeout_seconds,
    };
    let resolved = config.resolve(overrides, &cwd);
    generator::generate(&args.input, scope, &resolved)
}
