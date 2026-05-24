#![allow(dead_code)]

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
}

pub fn download_and_verify(
    asset: &ReleaseAsset,
    install_dir: &Path,
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
    let bytes_written = io::copy(&mut response, &mut file)
        .map_err(|error| format!("Could not write {}: {error}", temp_path.display()))?;
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
    })
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
