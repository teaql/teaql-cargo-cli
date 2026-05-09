use std::{
    fs::{self, File},
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use reqwest::blocking::{Client, multipart};
use tempfile::NamedTempFile;
use walkdir::WalkDir;
use zip::{
    CompressionMethod, ZipArchive, ZipWriter,
    write::{ExtendedFileOptions, FileOptions},
};

use crate::config::ResolvedConfig;

pub fn generate(input: &Path, scope: Option<&str>, config: &ResolvedConfig) -> Result<()> {
    if !input.exists() {
        bail!("input does not exist: {}", input.display());
    }
    if !config.license_file.exists() {
        bail!(
            "license file does not exist: {}",
            config.license_file.display()
        );
    }

    fs::create_dir_all(&config.build_dir)
        .with_context(|| format!("failed to create {}", config.build_dir.display()))?;

    let upload_path = prepare_upload(input)?;
    let zip_bytes = request_generation(&upload_path, scope, config)?;
    let archive_path = config.build_dir.join("domain.zip");
    fs::write(&archive_path, &zip_bytes)
        .with_context(|| format!("failed to write {}", archive_path.display()))?;

    extract_zip(&zip_bytes, &config.build_dir)?;

    let error_file = config.build_dir.join("error.txt");
    if error_file.exists() {
        let content = fs::read_to_string(&error_file)
            .with_context(|| format!("failed to read {}", error_file.display()))?;
        bail!(content.trim().to_string());
    }

    println!("generated output in {}", config.build_dir.display());
    println!("archive saved to {}", archive_path.display());
    Ok(())
}

fn prepare_upload(input: &Path) -> Result<PathBuf> {
    if input.is_file() {
        return Ok(input.to_path_buf());
    }

    let mut temp = NamedTempFile::new().context("failed to create temp zip file")?;
    zip_directory(input, temp.as_file_mut())?;
    Ok(temp.into_temp_path().keep()?)
}

fn request_generation(
    upload_path: &Path,
    scope: Option<&str>,
    config: &ResolvedConfig,
) -> Result<Vec<u8>> {
    let client = Client::builder()
        .timeout(Duration::from_secs(config.timeout_seconds))
        .build()
        .context("failed to build HTTP client")?;

    let upload_name = upload_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("model.zip")
        .to_string();
    let license_name = config
        .license_file
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("public.LICENSE")
        .to_string();

    let file_part = multipart::Part::bytes(
        fs::read(upload_path)
            .with_context(|| format!("failed to read {}", upload_path.display()))?,
    )
    .file_name(upload_name);

    let license_part = multipart::Part::bytes(
        fs::read(&config.license_file)
            .with_context(|| format!("failed to read {}", config.license_file.display()))?,
    )
    .file_name(license_name);

    let mut form = multipart::Form::new()
        .part("file", file_part)
        .part("licenseFile", license_part);

    if let Some(scope) = scope {
        form = form.text("scope", scope.to_string());
    }

    println!("using {}", config.service_url);
    let response = client
        .post(&config.service_url)
        .multipart(form)
        .send()
        .with_context(|| format!("request failed: {}", config.service_url))?
        .error_for_status()
        .with_context(|| format!("service returned error: {}", config.service_url))?;

    Ok(response.bytes()?.to_vec())
}

fn extract_zip(zip_bytes: &[u8], output_dir: &Path) -> Result<()> {
    let reader = Cursor::new(zip_bytes);
    let mut archive = ZipArchive::new(reader).context("response is not a valid zip archive")?;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let enclosed = match entry.enclosed_name() {
            Some(path) => path.to_owned(),
            None => continue,
        };
        let output_path = output_dir.join(enclosed);

        if entry.name().ends_with('/') {
            fs::create_dir_all(&output_path)
                .with_context(|| format!("failed to create {}", output_path.display()))?;
            continue;
        }

        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let mut file = File::create(&output_path)
            .with_context(|| format!("failed to create {}", output_path.display()))?;
        io::copy(&mut entry, &mut file)
            .with_context(|| format!("failed to extract {}", output_path.display()))?;
    }

    Ok(())
}

fn zip_directory(directory: &Path, writer: &mut File) -> Result<()> {
    let mut zip = ZipWriter::new(writer);
    let options: FileOptions<'_, ExtendedFileOptions> =
        FileOptions::default().compression_method(CompressionMethod::Deflated);

    for entry in WalkDir::new(directory) {
        let entry = entry?;
        let path = entry.path();
        let name = path
            .strip_prefix(directory)
            .with_context(|| format!("failed to relativize {}", path.display()))?;

        if name.as_os_str().is_empty() {
            continue;
        }

        let name_string = name.to_string_lossy().replace('\\', "/");
        if entry.file_type().is_dir() {
            zip.add_directory(format!("{name_string}/"), options.clone())?;
            continue;
        }

        zip.start_file(name_string, options.clone())?;
        let mut file =
            File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)?;
        zip.write_all(&buffer)?;
    }

    zip.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use tempfile::tempdir;

    #[test]
    fn prepare_upload_returns_original_file_for_file_input() {
        let temp = tempdir().unwrap();
        let input = temp.path().join("model.yml");
        fs::write(&input, "name: demo").unwrap();

        let upload = prepare_upload(&input).unwrap();

        assert_eq!(upload, input);
    }

    #[test]
    fn prepare_upload_zips_directory_contents() {
        let temp = tempdir().unwrap();
        let input_dir = temp.path().join("model");
        fs::create_dir_all(input_dir.join("nested")).unwrap();
        fs::write(input_dir.join("root.txt"), "root").unwrap();
        fs::write(input_dir.join("nested").join("child.txt"), "child").unwrap();

        let upload = prepare_upload(&input_dir).unwrap();
        let zip_bytes = fs::read(upload).unwrap();
        let mut archive = ZipArchive::new(Cursor::new(zip_bytes)).unwrap();

        let mut root = archive.by_name("root.txt").unwrap();
        let mut root_content = String::new();
        root.read_to_string(&mut root_content).unwrap();
        drop(root);

        let mut child = archive.by_name("nested/child.txt").unwrap();
        let mut child_content = String::new();
        child.read_to_string(&mut child_content).unwrap();

        assert_eq!(root_content, "root");
        assert_eq!(child_content, "child");
    }

    #[test]
    fn extract_zip_writes_files_to_output_directory() {
        let temp = tempdir().unwrap();
        let zip_path = temp.path().join("archive.zip");
        let mut file = File::create(&zip_path).unwrap();
        zip_directory(create_fixture_tree(temp.path()).as_path(), &mut file).unwrap();
        drop(file);

        let output_dir = temp.path().join("out");
        let zip_bytes = fs::read(zip_path).unwrap();
        extract_zip(&zip_bytes, &output_dir).unwrap();

        assert_eq!(
            fs::read_to_string(output_dir.join("root.txt")).unwrap(),
            "root"
        );
        assert_eq!(
            fs::read_to_string(output_dir.join("nested").join("child.txt")).unwrap(),
            "child"
        );
    }

    fn create_fixture_tree(base: &Path) -> PathBuf {
        let fixture = base.join("fixture");
        fs::create_dir_all(fixture.join("nested")).unwrap();
        fs::write(fixture.join("root.txt"), "root").unwrap();
        fs::write(fixture.join("nested").join("child.txt"), "child").unwrap();
        fixture
    }
}
