pub mod cli;
pub mod config;
pub mod eval;
pub mod generator;
pub mod service;

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use clap::Parser;
use cli::{CheckArgs, Cli, Commands, EvalArgs, DynamicArgs, InstallLinksArgs};
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
    let command = cli.command.unwrap_or_else(|| Commands::Dynamic(vec![OsString::from("services")]));
    match command {
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
        Commands::Ping(args) => run_ping(args, cli.cwd)?,
        Commands::Eval(args) => {
            let code = run_eval(args, cli.cwd)?;
            std::process::exit(code);
        }
        Commands::Check(args) => {
            let code = run_check(args, cli.cwd)?;
            std::process::exit(code);
        }
        Commands::Dynamic(args) => {
            if args.is_empty() {
                bail!("no target specified");
            }
            let target = args[0].to_string_lossy().to_string();
            
            let parsed_args = args.into_iter().skip(1).collect::<Vec<_>>();
            let dyn_args = DynamicArgs::parse_from(parsed_args);
            
            let config = TeaqlConfig::load()?;
            let env = EnvConfig::from_env();
            let overrides = ConfigOverrides {
                endpoint_prefix: dyn_args.endpoint_prefix,
                service_url: dyn_args.service_url,
                api_key: dyn_args.api_key,
                build_dir: dyn_args.output,
                timeout_seconds: dyn_args.timeout_seconds,
            };
            let resolved = config.resolve(overrides, &env, &cli.cwd);
            
            let mut all_paths = vec![target.clone()];
            let mut input = dyn_args.input.clone();

            // Backward compatibility: If no --input is specified, but there is a trailing positional argument
            // that looks like a model file, warn the user and use it as the input.
            if input.is_none() && !dyn_args.paths.is_empty() {
                let last = &dyn_args.paths[dyn_args.paths.len() - 1];
                let path = Path::new(last);
                if path.exists() && (last.ends_with(".xml") || last.ends_with(".ksml") || last.ends_with(".yml")) {
                    eprintln!("Warning: Implicit model file '{}' detected as positional argument.", last);
                    eprintln!("Warning: Please use `--input {}` in the future.", last);
                    input = Some(PathBuf::from(last));
                    let mut paths_without_last = dyn_args.paths.clone();
                    paths_without_last.pop();
                    all_paths.extend(paths_without_last);
                } else {
                    all_paths.extend(dyn_args.paths);
                }
            } else {
                all_paths.extend(dyn_args.paths);
            }

            let input_path = input.unwrap_or_else(|| PathBuf::from("."));

            let get_targets = ["version", "services"];
            if all_paths.len() == 1 && get_targets.contains(&all_paths[0].as_str()) {
                service::dynamic_get(&resolved, &all_paths[0]).with_context(|| {
                    format!("Command failed. Hint: If '{}' is not a valid remote command, run `cargo teaql services`.", all_paths[0])
                })?;
                return Ok(());
            }

            if all_paths.len() == 1 {
                // Single target (e.g. `rust-app-console`): POST to `/generate` with scope = target
                generator::generate(&input_path, "generate", Some(&all_paths[0]), &resolved).with_context(|| {
                    format!("Command failed. Hint: If '{}' is not a valid generation target, run `cargo teaql services` to see available services.", all_paths[0])
                })?;
            } else {
                // Multi-segment dynamic target (e.g. `assist task create`): POST directly to `/assist/task/create`
                let endpoint_path = all_paths.join("/");
                generator::generate(&input_path, &endpoint_path, None, &resolved).with_context(|| {
                    format!("Command failed on dynamic endpoint: {}", endpoint_path)
                })?;
            }
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
        api_key: None,
        build_dir: None,
        timeout_seconds: args.timeout_seconds,
    };
    let resolved = config.resolve(overrides, &env, &cwd);
    eval::evaluate(&args.input, &args, &resolved)
}

fn run_ping(args: cli::ServiceArgs, cwd: PathBuf) -> Result<()> {
    let config = TeaqlConfig::load()?;
    let env = EnvConfig::from_env();
    let overrides = ConfigOverrides {
        endpoint_prefix: args.endpoint_prefix,
        service_url: args.service_url,
        api_key: args.api_key,
        build_dir: Some(std::env::temp_dir().join("teaql-ping")),
        timeout_seconds: args.timeout_seconds,
    };
    let resolved = config.resolve(overrides, &env, &cwd);
    service::ping(&resolved)
}

