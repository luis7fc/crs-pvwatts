//! Where a lot's PDF + CSV land — resolved from folders that already exist,
//! never invented. Creatio's builder/community names rarely match the share's
//! folder names ("Cambridge at Placer One (aka Cambridge at The Ranch)"), so the
//! old create-on-demand template littered Production with near-duplicate trees.
//!
//! Community folder, first hit wins:
//!   1. a saved mapping for this community (Opportunity GUID), if that folder
//!      still exists — `community_folders.csv` beside the central ledger, written
//!      by earlier versions and now only read (a row naming a lot folder or
//!      `Consultations` is walked up to the community);
//!   2. the template `{root}\{builder}\{job_name}`, if it exists.
//!
//! Inside the community folder: descend into `Consultations` when it exists,
//! then into the one lot folder whose name carries this lot's street address
//! ("Lot 1 - 4524 Field View Drive") or its lot number ("Lot 75").
//!
//! No community or no single lot folder -> the UI asks the user for THIS lot's
//! folder (native dialog). That pick is used for this record only and never
//! saved, so one lot's folder can't become another lot's default. Nothing is
//! ever created.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::creatio::LotBundle;
use crate::pdf::safe;

const MAP_FILE: &str = "community_folders.csv";

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

/// The community folder for this lot, or None when it can't be found.
pub fn community_folder(root: &Path, ledger: &Path, b: &LotBundle) -> Option<(PathBuf, Source)> {
    let key = community_key(b);
    // community_of: older versions saved whatever was picked, often a lot folder.
    if let Some(p) = read_mapping(&ledger.join(MAP_FILE)).remove(&key).map(|p| community_of(&p)) {
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

/// `Consultations` under the community when it exists, else the community.
/// Where lot folders live, and where the lot picker opens.
pub fn lots_parent(community: &Path) -> PathBuf {
    let consult = community.join("Consultations");
    if consult.is_dir() { consult } else { community.to_path_buf() }
}

/// This lot's own folder under an existing community folder, or None when no
/// single folder matches (the UI then asks, for this record only).
pub fn lot_dir(community: &Path, b: &LotBundle) -> Option<PathBuf> {
    matching_lot_folder(&lots_parent(community), b)
}

/// "Lot 75", "lot 75 - 1495 Woodhill Lane" -> "75". None when the name doesn't
/// start with "Lot <something>".
fn lot_number_of(folder_name: &str) -> Option<String> {
    let s = folder_name.trim();
    let rest = s.get(..3).filter(|p| p.eq_ignore_ascii_case("lot")).map(|_| &s[3..])?;
    let n: String = rest.trim_start().chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
    // "Lot75" counts; "Lots", "Lotus" don't.
    let sep = rest.starts_with(|c: char| !c.is_ascii_alphabetic());
    (!n.is_empty() && sep).then(|| n.to_lowercase())
}

/// Walk a saved folder up to its community: drop a lot folder, then a
/// `Consultations` folder. Anything else is already a community folder.
fn community_of(folder: &Path) -> PathBuf {
    let mut p = folder.to_path_buf();
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if lot_number_of(&name(&p)).is_some() {
        if let Some(up) = p.parent() { p = up.to_path_buf(); }
    }
    if name(&p).trim().eq_ignore_ascii_case("consultations") {
        if let Some(up) = p.parent() { p = up.to_path_buf(); }
    }
    p
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

/// The one subfolder whose name ends with this lot's street address or is
/// "Lot <this lot's number>". Two or more matches is ambiguous — ask rather
/// than guess.
fn matching_lot_folder(dir: &Path, b: &LotBundle) -> Option<PathBuf> {
    // Address too short to be an address would match noise — skip that test.
    let addr = b.lot_addr.as_deref()
        .map(|a| (norm(a), norm(&street(a))))
        .filter(|(_, want)| want.len() >= 6);
    let lot = b.lot.as_deref().map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty());
    let mut hits = fs::read_dir(dir).ok()?.flatten()
        .filter(|e| e.path().is_dir())
        .filter(|e| {
            let raw = e.file_name().to_string_lossy().into_owned();
            let n = norm(&raw);
            addr.as_ref().is_some_and(|(full, want)| n == *full || n.ends_with(want.as_str()))
                || (lot.is_some() && lot_number_of(&raw) == lot)
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

/// key -> folder; later rows win.
fn read_mapping(path: &Path) -> HashMap<String, PathBuf> {
    let mut m = HashMap::new();
    let Ok(text) = fs::read_to_string(path) else { return m };
    for line in text.trim_start_matches('\u{feff}').lines().skip(1) {
        let f = split_line(line);
        if f.len() >= 4 && !f[0].trim().is_empty() && !f[3].trim().is_empty() {
            m.insert(f[0].trim().to_lowercase(), crate::share::to_local(f[3].trim()));
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAP_HEADER: &str = "community_key,builder,job_name,folder,set_by,set_at\r\n";

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
        assert_eq!(lot_dir(&c, &b), Some(lot.clone()));
        // No lot folder -> ask (None); the picker opens in Consultations.
        let mut b2 = bundle("KB Home", "Esquire", "Lot 9 - 99 Other Way");
        b2.lot = Some("9".into());
        assert_eq!(lot_dir(&c, &b2), None);
        assert_eq!(lots_parent(&c), lot.parent().unwrap());
    }

    #[test]
    fn lot_folder_by_number_only() {
        let root = tmp("root_num");
        let consult = root.join("Wilson Homes").join("Elev8ions 6205").join("Consultations");
        for n in ["Lot 7", "Lot 75", "Lot 79", "Lots archive"] {
            fs::create_dir_all(consult.join(n)).unwrap();
        }
        let community = consult.parent().unwrap();
        let mut b = bundle("Wilson Homes", "Elev8ions at Tract 6205", "79 - 1495 Woodhill Lane");
        b.lot = Some("79".into());
        assert_eq!(lot_dir(community, &b), Some(consult.join("Lot 79")));
        b.lot = Some("7".into()); // must not take "Lot 75"/"Lot 79"
        b.lot_addr = Some("7 - 1 Nowhere Road".into());
        assert_eq!(lot_dir(community, &b), Some(consult.join("Lot 7")));
        b.lot = Some("80".into());
        b.lot_addr = Some("80 - 1501 Woodhill Lane".into());
        assert_eq!(lot_dir(community, &b), None);
    }

    #[test]
    fn lot_number_parsing() {
        assert_eq!(lot_number_of("Lot 75").as_deref(), Some("75"));
        assert_eq!(lot_number_of("lot 75 - 1495 Woodhill Lane").as_deref(), Some("75"));
        assert_eq!(lot_number_of("Lot75").as_deref(), Some("75"));
        assert_eq!(lot_number_of("Lots archive"), None);
        assert_eq!(lot_number_of("Lotus Court"), None);
        assert_eq!(lot_number_of("Consultations"), None);
    }

    #[test]
    fn lot_folder_pick_never_becomes_community_default() {
        let root = tmp("root_walk");
        let ledger = tmp("ledger_walk");
        let community = root.join("Wilson Homes").join("Elev8ions 6205");
        let lot75 = community.join("Consultations").join("Lot 75");
        fs::create_dir_all(&lot75).unwrap();
        assert_eq!(community_of(&lot75), community);
        assert_eq!(community_of(&community.join("Consultations")), community);
        assert_eq!(community_of(&community), community);
        // An old row that saved the lot folder resolves to the community.
        let b = bundle("Wilson Homes", "Elev8ions walk", "79 - 1495 Woodhill Lane");
        fs::write(
            ledger.join(MAP_FILE),
            format!("{MAP_HEADER}{},Wilson Homes,Elev8ions walk,{},x,2026-10-13 08:00\r\n",
                community_key(&b), lot75.display()),
        ).unwrap();
        let (c, src) = community_folder(&root, &ledger, &b).unwrap();
        assert_eq!((c, src), (community, Source::Saved));
    }

    #[test]
    fn saved_mapping_wins() {
        let root = tmp("root_map");
        let ledger = tmp("ledger_map");
        let picked = root.join("KB Home").join("Cambridge at Placer One (aka Cambridge, The Ranch)");
        fs::create_dir_all(&picked).unwrap();
        let b = bundle("KB Home", "Cambridge at Placer One", "Lot 3 - 12 Elm Ct");
        // A row as older versions wrote it (the comma in the name exercises quoting).
        fs::write(
            ledger.join(MAP_FILE),
            format!("{MAP_HEADER}{},KB Home,Cambridge at Placer One,\"{}\",x,2026-09-30 10:00\r\n",
                community_key(&b), picked.display()),
        ).unwrap();
        let m = read_mapping(&ledger.join(MAP_FILE));
        assert_eq!(m.get(&community_key(&b)), Some(&picked));
        let (c, src) = community_folder(&root, &ledger, &b).unwrap();
        assert_eq!((c, src), (picked, Source::Saved));
    }
}
