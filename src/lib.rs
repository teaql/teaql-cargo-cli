pub mod cli;
pub mod config;
pub mod generator;
pub mod service;
pub mod eval;

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use clap::Parser;
use cli::{Cli, Commands, GenerateArgs, InstallLinksArgs, ServiceArgs, EvalArgs};
use config::{ConfigOverrides, EnvConfig, TeaqlConfig, config_file_path};

pub fn run_from_env() -> Result<()> {
    let args: Vec<OsString> = std::env::args_os().collect();
    run_with_args(args)
}

pub fn run_with_args<I, T>(args: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    let argv = rewrite_args_for_alias(args);
    run_cli(Cli::parse_from(argv))
}

pub fn run_cli(cli: Cli) -> Result<()> {
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
        Commands::InstallLinks(args) => install_links(args)?,
        Commands::GenLib(args) => run_generate(args, Some("rust-lib"), cli.cwd)?,
        Commands::GenDoc(args) => run_generate(args, Some("doc"), cli.cwd)?,
        Commands::GenModel(args) => run_generate(args, Some("frontend"), cli.cwd)?,
        Commands::GenWorkspace(args) => run_generate(args, Some("rust-workspace"), cli.cwd)?,
        Commands::Version(args) => run_version(args, cli.cwd)?,
        Commands::Ping(args) => run_ping(args, cli.cwd)?,
        Commands::Eval(args) => {
            let code = run_eval(args, cli.cwd)?;
            std::process::exit(code);
        }
    }

    Ok(())
}

fn run_eval(args: EvalArgs, cwd: PathBuf) -> Result<i32> {
    let config = TeaqlConfig::load()?;
    let env = EnvConfig::from_env();
    let overrides = ConfigOverrides {
        endpoint_prefix: args.endpoint_prefix.clone(),
        service_url: args.service_url.clone(),
        license_file: None,
        build_dir: None,
        timeout_seconds: args.timeout_seconds,
    };
    let resolved = config.resolve(overrides, &env, &cwd);
    eval::evaluate(&args.input, &args, &resolved)
}

fn run_generate(args: GenerateArgs, scope: Option<&str>, cwd: PathBuf) -> Result<()> {
    let config = TeaqlConfig::load()?;
    let env = EnvConfig::from_env();
    let overrides = ConfigOverrides {
        endpoint_prefix: args.endpoint_prefix,
        service_url: args.service_url,
        license_file: args.license_file,
        build_dir: args.output,
        timeout_seconds: args.timeout_seconds,
    };
    let resolved = config.resolve(overrides, &env, &cwd);
    generator::generate(&args.input, scope, &resolved)
}

fn run_version(args: ServiceArgs, cwd: PathBuf) -> Result<()> {
    let config = TeaqlConfig::load()?;
    let env = EnvConfig::from_env();
    let overrides = ConfigOverrides {
        endpoint_prefix: args.endpoint_prefix,
        service_url: args.service_url,
        license_file: None,
        build_dir: None,
        timeout_seconds: args.timeout_seconds,
    };
    let resolved = config.resolve(overrides, &env, &cwd);
    service::print_version(&resolved)
}

fn run_ping(args: ServiceArgs, cwd: PathBuf) -> Result<()> {
    let config = TeaqlConfig::load()?;
    let env = EnvConfig::from_env();
    let overrides = ConfigOverrides {
        endpoint_prefix: args.endpoint_prefix,
        service_url: args.service_url,
        license_file: args.license_file,
        build_dir: Some(std::env::temp_dir().join("teaql-ping")),
        timeout_seconds: args.timeout_seconds,
    };
    let resolved = config.resolve(overrides, &env, &cwd);
    service::ping(&resolved)
}

