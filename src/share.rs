//! I: drive paths across operating systems.
//!
//! Every stored path (built-in defaults, `pvwatts.env`, the shared
//! `community_folders.csv`) is written in Windows form, `I:\...`, because the
//! Windows consultants are the main users. On a Mac the same share
//! (`\\CRS-SERVER1\Shares`) is connected through Finder and appears under
//! `/Volumes/Shares`, so paths are translated on the way in (`to_local`) and
//! back to `I:\...` on the way out (`to_shared`). On Windows both are no-ops.
//!
//! Override the Mac mount point with PVWATTS_SHARE_MOUNT.

use std::path::{Path, PathBuf};

const MAC_MOUNT: &str = "/Volumes/Shares";

/// A stored path as this OS should open it.
pub fn to_local(p: &str) -> PathBuf {
    if cfg!(target_os = "macos") {
        drive_to_mount(p, &mount())
    } else {
        PathBuf::from(p)
    }
}

/// A local path in the stored (Windows) form, so every OS can read it back.
pub fn to_shared(p: &Path) -> String {
    if cfg!(target_os = "macos") {
        mount_to_drive(p, &mount())
    } else {
        p.display().to_string()
    }
}

/// Where the share is connected on this Mac. Finder names a second connection
/// `Shares-1`, so take the first `Shares*` folder that exists.
fn mount() -> PathBuf {
    if let Some(m) = std::env::var("PVWATTS_SHARE_MOUNT").ok().filter(|s| !s.trim().is_empty()) {
        return PathBuf::from(m.trim());
    }
    let default = PathBuf::from(MAC_MOUNT);
    if default.is_dir() {
        return default;
    }
    let mut found: Vec<PathBuf> = std::fs::read_dir("/Volumes")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("Shares-")) && p.is_dir()
        })
        .collect();
    found.sort();
    found.into_iter().next().unwrap_or(default)
}

/// `I:\a\b` -> `{mount}/a/b`. Anything not on I: is returned unchanged.
fn drive_to_mount(p: &str, mount: &Path) -> PathBuf {
    let t = p.trim();
    let rest = match t.get(..2) {
        Some(d) if d.eq_ignore_ascii_case("I:") => &t[2..],
        _ => return PathBuf::from(t),
    };
    let mut out = mount.to_path_buf();
    for part in rest.split(['\\', '/']).filter(|s| !s.is_empty()) {
        out.push(part);
    }
    out
}

/// `{mount}/a/b` -> `I:\a\b`. Anything outside the mount is returned unchanged.
fn mount_to_drive(p: &Path, mount: &Path) -> String {
    match p.strip_prefix(mount) {
        Ok(rest) => {
            let parts: Vec<String> = rest.iter().map(|s| s.to_string_lossy().into_owned()).collect();
            format!(r"I:\{}", parts.join(r"\"))
        }
        Err(_) => p.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_paths_map_onto_the_mount_and_back() {
        let m = Path::new("/Volumes/Shares");
        let local = drive_to_mount(r"I:\Solar\1- New Construction\Production", m);
        assert_eq!(local, Path::new("/Volumes/Shares/Solar/1- New Construction/Production"));
        assert_eq!(mount_to_drive(&local, m), r"I:\Solar\1- New Construction\Production");
        assert_eq!(drive_to_mount("i:/Inventory and Purchasing/pv_watts_db", m),
                   Path::new("/Volumes/Shares/Inventory and Purchasing/pv_watts_db"));
    }

    #[test]
    fn other_paths_pass_through() {
        let m = Path::new("/Volumes/Shares");
        assert_eq!(drive_to_mount("/Users/me/pv", m), Path::new("/Users/me/pv"));
        assert_eq!(drive_to_mount(r"O:\Archive", m), Path::new(r"O:\Archive"));
        assert_eq!(mount_to_drive(Path::new("/Users/me/pv"), m), "/Users/me/pv");
    }
}
