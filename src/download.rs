use reqwest::blocking::Client;
use reqwest::header::USER_AGENT;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::github_releases::ReleaseAsset;
use crate::paths;
use crate::release_manifest::normalize_sha256;

#[derive(Clone, Debug)]
pub struct VerifiedDownload {
    pub path: PathBuf,
    pub sha256: String,
    pub size: u64,
    pub reused: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DownloadProgress {
    pub downloaded: u64,
    pub total: u64,
}

pub fn download_and_verify_with_progress(
    asset: &ReleaseAsset,
    install_dir: &Path,
    mut on_progress: impl FnMut(DownloadProgress),
    mut on_cache_error: impl FnMut(String),
) -> Result<VerifiedDownload, String> {
    let expected_sha256 = expected_sha256(asset)?;
    let downloads_dir = paths::downloads_dir(install_dir);
    fs::create_dir_all(&downloads_dir).map_err(|error| {
        format!(
            "Could not create downloads directory {}: {error}",
            downloads_dir.display()
        )
    })?;

    let final_path = downloads_dir.join(&asset.name);
    let temp_path = downloads_dir.join(format!("{}.download", asset.name));

    if final_path.exists() {
        match verify_cached_archive(&final_path, asset, &expected_sha256) {
            Ok(download) => return Ok(download),
            Err(error) => {
                on_cache_error(format!(
                    "Discarding invalid cached archive {}: {error}",
                    final_path.display()
                ));
                let _ = fs::remove_file(&final_path);
            }
        }
    }

    let client = Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|error| format!("Could not create download client: {error}"))?;

    let mut response = client
        .get(&asset.download_url)
        .header(USER_AGENT, "DRH-Launcher")
        .send()
        .map_err(|error| format!("Could not download {}: {error}", asset.name))?
        .error_for_status()
        .map_err(|error| format!("Download failed for {}: {error}", asset.name))?;

    let mut file = File::create(&temp_path)
        .map_err(|error| format!("Could not create {}: {error}", temp_path.display()))?;

    let expected_size = asset.size;
    let mut bytes_written = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    on_progress(DownloadProgress {
        downloaded: bytes_written,
        total: expected_size,
    });
    loop {
        let bytes_read = response
            .read(&mut buffer)
            .map_err(|error| format!("Could not download {}: {error}", asset.name))?;
        if bytes_read == 0 {
            break;
        }

        file.write_all(&buffer[..bytes_read])
            .map_err(|error| format!("Could not write {}: {error}", temp_path.display()))?;
        bytes_written += bytes_read as u64;
        on_progress(DownloadProgress {
            downloaded: bytes_written,
            total: expected_size,
        });
    }
    file.flush()
        .map_err(|error| format!("Could not flush {}: {error}", temp_path.display()))?;

    if bytes_written != asset.size {
        let _ = fs::remove_file(&temp_path);
        return Err(format!(
            "Downloaded size mismatch for {}: expected {} bytes, got {} bytes",
            asset.name, asset.size, bytes_written
        ));
    }

    let actual_sha256 = sha256_file(&temp_path)
        .map_err(|error| format!("Could not hash {}: {error}", temp_path.display()))?;
    if actual_sha256 != expected_sha256 {
        let _ = fs::remove_file(&temp_path);
        return Err(format!(
            "SHA-256 mismatch for {}: expected {}, got {}",
            asset.name, expected_sha256, actual_sha256
        ));
    }

    fs::rename(&temp_path, &final_path).map_err(|error| {
        format!(
            "Could not move {} to {}: {error}",
            temp_path.display(),
            final_path.display()
        )
    })?;

    Ok(VerifiedDownload {
        path: final_path,
        sha256: actual_sha256,
        size: bytes_written,
        reused: false,
    })
}

pub fn verify_cached_archive_by_metadata(
    install_dir: &Path,
    archive_name: &str,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<VerifiedDownload, String> {
    let expected_sha256 = normalize_sha256(expected_sha256);
    if expected_size == 0 {
        return Err("Cached archive size is unknown.".to_string());
    }
    if expected_sha256.is_empty() {
        return Err("Cached archive SHA-256 is unknown.".to_string());
    }

    let archive_path = paths::downloads_dir(install_dir).join(archive_name);
    if !archive_path.exists() {
        return Err(format!(
            "Cached archive is missing: {}",
            archive_path.display()
        ));
    }

    verify_cached_archive_values(&archive_path, expected_size, &expected_sha256)
}

fn verify_cached_archive(
    path: &Path,
    asset: &ReleaseAsset,
    expected_sha256: &str,
) -> Result<VerifiedDownload, String> {
    verify_cached_archive_values(path, asset.size, expected_sha256)
}

fn verify_cached_archive_values(
    path: &Path,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<VerifiedDownload, String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("Could not inspect cached archive: {error}"))?;
    if metadata.len() != expected_size {
        return Err(format!(
            "size mismatch, expected {} bytes, got {} bytes",
            expected_size,
            metadata.len()
        ));
    }

    let actual_sha256 =
        sha256_file(path).map_err(|error| format!("Could not hash cached archive: {error}"))?;
    if actual_sha256 != expected_sha256 {
        return Err(format!(
            "SHA-256 mismatch, expected {}, got {}",
            expected_sha256, actual_sha256
        ));
    }

    Ok(VerifiedDownload {
        path: path.to_path_buf(),
        sha256: actual_sha256,
        size: metadata.len(),
        reused: true,
    })
}