fn rewrite_args_for_alias(mut args: Vec<OsString>) -> Vec<OsString> {
    let alias_name = args
        .first()
        .and_then(|arg| Path::new(arg).file_name())
        .and_then(|name| name.to_str())
        .map(String::from);

    if let Some(ref program_name) = alias_name {
        if let Some(subcommand) = alias_subcommand(program_name) {
            if args
                .get(1)
                .and_then(|arg| arg.to_str())
                .is_some_and(|arg| arg == cargo_invoked_subcommand(program_name))
            {
                args.remove(1);
            }
            args[0] = OsString::from("teaql");
            args.insert(1, OsString::from(subcommand));
            // Cargo passes the subcommand name (without the "cargo-" prefix)
            // as the second argument, e.g.:
            //   "cargo teaql-version" → argv[1] = "teaql-version"
            // After rewriting, this becomes redundant; strip it.
            let cargo_arg = program_name.strip_prefix("cargo-").unwrap_or(program_name);
            if args.len() > 2 && args[2] == cargo_arg {
                args.remove(2);
            }
        }
    }
    args
}

fn alias_subcommand(program_name: &str) -> Option<&'static str> {
    match program_name {
        "cargo-teaql-gen-lib" => Some("gen-lib"),
        "cargo-teaql-gen-doc" => Some("gen-doc"),
        "cargo-teaql-gen-model" => Some("gen-model"),
        "cargo-teaql-gen-workspace" => Some("gen-workspace"),
        "cargo-teaql-version" => Some("version"),
        "cargo-teaql-ping" => Some("ping"),
        "cargo-teaql-show-config" => Some("show-config"),
        "cargo-teaql-config" => Some("config"),
        "cargo-teaql-eval" => Some("eval"),
        _ => None,
    }
}

fn cargo_invoked_subcommand(program_name: &str) -> &str {
    program_name.strip_prefix("cargo-").unwrap_or(program_name)
}

fn install_links(args: InstallLinksArgs) -> Result<()> {
    #[cfg(not(unix))]
    {
        let _ = args;
        bail!("install-links currently supports Unix-style symlinks only");
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let current_exe = std::env::current_exe().context("failed to locate current executable")?;
        let target = fs::canonicalize(&current_exe)
            .with_context(|| format!("failed to resolve {}", current_exe.display()))?;
        let install_dir = match args.dir {
            Some(dir) => dir,
            None => current_exe
                .parent()
                .context("current executable has no parent directory")?
                .to_path_buf(),
        };

        fs::create_dir_all(&install_dir)
            .with_context(|| format!("failed to create {}", install_dir.display()))?;

        for alias in link_names() {
            let link_path = install_dir.join(alias);
            if link_path.exists() || symlink_metadata_exists(&link_path) {
                if points_to_target(&link_path, &target)? {
                    println!("exists {}", link_path.display());
                    continue;
                }

                if !args.force {
                    bail!(
                        "refusing to overwrite existing path without --force: {}",
                        link_path.display()
                    );
                }

                fs::remove_file(&link_path)
                    .with_context(|| format!("failed to remove {}", link_path.display()))?;
            }

            symlink(&target, &link_path).with_context(|| {
                format!(
                    "failed to create symlink {} -> {}",
                    link_path.display(),
                    target.display()
                )
            })?;
            println!("linked {} -> {}", link_path.display(), target.display());
        }
    }

    Ok(())
}

fn link_names() -> &'static [&'static str] {
    &[
        "teaql",
        "cargo-teaql-gen-lib",
        "cargo-teaql-gen-doc",
        "cargo-teaql-gen-model",
        "cargo-teaql-gen-workspace",
        "cargo-teaql-version",
        "cargo-teaql-show-config",
        "cargo-teaql-ping",
        "cargo-teaql-config",
    ]
}

