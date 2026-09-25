//! Where a lot's PDF + CSV land — resolved from folders that already exist,
//! never invented. Creatio's builder/community names rarely match the share's
//! folder names ("Cambridge at Placer One (aka Cambridge at The Ranch)"), so the
//! old create-on-demand template littered Production with near-duplicate trees.
//!
//! Community folder, first hit wins:
//!   1. the saved mapping for this community (Opportunity GUID), if that folder
//!      still exists — `community_folders.csv` beside the central ledger, shared
//!      by every PC so one person's pick serves the whole team;
//!   2. the template `{root}\{builder}\{job_name}`, if it exists;
//!   3. nothing -> the UI asks the user to pick a folder (native dialog), and the
//!      pick is saved as that community's mapping.
//!
//! Inside the community folder: descend into `Consultations` when it exists,
//! then into a lot folder whose name carries this lot's street address when
//! exactly one does ("Lot 1 - 4524 Field View Drive"). Otherwise the files go in
//! the deepest of those that exists. Nothing is ever created.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};

use crate::creatio::LotBundle;
use crate::pdf::safe;

const MAP_FILE: &str = "community_folders.csv";
const MAP_HEADER: &str = "community_key,builder,job_name,folder,set_by,set_at\r\n";

/// Picks made this session. Covers a share hiccup on the mapping file so the
/// user isn't asked twice for the same community in one sitting.
static SESSION: Mutex<Option<HashMap<String, PathBuf>>> = Mutex::new(None);

/// Opportunity GUID; builder|job when a lot has no linked opportunity.
pub fn community_key(b: &LotBundle) -> String {
    match b.community_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => id.to_lowercase(),
        None => format!(
            "{}|{}",
            b.builder.as_deref().unwrap_or("").trim().to_lowercase(),
            b.job_name.as_deref().unwrap_or("").trim().to_lowercase()
        ),
    }
}

/// Where the resolved folder came from — shown to the user after a save.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Source {
    Saved,
    Template,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Saved => "saved folder",
            Source::Template => "builder/community folder",
        }
    }
}

/// The community folder for this lot, or None when the user has to choose.
pub fn community_folder(root: &Path, ledger: &Path, b: &LotBundle) -> Option<(PathBuf, Source)> {
    let key = community_key(b);
    if let Some(p) = SESSION.lock().ok().and_then(|g| g.as_ref()?.get(&key).cloned()) {
        if p.is_dir() {
            return Some((p, Source::Saved));
        }
    }
    if let Some(p) = read_mapping(&ledger.join(MAP_FILE)).remove(&key) {
        if p.is_dir() {
            return Some((p, Source::Saved));
        }
    }
    let t = template_dir(root, b)?;
    t.is_dir().then_some((t, Source::Template))
}

fn template_dir(root: &Path, b: &LotBundle) -> Option<PathBuf> {
    let builder = b.builder.as_deref().filter(|s| !s.trim().is_empty())?;
    let job = b.job_name.as_deref().filter(|s| !s.trim().is_empty())?;
    Some(root.join(safe(builder)).join(safe(job)))
}

/// Where the folder picker opens: the builder's folder if it exists, else root.
pub fn picker_start(root: &Path, b: &LotBundle) -> Option<PathBuf> {
    let builder = b.builder.as_deref().filter(|s| !s.trim().is_empty()).map(|s| root.join(safe(s)));
    builder.filter(|p| p.is_dir()).or_else(|| root.is_dir().then(|| root.to_path_buf()))
}

/// Final save folder under an existing community folder. Only descends into
/// folders that exist.
pub fn lot_dir(community: &Path, b: &LotBundle) -> PathBuf {
    let mut dir = community.to_path_buf();
    let consult = dir.join("Consultations");
    if consult.is_dir() {
        dir = consult;
    }
    if let Some(sub) = b.lot_addr.as_deref().and_then(|a| matching_lot_folder(&dir, a)) {
        dir = sub;
    }
    dir
}

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase()
}

/// Street part of `UsrLotNumberPlusAddress` — drops a leading "Lot 12 -" / "12-".
fn street(lot_addr: &str) -> String {
    let s = lot_addr.trim();
    let rest = s.strip_prefix("Lot ").or_else(|| s.strip_prefix("lot ")).unwrap_or(s);
    match rest.split_once('-') {
        Some((head, tail)) if head.trim().chars().all(|c| c.is_ascii_alphanumeric()) && !tail.trim().is_empty() => {
            tail.trim().to_string()
        }
        _ => s.to_string(),
    }
}

/// The one subfolder whose name ends with this lot's street address. Two or
/// more matches is ambiguous — stay put rather than guess.
fn matching_lot_folder(dir: &Path, lot_addr: &str) -> Option<PathBuf> {
    let want = norm(&street(lot_addr));
    if want.len() < 6 {
        return None; // too short to be an address; would match noise
    }
    let full = norm(lot_addr);
    let mut hits = fs::read_dir(dir).ok()?.flatten()
        .filter(|e| e.path().is_dir())
        .filter(|e| {
            let n = norm(&e.file_name().to_string_lossy());
            n == full || n.ends_with(&want)
        })
        .map(|e| e.path());
    let first = hits.next()?;
    hits.next().is_none().then_some(first)
}

