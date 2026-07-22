//! Config + credential persistence.
//!
//! SECRETS (Creatio password, sidecar API key) are stored in the OS credential
//! vault via `keyring` — Windows Credential Manager (DPAPI-encrypted, per Windows
//! user) in production, macOS Keychain for dev. They are NEVER written to a file.
//!
//! NON-secret config (Creatio username, service URLs, PDF output root) lives in
//! `pvwatts.env` next to the exe, so a handed-off exe works without env setup and
//! auto-login knows which user to look up. Real env vars override the file.
//!
//! Legacy: earlier builds wrote plaintext CREATIO_PASSWORD / N8N_TOOLS_API_KEY into
//! pvwatts.env. Those are still honored on read (via env), and the next save
//! migrates them into the vault and strips them from the file.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

const KEYRING_SERVICE: &str = "crs-pvwatts";
const SIDECAR_ACCOUNT: &str = "sidecar-key";

// ── OS credential vault (keyring) ───────────────────────────────────────────
fn vault_set(account: &str, value: &str) -> Result<()> {
    keyring::Entry::new(KEYRING_SERVICE, account)?.set_password(value)?;
    Ok(())
}
fn vault_get(account: &str) -> Option<String> {
    keyring::Entry::new(KEYRING_SERVICE, account).ok()?.get_password().ok()
}
fn vault_del(account: &str) {
    if let Ok(e) = keyring::Entry::new(KEYRING_SERVICE, account) {
        let _ = e.delete_credential();
    }
}
fn creatio_account(username: &str) -> String {
    format!("creatio:{}", username.to_lowercase())
}

// ── pvwatts.env (non-secret config) ─────────────────────────────────────────
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

/// Remove keys from pvwatts.env if present (used to migrate legacy plaintext secrets out).
fn strip_keys(keys: &[&str]) {
    let path = config_path();
    let mut pairs = read_pairs(&path);
    let before = pairs.len();
    pairs.retain(|(k, _)| !keys.contains(&k.as_str()));
    if pairs.len() != before {
        let _ = write_pairs(&path, &pairs);
    }
}

/// Merge arbitrary NON-secret KEY=VALUE settings into pvwatts.env.
pub fn save_kv(entries: &[(&str, &str)]) -> Result<PathBuf> {
    let path = config_path();
    let mut pairs = read_pairs(&path);
    for (k, v) in entries {
        upsert(&mut pairs, k, v);
    }
    write_pairs(&path, &pairs)?;
    Ok(path)
}

// ── Creatio credentials (username in file, password in vault) ────────────────
pub fn save_creatio_creds(username: &str, password: &str) -> Result<()> {
    vault_set(&creatio_account(username), password)?; // -> OS credential vault
    save_kv(&[("CREATIO_USERNAME", username)])?; // username is not a secret
    strip_keys(&["CREATIO_PASSWORD"]); // migrate any legacy plaintext password out
    Ok(())
}

/// Password for auto-login: OS vault first, then legacy plaintext env (migrated on next save).
pub fn get_creatio_password(username: &str) -> Option<String> {
    vault_get(&creatio_account(username))
        .or_else(|| std::env::var("CREATIO_PASSWORD").ok().filter(|s| !s.is_empty()))
}

pub fn forget_creatio_creds() -> Result<()> {
    if let Ok(u) = std::env::var("CREATIO_USERNAME") {
        if !u.is_empty() {
            vault_del(&creatio_account(&u));
        }
    }
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

// ── Sidecar key (in the vault) ───────────────────────────────────────────────
pub fn save_sidecar_key(key: &str) -> Result<()> {
    vault_set(SIDECAR_ACCOUNT, key)?;
    strip_keys(&["N8N_TOOLS_API_KEY"]); // never keep it in the file
    Ok(())
}

/// Sidecar key: legacy/override env first, then the OS vault.
pub fn get_sidecar_key() -> Option<String> {
    std::env::var("N8N_TOOLS_API_KEY")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| vault_get(SIDECAR_ACCOUNT))
}