pub fn update_download_cache(
    install_dir: &Path,
    archive_name: &str,
    limit: usize,
) -> Result<Vec<PathBuf>, String> {
    let downloads_dir = paths::downloads_dir(install_dir);
    fs::create_dir_all(&downloads_dir).map_err(|error| {
        format!(
            "Could not create downloads directory {}: {error}",
            downloads_dir.display()
        )
    })?;

    let mut cache = read_download_cache_index(install_dir);
    cache.retain(|name| name != archive_name);
    cache.insert(0, archive_name.to_string());
    cache.truncate(limit);

    let cache_file = paths::download_cache_index_file(install_dir);
    fs::write(&cache_file, cache.join("\n"))
        .map_err(|error| format!("Could not write {}: {error}", cache_file.display()))?;

    prune_downloads_dir(&downloads_dir, &cache)
}

fn read_download_cache_index(install_dir: &Path) -> Vec<String> {
    fs::read_to_string(paths::download_cache_index_file(install_dir))
        .map(|contents| {
            contents
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn prune_downloads_dir(downloads_dir: &Path, keep: &[String]) -> Result<Vec<PathBuf>, String> {
    let mut removed = Vec::new();
    let keep = keep.iter().map(String::as_str).collect::<Vec<_>>();
    for entry in fs::read_dir(downloads_dir)
        .map_err(|error| format!("Could not read {}: {error}", downloads_dir.display()))?
    {
        let entry =
            entry.map_err(|error| format!("Could not read download cache entry: {error}"))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name == "cache.txt" || name.ends_with(".download") || keep.contains(&name) {
            continue;
        }

        fs::remove_file(&path).map_err(|error| {
            format!(
                "Could not remove cached archive {}: {error}",
                path.display()
            )
        })?;
        removed.push(path);
    }

    Ok(removed)
}

pub fn expected_sha256(asset: &ReleaseAsset) -> Result<String, String> {
    asset
        .digest
        .as_deref()
        .map(normalize_sha256)
        .ok_or_else(|| {
            format!(
                "Release asset {} does not provide a SHA-256 digest",
                asset.name
            )
        })
}

pub fn sha256_file(path: &Path) -> io::Result<String> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];

    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    Ok(hex_lower(&hasher.finalize()))
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);

    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn normalizes_expected_asset_digest() {
        let asset = test_asset(Some("sha256:abc123"), 0);

        assert_eq!(expected_sha256(&asset).unwrap(), "abc123");
    }

    #[test]
    fn requires_asset_digest() {
        let asset = test_asset(None, 0);

        let error = expected_sha256(&asset).unwrap_err();

        assert!(error.contains("does not provide a SHA-256 digest"));
    }

    #[test]
    fn hashes_file_contents() {
        let temp = tempdir().unwrap();
        let file_path = temp.path().join("sample.bin");
        fs::write(&file_path, b"drh-launcher").unwrap();

        let digest = sha256_file(&file_path).unwrap();

        assert_eq!(
            digest,
            "d7f08e6a07ee091d21cbbd9702c217f75dcb26418edd45ac3c7345f5ebc70686"
        );
    }

    #[test]
    fn verifies_matching_cached_archive() {
        let temp = tempdir().unwrap();
        let archive_path = temp.path().join("test.tar.gz");
        fs::write(&archive_path, b"cached archive").unwrap();
        let sha256 = sha256_file(&archive_path).unwrap();
        let asset = test_asset(Some(&format!("sha256:{sha256}")), 14);

        let verified = verify_cached_archive(&archive_path, &asset, &sha256).unwrap();

        assert!(verified.reused);
        assert_eq!(verified.path, archive_path);
        assert_eq!(verified.sha256, sha256);
        assert_eq!(verified.size, 14);
    }

    #[test]
    fn prunes_download_cache_to_recent_archives() {
        let temp = tempdir().unwrap();
        let downloads_dir = paths::downloads_dir(temp.path());
        fs::create_dir_all(&downloads_dir).unwrap();
        fs::write(downloads_dir.join("A.tar.gz"), b"a").unwrap();
        fs::write(downloads_dir.join("B.tar.gz"), b"b").unwrap();
        fs::write(downloads_dir.join("C.tar.gz"), b"c").unwrap();
        fs::write(
            paths::download_cache_index_file(temp.path()),
            "B.tar.gz\nC.tar.gz\n",
        )
        .unwrap();

        let removed = update_download_cache(temp.path(), "A.tar.gz", 2).unwrap();

        assert!(downloads_dir.join("A.tar.gz").exists());
        assert!(downloads_dir.join("B.tar.gz").exists());
        assert!(!downloads_dir.join("C.tar.gz").exists());
        assert_eq!(removed, vec![downloads_dir.join("C.tar.gz")]);
        assert_eq!(
            fs::read_to_string(paths::download_cache_index_file(temp.path())).unwrap(),
            "A.tar.gz\nB.tar.gz"
        );
    }

    #[test]
    fn formats_lowercase_hex() {
        assert_eq!(hex_lower(&[0x00, 0x0f, 0x10, 0xab, 0xff]), "000f10abff");
    }

    fn test_asset(digest: Option<&str>, size: u64) -> ReleaseAsset {
        ReleaseAsset {
            platform_id: "linux-x64".to_string(),
            name: "test.tar.gz".to_string(),
            download_url: "https://example.test/test.tar.gz".to_string(),
            size,
            digest: digest.map(str::to_string),
        }
    }
}