// ── mapping file ────────────────────────────────────────────────────────────

/// Minimal RFC-4180 split of one line (quoted fields, doubled quotes).
fn split_line(line: &str) -> Vec<String> {
    let (mut out, mut cur, mut q) = (Vec::new(), String::new(), false);
    let mut it = line.chars().peekable();
    while let Some(c) = it.next() {
        match (c, q) {
            ('"', true) if it.peek() == Some(&'"') => { cur.push('"'); it.next(); }
            ('"', _) => q = !q,
            (',', false) => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

fn esc(s: &str) -> String {
    if s.contains(',') || s.contains('"') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// key -> folder; later rows win, so re-picking a community just appends.
fn read_mapping(path: &Path) -> HashMap<String, PathBuf> {
    let mut m = HashMap::new();
    let Ok(text) = fs::read_to_string(path) else { return m };
    for line in text.trim_start_matches('\u{feff}').lines().skip(1) {
        let f = split_line(line);
        if f.len() >= 4 && !f[0].trim().is_empty() && !f[3].trim().is_empty() {
            m.insert(f[0].trim().to_lowercase(), PathBuf::from(f[3].trim()));
        }
    }
    m
}

/// Remember `folder` for this lot's community: this session always, the shared
/// mapping file best-effort (the returned error is for the UI to show).
pub fn save_mapping(ledger: &Path, b: &LotBundle, folder: &Path) -> Result<()> {
    let key = community_key(b);
    if let Ok(mut g) = SESSION.lock() {
        g.get_or_insert_with(HashMap::new).insert(key.clone(), folder.to_path_buf());
    }
    if !ledger.is_dir() {
        anyhow::bail!("shared folder list unavailable: {}", ledger.display());
    }
    let path = ledger.join(MAP_FILE);
    let new = !path.exists();
    let mut f = fs::OpenOptions::new().create(true).append(true).open(&path)
        .with_context(|| format!("could not write {}", path.display()))?;
    if new {
        f.write_all(MAP_HEADER.as_bytes())?;
    }
    let who = std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_default();
    let row = [
        key,
        b.builder.clone().unwrap_or_default(),
        b.job_name.clone().unwrap_or_default(),
        folder.display().to_string(),
        who,
        chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
    ];
    let line = row.iter().map(|s| esc(s)).collect::<Vec<_>>().join(",") + "\r\n";
    f.write_all(line.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Each test uses its own community id: the session cache is process-wide.
    fn bundle(dir_builder: &str, job: &str, addr: &str) -> LotBundle {
        serde_json::from_value(serde_json::json!({
            "lot_id": "L1", "job": null, "lot": "1", "lot_addr": addr, "zip": null, "plan": null,
            "builder": dir_builder, "job_name": job, "community_id": format!("{dir_builder}|{job}"),
            "system": null, "system_count": 0, "inverter_efficiency": null, "wattage": null,
        })).unwrap()
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pvw_folders_{}_{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn street_strips_lot_prefix() {
        assert_eq!(street("Lot 1 - 4524 Field View Drive"), "4524 Field View Drive");
        assert_eq!(street("59- 4613 Fallon Court"), "4613 Fallon Court");
        assert_eq!(street("4613 Fallon Court"), "4613 Fallon Court");
    }

    #[test]
    fn missing_template_asks_instead_of_creating() {
        let root = tmp("root_missing");
        let ledger = tmp("ledger_missing");
        let b = bundle("KB Home", "Nowhere Estates", "Lot 1 - 1 Main St");
        assert!(community_folder(&root, &ledger, &b).is_none());
        assert!(!root.join("KB Home").exists(), "resolver must not create folders");
    }

    #[test]
    fn template_then_consultations_then_lot_folder() {
        let root = tmp("root_tpl");
        let ledger = tmp("ledger_tpl");
        let lot = root.join("KB Home").join("Esquire").join("Consultations").join("Lot 1 - 4524 Field View Drive");
        fs::create_dir_all(&lot).unwrap();
        let b = bundle("KB Home", "Esquire", "1 - 4524 Field View Drive");
        let (c, src) = community_folder(&root, &ledger, &b).unwrap();
        assert_eq!(src, Source::Template);
        assert_eq!(lot_dir(&c, &b), lot);
        // No lot folder -> Consultations itself.
        let b2 = bundle("KB Home", "Esquire", "Lot 9 - 99 Other Way");
        assert_eq!(lot_dir(&c, &b2), lot.parent().unwrap());
    }

    #[test]
    fn saved_mapping_wins_and_survives_reload() {
        let root = tmp("root_map");
        let ledger = tmp("ledger_map");
        let picked = root.join("KB Home").join("Cambridge at Placer One (aka Cambridge, The Ranch)");
        fs::create_dir_all(&picked).unwrap();
        let b = bundle("KB Home", "Cambridge at Placer One", "Lot 3 - 12 Elm Ct");
        save_mapping(&ledger, &b, &picked).unwrap();
        // Read straight from the file (the comma in the name exercises quoting).
        let m = read_mapping(&ledger.join(MAP_FILE));
        assert_eq!(m.get(&community_key(&b)), Some(&picked));
        let (c, src) = community_folder(&root, &ledger, &b).unwrap();
        assert_eq!((c, src), (picked, Source::Saved));
    }
}
