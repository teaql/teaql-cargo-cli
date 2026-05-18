use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use dialoguer::Input;
use serde::{Deserialize, Serialize};

/// Where a resolved config value came from (highest priority first).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSource {
    Cli,
    Env,
    ConfigFile,
    Default,
}

impl ConfigSource {
    pub fn label(self) -> &'static str {
        match self {
            ConfigSource::Cli => "cli flag",
            ConfigSource::Env => "environment variable",
            ConfigSource::ConfigFile => "~/.teaql/config.yml",
            ConfigSource::Default => "built-in default",
        }
    }
}

/// Values read from environment variables.
/// Each field is `Some` only if the corresponding env var is set.
#[derive(Debug, Clone, Default)]
pub struct EnvConfig {
    pub endpoint_prefix: Option<String>,
    pub service_url: Option<String>,
    pub license_file: Option<PathBuf>,
    pub build_dir: Option<PathBuf>,
    pub timeout_seconds: Option<u64>,
}

impl EnvConfig {
    pub fn from_env() -> Self {
        Self {
            endpoint_prefix: env::var("TEAQL_ENDPOINT_PREFIX").ok(),
            service_url: env::var("TEAQL_SERVICE_URL").ok(),
            license_file: env::var("TEAQL_LICENSE_FILE").ok().map(PathBuf::from),
            build_dir: env::var("TEAQL_BUILD_DIR").ok().map(PathBuf::from),
            timeout_seconds: env::var("TEAQL_TIMEOUT_SECONDS")
                .ok()
                .and_then(|v| v.parse().ok()),
        }
    }

    /// Returns `true` if any env var was set.
    pub fn is_empty(&self) -> bool {
        self.endpoint_prefix.is_none()
            && self.service_url.is_none()
            && self.license_file.is_none()
            && self.build_dir.is_none()
            && self.timeout_seconds.is_none()
    }
}

const DEFAULT_ENDPOINT_PREFIX: &str = "https://api.teaql.io/latest/";
const DEFAULT_BUILD_DIR: &str = "build";
const DEFAULT_TIMEOUT_SECONDS: u64 = 300;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeaqlConfig {
    #[serde(default = "default_endpoint_prefix", alias = "service_url")]
    pub endpoint_prefix: String,
    #[serde(default)]
    pub license_file: Option<PathBuf>,
    #[serde(default = "default_build_dir")]
    pub build_dir: PathBuf,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone)]
pub struct ConfigOverrides {
    pub endpoint_prefix: Option<String>,
    pub service_url: Option<String>,
    pub license_file: Option<PathBuf>,
    pub build_dir: Option<PathBuf>,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub endpoint_prefix: String,
    pub license_file: PathBuf,
    /// `true` when no license was configured and the bundled `public.LICENSE` is used.
    pub is_default_license: bool,
    pub build_dir: PathBuf,
    pub timeout_seconds: u64,
}

impl Default for TeaqlConfig {
    fn default() -> Self {
        Self {
            endpoint_prefix: default_endpoint_prefix(),
            license_file: None,
            build_dir: default_build_dir(),
            timeout_seconds: default_timeout_seconds(),
        }
    }
}

impl TeaqlConfig {
    pub fn load() -> Result<Self> {
        let path = config_file_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let mut config: Self = serde_yaml::from_str(&content)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        config.endpoint_prefix = normalize_endpoint_prefix(config.endpoint_prefix);
        Ok(config)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        let yaml = serde_yaml::to_string(self)?;
        fs::write(path, yaml).with_context(|| format!("failed to write {}", path.display()))?;
        Ok(())
    }