fn symlink_metadata_exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn points_to_target(link_path: &Path, target: &Path) -> Result<bool> {
    let metadata = match fs::symlink_metadata(link_path) {
        Ok(metadata) => metadata,
        Err(_) => return Ok(false),
    };
    if !metadata.file_type().is_symlink() {
        return Ok(false);
    }

    let linked = fs::canonicalize(link_path)
        .with_context(|| format!("failed to resolve {}", link_path.display()))?;
    Ok(linked == target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_alias_binary_name_to_subcommand() {
        let args = vec![
            OsString::from("/tmp/bin/cargo-teaql-gen-lib"),
            OsString::from("model.yml"),
            OsString::from("--cwd"),
            OsString::from("/workspace"),
        ];

        let rewritten = rewrite_args_for_alias(args);

        assert_eq!(rewritten[0], OsString::from("teaql"));
        assert_eq!(rewritten[1], OsString::from("gen-lib"));
        assert_eq!(rewritten[2], OsString::from("model.yml"));
        assert_eq!(rewritten[3], OsString::from("--cwd"));
        assert_eq!(rewritten[4], OsString::from("/workspace"));
    }

    #[test]
    fn strips_cargo_injected_subcommand_name() {
        // Cargo strips the "cargo-" prefix when passing the subcommand name:
        // "cargo teaql-version" → argv = ["/path/to/cargo-teaql-version", "teaql-version"]
        let args = vec![
            OsString::from("/tmp/bin/cargo-teaql-version"),
            OsString::from("teaql-version"),
        ];

        let rewritten = rewrite_args_for_alias(args);

        assert_eq!(rewritten[0], OsString::from("teaql"));
        assert_eq!(rewritten[1], OsString::from("version"));
        assert_eq!(rewritten.len(), 2, "cargo-injected arg should be stripped");
    }

    #[test]
    fn strips_cargo_injected_arg_for_gen_lib_with_input() {
        // "cargo teaql-gen-lib model.xml"
        // cargo passes: argv = ["/path/to/cargo-teaql-gen-lib", "teaql-gen-lib", "model.xml"]
        let args = vec![
            OsString::from("/tmp/bin/cargo-teaql-gen-lib"),
            OsString::from("teaql-gen-lib"),
            OsString::from("model.xml"),
        ];

        let rewritten = rewrite_args_for_alias(args);

        assert_eq!(rewritten[0], OsString::from("teaql"));
        assert_eq!(rewritten[1], OsString::from("gen-lib"));
        assert_eq!(rewritten[2], OsString::from("model.xml"));
        assert_eq!(rewritten.len(), 3);
    }

    #[test]
    fn leaves_primary_binary_name_unchanged() {
        let args = vec![OsString::from("cargo-teaql"), OsString::from("show-config")];

        let rewritten = rewrite_args_for_alias(args.clone());

        assert_eq!(rewritten, args);
    }

    #[test]
    fn removes_cargo_forwarded_subcommand_argument_for_aliases() {
        let args = vec![
            OsString::from("/tmp/bin/cargo-teaql-show-config"),
            OsString::from("teaql-show-config"),
            OsString::from("--cwd"),
            OsString::from("/workspace"),
        ];

        let rewritten = rewrite_args_for_alias(args);

        assert_eq!(rewritten[0], OsString::from("teaql"));
        assert_eq!(rewritten[1], OsString::from("show-config"));
        assert_eq!(rewritten[2], OsString::from("--cwd"));
        assert_eq!(rewritten[3], OsString::from("/workspace"));
        assert_eq!(rewritten.len(), 4);
    }

    #[test]
    fn link_names_cover_all_aliases() {
        assert!(link_names().contains(&"teaql"));
        assert!(link_names().contains(&"cargo-teaql-gen-lib"));
        assert!(link_names().contains(&"cargo-teaql-gen-doc"));
        assert!(link_names().contains(&"cargo-teaql-gen-model"));
        assert!(link_names().contains(&"cargo-teaql-gen-workspace"));
        assert!(link_names().contains(&"cargo-teaql-version"));
        assert!(link_names().contains(&"cargo-teaql-show-config"));
        assert!(link_names().contains(&"cargo-teaql-ping"));
        assert!(link_names().contains(&"cargo-teaql-config"));
    }

    #[test]
    fn rewrites_workspace_alias_binary_name_to_subcommand() {
        let args = vec![
            OsString::from("/tmp/bin/cargo-teaql-gen-workspace"),
            OsString::from("model.yml"),
        ];

        let rewritten = rewrite_args_for_alias(args);

        assert_eq!(rewritten[0], OsString::from("teaql"));
        assert_eq!(rewritten[1], OsString::from("gen-workspace"));
        assert_eq!(rewritten[2], OsString::from("model.yml"));
    }
}
