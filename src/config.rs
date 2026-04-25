use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use dialoguer::Input;
use serde::{Deserialize, Serialize};

const DEFAULT_SERVICE_URL: &str =
    "http://springboot.teaql-gen-code.1496855407387739.cn-chengdu.fc.devsapp.net/generate";
const DEFAULT_BUILD_DIR: &str = "build";
const DEFAULT_TIMEOUT_SECONDS: u64 = 300;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeaqlConfig {
    #[serde(default = "default_service_url")]
    pub service_url: String,
    #[serde(default)]
    pub license_file: Option<PathBuf>,
    #[serde(default = "default_build_dir")]
    pub build_dir: PathBuf,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone)]
pub struct ConfigOverrides {
    pub service_url: Option<String>,
    pub license_file: Option<PathBuf>,
    pub build_dir: Option<PathBuf>,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub service_url: String,
    pub license_file: PathBuf,
    pub build_dir: PathBuf,
    pub timeout_seconds: u64,
}

impl Default for TeaqlConfig {
    fn default() -> Self {
        Self {
            service_url: default_service_url(),
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
        let config: Self = serde_yaml::from_str(&content)
            .with_context(|| format!("failed to parse {}", path.display()))?;
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

    pub fn resolve(&self, overrides: ConfigOverrides, cwd: &Path) -> ResolvedConfig {
        let service_url = overrides
            .service_url
            .unwrap_or_else(|| self.service_url.clone());
        let build_dir = normalize_path(
            overrides
                .build_dir
                .unwrap_or_else(|| self.build_dir.clone()),
            cwd,
        );
        let timeout_seconds = overrides.timeout_seconds.unwrap_or(self.timeout_seconds);
        let configured_license = overrides.license_file.or_else(|| self.license_file.clone());
        let license_file = configured_license
            .map(|path| normalize_path(path, cwd))
            .unwrap_or_else(default_license_path);

        ResolvedConfig {
            service_url,
            license_file,
            build_dir,
            timeout_seconds,
        }
    }
}

pub fn config_file_path() -> Result<PathBuf> {
    let home = env::var_os("HOME").context("HOME environment variable is not set")?;
    Ok(PathBuf::from(home).join(".teaql").join("config.yml"))
}

pub fn run_wizard(existing: TeaqlConfig) -> Result<TeaqlConfig> {
    let service_url = Input::new()
        .with_prompt("TeaQL service URL")
        .default(existing.service_url)
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
        service_url,
        license_file: Some(PathBuf::from(license_file)),
        build_dir: PathBuf::from(build_dir),
        timeout_seconds,
    })
}

fn default_service_url() -> String {
    DEFAULT_SERVICE_URL.to_string()
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
