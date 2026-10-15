//! Getting an indexed mod version onto disk: download, verify, extract.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use reqwest::header::USER_AGENT;
use sha2::{Digest, Sha256};

use super::index::{Entry, http_client};
use super::manifest::valid_id;
use crate::archive::extract_zip;

/// `<cache>/<id>-<version>/`, extracted from the entry's zip once its size and
/// SHA-256 match. Kept for later runs.
pub fn cached_mod(entry: &Entry, cache: &Path) -> Result<PathBuf, String> {
    let version = entry
        .parsed_version()
        .ok_or_else(|| format!("{} has no valid version", entry.id))?;
    if !valid_id(&entry.id) {
        return Err(format!("{} is not a valid mod id", entry.id));
    }
    let folder = cache.join(format!("{}-{version}", entry.id));
    if !folder.join("mod.json").is_file() {
        download_and_extract(entry, &folder)?;
    }
    Ok(folder)
}

/// Downloads the entry's zip without an account, checks its size and
/// SHA-256, and extracts it into `folder`, created next to it.
pub fn download_and_extract(entry: &Entry, folder: &Path) -> Result<(), String> {
    let parent = folder
        .parent()
        .ok_or_else(|| format!("{} has no parent folder", folder.display()))?;
    let name = folder
        .file_name()
        .ok_or_else(|| format!("{} has no name", folder.display()))?
        .to_string_lossy();
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
    let zip = parent.join(format!("{name}.zip"));
    download_verified(entry, &zip)?;
    let staging = parent.join(format!("{name}.extracting"));
    let _ = fs::remove_dir_all(&staging);
    let extracted = extract_zip(&zip, &staging).and_then(|()| {
        fs::rename(&staging, folder)
            .map_err(|error| format!("Could not move {}: {error}", staging.display()))
    });
    let _ = fs::remove_file(&zip);
    if extracted.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    extracted
}

fn download_verified(entry: &Entry, path: &Path) -> Result<(), String> {
    let label = format!("{} {}", entry.id, entry.version);
    let mut response = http_client()?
        .get(&entry.url)
        .header(USER_AGENT, "DRH-Launcher")
        .send()
        .and_then(|response| response.error_for_status())
        .map_err(|error| format!("Could not download {label}: {error}"))?;
    let result = (|| {
        let mut file = File::create(path)
            .map_err(|error| format!("Could not create {}: {error}", path.display()))?;
        let mut hasher = Sha256::new();
        let mut size = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = response
                .read(&mut buffer)
                .map_err(|error| format!("Could not download {label}: {error}"))?;
            if read == 0 {
                break;
            }
            size += read as u64;
            if size > entry.size {
                return Err(format!("{label}: larger than its {} bytes", entry.size));
            }
            hasher.update(&buffer[..read]);
            file.write_all(&buffer[..read])
                .map_err(|error| format!("Could not write {}: {error}", path.display()))?;
        }
        if size != entry.size {
            return Err(format!(
                "{label}: {size} bytes, the index says {}",
                entry.size
            ));
        }
        let sha256: String = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if sha256 != entry.sha256.to_ascii_lowercase() {
            return Err(format!("{label}: SHA-256 does not match the index"));
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}