    pub fn resolve(
        &self,
        overrides: ConfigOverrides,
        env: &EnvConfig,
        cwd: &Path,
    ) -> ResolvedConfig {
        // ── endpoint_prefix: cli > env > config.yml > default ──
        let (endpoint_prefix, endpoint_prefix_source) = if let Some(v) = overrides.endpoint_prefix {
            (normalize_endpoint_prefix(v), ConfigSource::Cli)
        } else if let Some(v) = overrides.service_url {
            (normalize_endpoint_prefix(v), ConfigSource::Cli)
        } else if let Some(ref v) = env.endpoint_prefix {
            (normalize_endpoint_prefix(v.clone()), ConfigSource::Env)
        } else if let Some(ref v) = env.service_url {
            (normalize_endpoint_prefix(v.clone()), ConfigSource::Env)
        } else {
            (
                normalize_endpoint_prefix(self.endpoint_prefix.clone()),
                ConfigSource::ConfigFile,
            )
        };

        // ── license_file: cli > env > config.yml > default ──
        let (license_file, is_default_license, license_source) =
            if let Some(p) = overrides.license_file {
                (normalize_path(p, cwd), false, ConfigSource::Cli)
            } else if let Some(ref p) = env.license_file {
                (normalize_path(p.clone(), cwd), false, ConfigSource::Env)
            } else if let Some(ref p) = self.license_file {
                (
                    normalize_path(p.clone(), cwd),
                    false,
                    ConfigSource::ConfigFile,
                )
            } else {
                (default_license_path(), true, ConfigSource::Default)
            };

        // ── build_dir: cli > env > config.yml > default ──
        let (build_dir, build_dir_source) = if let Some(p) = overrides.build_dir {
            (normalize_path(p, cwd), ConfigSource::Cli)
        } else if let Some(ref p) = env.build_dir {
            (normalize_path(p.clone(), cwd), ConfigSource::Env)
        } else {
            (
                normalize_path(self.build_dir.clone(), cwd),
                ConfigSource::ConfigFile,
            )
        };

        // ── timeout_seconds: cli > env > config.yml > default ──
        let (timeout_seconds, timeout_source) = if let Some(v) = overrides.timeout_seconds {
            (v, ConfigSource::Cli)
        } else if let Some(v) = env.timeout_seconds {
            (v, ConfigSource::Env)
        } else {
            (self.timeout_seconds, ConfigSource::ConfigFile)
        };

        // ── print sources ──
        println!();
        println!("  config (precedence: cli > env > config.yml > default):");
        println!(
            "    endpoint_prefix = {}  (from: {})",
            endpoint_prefix,
            endpoint_prefix_source.label(),
        );
        println!(
            "    license_file  = {}  (from: {})",
            license_file.display(),
            license_source.label(),
        );
        println!(
            "    build_dir     = {}  (from: {})",
            build_dir.display(),
            build_dir_source.label(),
        );
        println!(
            "    timeout_seconds = {}  (from: {})",
            timeout_seconds,
            timeout_source.label(),
        );
        println!();

        ResolvedConfig {
            endpoint_prefix,
            license_file,
            is_default_license,
            build_dir,
            timeout_seconds,
        }
    }
}

pub fn config_file_path() -> Result<PathBuf> {
    let home = env::var_os("HOME").context("HOME environment variable is not set")?;
    Ok(config_file_path_from_home(Path::new(&home)))
}

pub fn run_wizard(existing: TeaqlConfig) -> Result<TeaqlConfig> {
    let endpoint_prefix = Input::new()
        .with_prompt("TeaQL endpoint prefix")
        .default(existing.endpoint_prefix)
        .interact_text()?;

    let license_default = existing
        .license_file
        .unwrap_or_else(default_license_path)
        .display()
        .to_string();
    let license_file = Input::new()
        .with_prompt("License file path")
        .default(license_default)
        .interact_text()?;

    let build_dir = Input::new()
        .with_prompt("Build output directory")
        .default(existing.build_dir.display().to_string())
        .interact_text()?;

    let timeout_seconds = Input::new()
        .with_prompt("Request timeout (seconds)")
        .default(existing.timeout_seconds)
        .interact_text()?;

    Ok(TeaqlConfig {
        endpoint_prefix: normalize_endpoint_prefix(endpoint_prefix),
        license_file: Some(PathBuf::from(license_file)),
        build_dir: PathBuf::from(build_dir),
        timeout_seconds,
    })
}

fn default_endpoint_prefix() -> String {
    DEFAULT_ENDPOINT_PREFIX.to_string()
}

pub fn normalize_endpoint_prefix(value: String) -> String {
    let mut trimmed = value.trim().trim_end_matches('/').to_string();
    if trimmed.ends_with("/generate") {
        trimmed.truncate(trimmed.len() - "/generate".len());
    }
    format!("{}/", trimmed.trim_end_matches('/'))
}

fn default_build_dir() -> PathBuf {
    PathBuf::from(DEFAULT_BUILD_DIR)
}

fn default_timeout_seconds() -> u64 {
    DEFAULT_TIMEOUT_SECONDS
}

fn default_license_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("public.LICENSE")
}