// Removed hardcoded run_generate, run_version, run_list_services

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
        "cargo-teaql-check" => Some("check"),
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
        "cargo-teaql-eval",
        "cargo-teaql-check",
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

fn run_check(args: CheckArgs, cwd: PathBuf) -> Result<i32> {
    use std::io::{BufRead, BufReader};

    let mut command = std::process::Command::new("cargo");
    command.arg("check").arg("--message-format=json");
    for cargo_arg in args.cargo_args {
        command.arg(cargo_arg);
    }
    command.current_dir(&cwd);
    command.stdout(std::process::Stdio::piped());

    let mut child = command.spawn().context("failed to spawn cargo check")?;
    let stdout = child.stdout.take().context("failed to take stdout")?;
    let reader = BufReader::new(stdout);

    for line_res in reader.lines() {
        let line = match line_res {
            Ok(l) => l,
            Err(_) => break,
        };

        if let Ok(cargo_json) = serde_json::from_str::<CargoJson>(&line) {
            if cargo_json.reason == "compiler-message" {
                if let Some(diagnostic) = cargo_json.message {
                    let mut mapped = false;
                    for span in &diagnostic.spans {
                        if span.is_primary {
                            if let Some((xml_path, xml_line)) = try_map_span(&cwd, span) {
                                print_mapped_error(
                                    &diagnostic.level,
                                    &diagnostic.message,
                                    &xml_path,
                                    xml_line,
                                    span,
                                    &cwd,
                                );
                                mapped = true;
                                break;
                            }
                        }
                    }
                    if !mapped {
                        if let Some(rendered) = diagnostic.rendered {
                            eprint!("{}", rendered);
                        }
                    }
                }
            }
        }
    }

    let status = child.wait()?;
    Ok(status.code().unwrap_or(1))
}

#[derive(serde::Deserialize, Debug)]
struct CargoJson {
    reason: String,
    message: Option<Diagnostic>,
}

#[derive(serde::Deserialize, Debug)]
struct Diagnostic {
    message: String,
    level: String,
    spans: Vec<DiagnosticSpan>,
    rendered: Option<String>,
}

#[derive(serde::Deserialize, Debug)]
struct DiagnosticSpan {
    file_name: String,
    line_start: usize,
    column_start: usize,
    is_primary: bool,
}

fn try_map_span(cwd: &Path, span: &DiagnosticSpan) -> Option<(PathBuf, usize)> {
    let file_path = cwd.join(&span.file_name);
    if !file_path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&file_path).ok()?;
    let lines: Vec<&str> = content.lines().collect();

    let mut current_idx = span.line_start.checked_sub(1)?;
    while current_idx < lines.len() {
        let line = lines[current_idx].trim();
        if line.starts_with("// @source ") {
            let parts = line.strip_prefix("// @source ")?;
            let mut parts_split = parts.split(':');
            let path_str = parts_split.next()?;
            let line_str = parts_split.next()?;
            let line_num = line_str.parse::<usize>().ok()?;
            return Some((PathBuf::from(path_str), line_num));
        }
        if current_idx == 0 {
            break;
        }
        current_idx -= 1;
    }
    None
}

fn print_mapped_error(
    level: &str,
    message: &str,
    xml_path: &Path,
    xml_line: usize,
    span: &DiagnosticSpan,
    cwd: &Path,
) {
    eprintln!("{}: {}", level, message);
    eprintln!("  --> {}:{}", xml_path.display(), xml_line);

    let full_xml_path = cwd.join(xml_path);
    if full_xml_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&full_xml_path) {
            let lines: Vec<&str> = content.lines().collect();
            if xml_line > 0 && xml_line <= lines.len() {
                let line_content = lines[xml_line - 1];
                eprintln!("   |");
                eprintln!("{:3} | {}", xml_line, line_content);
                eprintln!("   | (error generated from here)");
            }
        }
    }
    eprintln!("   =");
    eprintln!(
        "   = note: generated Rust code in {}:{}:{} failed to compile",
        span.file_name, span.line_start, span.column_start
    );
    eprintln!();
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
        // "cargo teaql-version" -> argv = ["/path/to/cargo-teaql-version", "teaql-version"]
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
