#![allow(dead_code)]

use flate2::read::GzDecoder;
use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};
use tar::Archive;
use zip::ZipArchive;

use crate::download::VerifiedDownload;
use crate::paths;

#[derive(Clone, Debug)]
pub struct ExtractedArchive {
    pub path: PathBuf,
}

pub fn extract_to_staging(
    download: &VerifiedDownload,
    install_dir: &Path,
) -> Result<ExtractedArchive, String> {
    let staging_root = paths::staging_dir(install_dir);
    let extract_dir = staging_root.join("extracted");

    if extract_dir.exists() {
        fs::remove_dir_all(&extract_dir).map_err(|error| {
            format!(
                "Could not clear staging directory {}: {error}",
                extract_dir.display()
            )
        })?;
    }
    fs::create_dir_all(&extract_dir).map_err(|error| {
        format!(
            "Could not create staging directory {}: {error}",
            extract_dir.display()
        )
    })?;

    let archive_name = download
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("Archive path has no file name: {}", download.path.display()))?;

    if archive_name.ends_with(".tar.gz") || archive_name.ends_with(".tgz") {
        extract_tar_gz(&download.path, &extract_dir)?;
    } else if archive_name.ends_with(".zip") {
        extract_zip(&download.path, &extract_dir)?;
    } else {
        return Err(format!("Unsupported archive format: {archive_name}"));
    }

    Ok(ExtractedArchive { path: extract_dir })
}

fn extract_tar_gz(archive_path: &Path, destination: &Path) -> Result<(), String> {
    let file = File::open(archive_path)
        .map_err(|error| format!("Could not open {}: {error}", archive_path.display()))?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|error| format!("Could not read {}: {error}", archive_path.display()))?;

    for entry in entries {
        let mut entry = entry.map_err(|error| format!("Could not read tar entry: {error}"))?;
        let entry_type = entry.header().entry_type();
        if !(entry_type.is_file() || entry_type.is_dir()) {
            let entry_path = entry
                .path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| "<unreadable path>".to_string());
            return Err(format!("Unsupported tar entry type for {}", entry_path));
        }

        let relative_path = safe_relative_path(
            &entry
                .path()
                .map_err(|error| format!("Could not read tar entry path: {error}"))?,
        )?;
        let output_path = destination.join(relative_path);

        if entry_type.is_dir() {
            fs::create_dir_all(&output_path).map_err(|error| {
                format!(
                    "Could not create directory {}: {error}",
                    output_path.display()
                )
            })?;
        } else {
            if let Some(parent) = output_path.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    format!("Could not create directory {}: {error}", parent.display())
                })?;
            }
            entry
                .unpack(&output_path)
                .map_err(|error| format!("Could not extract {}: {error}", output_path.display()))?;
        }
    }

    Ok(())
}

fn extract_zip(archive_path: &Path, destination: &Path) -> Result<(), String> {
    let file = File::open(archive_path)
        .map_err(|error| format!("Could not open {}: {error}", archive_path.display()))?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| format!("Could not read {}: {error}", archive_path.display()))?;

    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|error| format!("Could not read zip entry {index}: {error}"))?;
        let file_name = file
            .name()
            .map(|name| name.to_string())
            .unwrap_or_else(|_| "<unreadable path>".to_string());
        let Some(enclosed_name) = file.enclosed_name() else {
            return Err(format!("Unsafe zip entry path: {file_name}"));
        };
        let output_path = destination.join(enclosed_name);

        if file.is_dir() {
            fs::create_dir_all(&output_path).map_err(|error| {
                format!(
                    "Could not create directory {}: {error}",
                    output_path.display()
                )
            })?;
        } else if file.is_file() {
            if let Some(parent) = output_path.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    format!("Could not create directory {}: {error}", parent.display())
                })?;
            }
            let mut output = File::create(&output_path)
                .map_err(|error| format!("Could not create {}: {error}", output_path.display()))?;
            io::copy(&mut file, &mut output)
                .map_err(|error| format!("Could not extract {}: {error}", output_path.display()))?;
        } else {
            return Err(format!("Unsupported zip entry type: {file_name}"));
        }
    }

    Ok(())
}

fn safe_relative_path(path: &Path) -> Result<PathBuf, String> {
    let mut output = PathBuf::new();

    for component in path.components() {
        match component {
            Component::Normal(part) => output.push(part),
            Component::CurDir => {}
            _ => return Err(format!("Unsafe archive entry path: {}", path.display())),
        }
    }

    if output.as_os_str().is_empty() {
        return Err("Archive entry path is empty".to_string());
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;
    use tar::{Builder, Header};
    use tempfile::tempdir;
    use zip::write::{SimpleFileOptions, ZipWriter};

    #[test]
    fn extracts_tar_gz_to_staging() {
        let temp = tempdir().unwrap();
        let archive_path = temp.path().join("sample.tar.gz");
        write_tar_gz(&archive_path, "game/version.json", br#"{"version":"V1"}"#);
        let download = test_download(archive_path);

        let extracted = extract_to_staging(&download, temp.path()).unwrap();

        assert_eq!(
            fs::read_to_string(extracted.path.join("game/version.json")).unwrap(),
            r#"{"version":"V1"}"#
        );
    }

    #[test]
    fn extracts_zip_to_staging() {
        let temp = tempdir().unwrap();
        let archive_path = temp.path().join("sample.zip");
        write_zip(&archive_path, "game/version.json", br#"{"version":"V1"}"#);
        let download = test_download(archive_path);

        let extracted = extract_to_staging(&download, temp.path()).unwrap();

        assert_eq!(
            fs::read_to_string(extracted.path.join("game/version.json")).unwrap(),
            r#"{"version":"V1"}"#
        );
    }

    #[test]
    fn rejects_unsupported_archive_format() {
        let temp = tempdir().unwrap();
        let archive_path = temp.path().join("sample.rar");
        fs::write(&archive_path, b"not really an archive").unwrap();
        let download = test_download(archive_path);

        let error = extract_to_staging(&download, temp.path()).unwrap_err();

        assert!(error.contains("Unsupported archive format"));
    }

    #[test]
    fn rejects_unsafe_relative_paths() {
        assert!(safe_relative_path(Path::new("game/version.json")).is_ok());
        assert!(safe_relative_path(Path::new("../escape")).is_err());
        assert!(safe_relative_path(Path::new("/absolute")).is_err());
    }

    fn test_download(path: PathBuf) -> VerifiedDownload {
        VerifiedDownload {
            path,
            sha256: "unused".to_string(),
            size: 0,
        }
    }

    fn write_tar_gz(path: &Path, entry_name: &str, contents: &[u8]) {
        let file = File::create(path).unwrap();
        let encoder = GzEncoder::new(file, Compression::default());
        let mut builder = Builder::new(encoder);
        let mut header = Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_cksum();
        builder
            .append_data(&mut header, entry_name, contents)
            .unwrap();
        builder.finish().unwrap();
    }

    fn write_zip(path: &Path, entry_name: &str, contents: &[u8]) {
        let file = File::create(path).unwrap();
        let mut writer = ZipWriter::new(file);
        writer
            .start_file(entry_name, SimpleFileOptions::default())
            .unwrap();
        writer.write_all(contents).unwrap();
        writer.finish().unwrap();
    }
}