fn normalize_path(path: PathBuf, cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

fn config_file_path_from_home(home: &Path) -> PathBuf {
    home.join(".teaql").join("config.yml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_file_path_uses_home_directory() {
        let path = config_file_path_from_home(Path::new("/tmp/alice"));
        assert_eq!(path, PathBuf::from("/tmp/alice/.teaql/config.yml"));
    }

    #[test]
    fn resolve_uses_defaults_and_normalizes_relative_paths() {
        let cwd = Path::new("/workspace/project");
        let config = TeaqlConfig {
            endpoint_prefix: "https://example.com/latest/".to_string(),
            license_file: Some(PathBuf::from("licenses/public.LICENSE")),
            build_dir: PathBuf::from("dist"),
            timeout_seconds: 42,
        };

        let resolved = config.resolve(
            ConfigOverrides {
                endpoint_prefix: None,
                service_url: None,
                license_file: None,
                build_dir: None,
                timeout_seconds: None,
            },
            &EnvConfig::default(),
            cwd,
        );

        assert_eq!(resolved.endpoint_prefix, "https://example.com/latest/");
        assert_eq!(
            resolved.license_file,
            PathBuf::from("/workspace/project/licenses/public.LICENSE")
        );
        assert_eq!(resolved.build_dir, PathBuf::from("/workspace/project/dist"));
        assert_eq!(resolved.timeout_seconds, 42);
    }

    #[test]
    fn resolve_applies_overrides() {
        let cwd = Path::new("/workspace/project");
        let config = TeaqlConfig::default();

        let resolved = config.resolve(
            ConfigOverrides {
                endpoint_prefix: Some("https://override.test/latest".to_string()),
                service_url: None,
                license_file: Some(PathBuf::from("/tmp/license.txt")),
                build_dir: Some(PathBuf::from("custom-build")),
                timeout_seconds: Some(15),
            },
            &EnvConfig::default(),
            cwd,
        );

        assert_eq!(resolved.endpoint_prefix, "https://override.test/latest/");
        assert_eq!(resolved.license_file, PathBuf::from("/tmp/license.txt"));
        assert_eq!(
            resolved.build_dir,
            PathBuf::from("/workspace/project/custom-build")
        );
        assert_eq!(resolved.timeout_seconds, 15);
    }

    #[test]
    fn resolve_env_overrides_config_file() {
        let cwd = Path::new("/workspace/project");
        let config = TeaqlConfig {
            endpoint_prefix: "https://config.file/latest/".to_string(),
            license_file: None,
            build_dir: PathBuf::from("build"),
            timeout_seconds: 300,
        };

        let env = EnvConfig {
            endpoint_prefix: Some("https://env.var/latest".to_string()),
            service_url: None,
            license_file: None,
            build_dir: None,
            timeout_seconds: None,
        };

        let resolved = config.resolve(
            ConfigOverrides {
                endpoint_prefix: None,
                service_url: None,
                license_file: None,
                build_dir: None,
                timeout_seconds: None,
            },
            &env,
            cwd,
        );

        assert_eq!(resolved.endpoint_prefix, "https://env.var/latest/");
        assert_eq!(
            resolved.build_dir,
            PathBuf::from("/workspace/project/build")
        );
    }

    #[test]
    fn resolve_cli_overrides_env() {
        let cwd = Path::new("/workspace/project");
        let config = TeaqlConfig::default();
        let env = EnvConfig {
            endpoint_prefix: Some("https://env.var/latest".to_string()),
            service_url: None,
            license_file: None,
            build_dir: None,
            timeout_seconds: None,
        };

        let resolved = config.resolve(
            ConfigOverrides {
                endpoint_prefix: Some("https://cli.flag/latest".to_string()),
                service_url: None,
                license_file: None,
                build_dir: None,
                timeout_seconds: None,
            },
            &env,
            cwd,
        );

        assert_eq!(resolved.endpoint_prefix, "https://cli.flag/latest/");
    }

    #[test]
    fn legacy_service_url_is_normalized_to_endpoint_prefix() {
        let cwd = Path::new("/workspace/project");
        let config = TeaqlConfig::default();

        let resolved = config.resolve(
            ConfigOverrides {
                endpoint_prefix: None,
                service_url: Some("https://legacy.test/latest/generate".to_string()),
                license_file: None,
                build_dir: None,
                timeout_seconds: None,
            },
            &EnvConfig::default(),
            cwd,
        );

        assert_eq!(resolved.endpoint_prefix, "https://legacy.test/latest/");
    }

    #[test]
    fn deserialize_legacy_service_url_config_key() {
        let config: TeaqlConfig = serde_yaml::from_str(
            r#"
service_url: https://legacy.config/latest/generate
build_dir: build
timeout_seconds: 300
"#,
        )
        .unwrap();

        assert_eq!(
            config.endpoint_prefix,
            "https://legacy.config/latest/generate"
        );
        assert_eq!(
            normalize_endpoint_prefix(config.endpoint_prefix),
            "https://legacy.config/latest/"
        );
    }
}
