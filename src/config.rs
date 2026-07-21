//! Persist the coworker's Creatio credentials to `pvwatts.env` next to the exe,
//! so they sign in once per PC (auto-login on later launches).
//!
//! Security tradeoff (intentional, opt-in via the "remember" checkbox): the
//! password is stored in PLAINTEXT. The file is chmod 0600 on unix; on Windows it
//! sits in the exe's folder under the user's profile. "Forget" clears it.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

/// `pvwatts.env` next to the exe (fallback: cwd). Same file `main::load_local_env` reads.
pub fn config_path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.join("pvwatts.env");
        }
    }
    PathBuf::from("pvwatts.env")
}

fn read_pairs(path: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Ok(text) = fs::read_to_string(path) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                out.push((k.trim().to_string(), v.trim().trim_matches('"').to_string()));
            }
        }
    }
    out
}

fn write_pairs(path: &Path, pairs: &[(String, String)]) -> Result<()> {
    // Quote values so spaces/specials in a password round-trip cleanly.
    let body: String = pairs.iter().map(|(k, v)| format!("{k}=\"{v}\"\n")).collect();
    fs::write(path, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn upsert(pairs: &mut Vec<(String, String)>, key: &str, val: &str) {
    match pairs.iter_mut().find(|(k, _)| k == key) {
        Some(p) => p.1 = val.to_string(),
        None => pairs.push((key.to_string(), val.to_string())),
    }
}

/// Merge arbitrary KEY=VALUE settings into pvwatts.env, preserving other keys.
pub fn save_kv(entries: &[(&str, &str)]) -> Result<PathBuf> {
    let path = config_path();
    let mut pairs = read_pairs(&path);
    for (k, v) in entries {
        upsert(&mut pairs, k, v);
    }
    write_pairs(&path, &pairs)?;
    Ok(path)
}

/// Save (merge) the Creatio credentials, preserving other keys (e.g. the sidecar key).
pub fn save_creatio_creds(username: &str, password: &str) -> Result<PathBuf> {
    let path = config_path();
    let mut pairs = read_pairs(&path);
    upsert(&mut pairs, "CREATIO_USERNAME", username);
    upsert(&mut pairs, "CREATIO_PASSWORD", password);
    write_pairs(&path, &pairs)?;
    Ok(path)
}

/// Remove saved Creatio credentials (keeps other keys; deletes the file if empty).
pub fn forget_creatio_creds() -> Result<()> {
    let path = config_path();
    let mut pairs = read_pairs(&path);
    pairs.retain(|(k, _)| k != "CREATIO_USERNAME" && k != "CREATIO_PASSWORD");
    if pairs.is_empty() {
        let _ = fs::remove_file(&path);
    } else {
        write_pairs(&path, &pairs)?;
    }
    Ok(())
}
