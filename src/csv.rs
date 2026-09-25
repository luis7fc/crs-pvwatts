//! Per-lot CSV — every value that appears on the enhanced PDF, in one flat file
//! next to it. Same folder, same basename, `.csv`.
//!
//! Shape: one row per modelled array, then a `LOT_TOTAL` row. Wide rather than
//! tidy, because the consumers are Excel and the consultation team rather than a
//! database — a full array fits on one line, monthly series included.
//!
//! The LOT_TOTAL row only fills columns that are genuinely additive across
//! arrays (DC size, AC and DC energy). Irradiance columns (solrad/poa), capacity
//! factor and the geometry inputs are left BLANK there: summing kWh/m2/day or
//! averaging tilt across arrays would produce a number that looks authoritative
//! and means nothing.
//!
//! Written UTF-8 with a BOM so Excel on Windows renders the degree signs and em
//! dashes instead of mojibake.
//!
//! LEDGER: every Save is also filed centrally under `ledger_dir()` (default
//! `I:\Inventory and Purchasing\pv_watts_db`) as `<lot>-<job name>-v<N>.csv`,
//! identical in shape to the lot CSV, and appended to `all_submissions.csv` in
//! the same folder. N counts prior submissions for that lot+job, so options lots
//! saved several times keep every version. The ledger is best-effort: a share
//! outage must not block the consultation, so the caller reports the failure
//! instead of failing the commit.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::creatio::LotBundle;
use crate::pdf::{lot_stem, safe};
use crate::sidecar::{ArrayInput, PvArray, PvResult};

const MONTH_KEYS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

/// RFC-4180 field escaping.
fn esc(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn line(fields: &[String]) -> String {
    let mut s = fields.iter().map(|f| esc(f)).collect::<Vec<_>>().join(",");
    s.push_str("\r\n");
    s
}

fn blank() -> String { String::new() }
fn num<T: std::fmt::Display>(v: Option<T>) -> String {
    v.map(|x| x.to_string()).unwrap_or_default()
}
fn s(o: &Option<String>) -> String {
    o.clone().unwrap_or_default()
}

/// Column order. Kept as one list so the header and every row stay in step.
fn header() -> Vec<String> {
    let mut h: Vec<String> = [
        "record_type", "generated_at", "tool_version",
        "submission_key", "submission_version",
        // lot identity
        "builder", "job_name", "job", "lot", "lot_addr", "lot_id", "plan",
        "variant_label", "is_candidate", "creatio_writeback_kwh",
        // Creatio system inputs
        "creatio_system_name", "creatio_system_count", "creatio_panel_model",
        "creatio_panel_qty", "creatio_wattage_per_panel", "creatio_inverter_model",
        "creatio_inverter_guid", "creatio_inverter_eff_pct", "creatio_size_dc_kw",
        "creatio_size_ac_kw",
        // location / station
        "requested_zip", "requested_lat", "requested_lon",
        "station_lat", "station_lon", "station_elev_m", "station_tz",
        "station_location", "station_city", "station_state", "station_country",
        "station_solar_resource_file", "station_distance_m", "station_distance_mi",
        "weather_data_source", "nrel_key_source",
        // PV system specifications (as sent to PVWatts)
        "array_index", "panels", "dc_system_size_kw", "module_type", "module_type_label",
        "array_type", "array_type_label", "losses_pct", "tilt_deg", "azimuth_deg",
        "dc_ac_ratio", "inv_eff_pct", "gcr", "albedo", "bifacial",
        // annual results
        "ac_annual_kwh", "ac_annual_displayed_kwh", "solrad_annual",
        "capacity_factor_pct",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    for (prefix, unit) in [
        ("ac", "kwh"),
        ("solrad", "kwh_m2_day"),
        ("poa", "kwh_m2"),
        ("dc", "kwh"),
        ("soiling", "pct"),
    ] {
        for m in MONTH_KEYS {
            h.push(format!("{prefix}_{m}_{unit}"));
        }
    }
    h.push("errors".into());
    h.push("warnings".into());
    h
}

/// The 12 monthly cells for one series, or 12 blanks.
fn monthly(v: &Option<Vec<f64>>) -> Vec<String> {
    match v {
        Some(m) if m.len() == 12 => m.iter().map(|x| format!("{x}")).collect(),
        _ => vec![blank(); 12],
    }
}

fn sum_monthly(arrays: &[PvArray], pick: fn(&PvArray) -> &Option<Vec<f64>>) -> Vec<String> {
    let mut acc = [0.0f64; 12];
    let mut any = false;
    for a in arrays {
        if let Some(m) = pick(a) {
            if m.len() == 12 {
                any = true;
                for (i, v) in m.iter().enumerate() {
                    acc[i] += v;
                }
            }
        }
    }
    if any {
        acc.iter().map(|x| format!("{x}")).collect()
    } else {
        vec![blank(); 12]
    }
}

/// Identity + station columns, identical on every row of the file.
fn common(bundle: &LotBundle, pv: &PvResult, generated: &str, is_candidate: bool,
          variant_label: Option<&str>, record_type: &str, sub: &Submission) -> Vec<String> {
    let sys = bundle.system.as_ref();
    let st = pv.station_info.clone().unwrap_or_default();
    vec![
        record_type.to_string(),
        generated.to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
        sub.key.clone(),
        sub.version.to_string(),
        s(&bundle.builder),
        s(&bundle.job_name),
        s(&bundle.job),
        s(&bundle.lot),
        s(&bundle.lot_addr),
        bundle.lot_id.clone(),
        s(&bundle.plan),
        variant_label.unwrap_or("").to_string(),
        is_candidate.to_string(),
        if is_candidate { pv.lot_total_kwh.to_string() } else { blank() },
        sys.and_then(|x| x.name.clone()).unwrap_or_default(),
        bundle.system_count.to_string(),
        sys.and_then(|x| x.panel_model.clone()).unwrap_or_default(),
        num(sys.and_then(|x| x.panel_qty)),
        num(bundle.wattage),
        sys.and_then(|x| x.inverter_model.clone()).unwrap_or_default(),
        sys.and_then(|x| x.inverter_guid.clone()).unwrap_or_default(),
        num(bundle.inverter_efficiency),
        num(sys.and_then(|x| x.size_dc)),
        num(sys.and_then(|x| x.size_ac)),
        pv.zip.clone(),
        pv.lat.to_string(),
        pv.lon.to_string(),
        num(st.lat),
        num(st.lon),
        num(st.elev),
        num(st.tz),
        s(&st.location),
        s(&st.city),
        s(&st.state),
        s(&st.country),
        s(&st.solar_resource_file),
        num(st.distance),
        st.distance_mi().map(|m| format!("{m:.3}")).unwrap_or_default(),
        s(&st.weather_data_source),
        pv.api_key_source.clone(),
    ]
}

/// Panels in one array, recovered from kW DC and the Creatio panel wattage.
/// Only meaningful on normal lots (options lots are sized in kW, not panels).
fn panels_for(bundle: &LotBundle, a: &PvArray) -> Option<u32> {
    if bundle.options_lot { return None; }
    let w = bundle.wattage.filter(|w| *w > 0)? as f64;
    Some((a.system_capacity_kw * 1000.0 / w).round() as u32)
}

fn array_row(bundle: &LotBundle, pv: &PvResult, a: &PvArray, idx: usize, generated: &str,
             is_candidate: bool, variant_label: Option<&str>, sub: &Submission) -> Vec<String> {
    let mut r = common(bundle, pv, generated, is_candidate, variant_label, "ARRAY", sub);
    r.extend([
        (idx + 1).to_string(),
        num(panels_for(bundle, a)),
        a.system_capacity_kw.to_string(),
        num(a.module_type),
        a.module_type_label().to_string(),
        num(a.array_type),
        a.array_type_label().to_string(),
        num(a.losses),
        a.tilt.to_string(),
        a.azimuth.to_string(),
        num(a.dc_ac_ratio),
        num(a.inv_eff),
        num(a.gcr),
        "From weather file".to_string(),
        "No (0)".to_string(),
        num(a.ac_annual),
        num(a.ac_annual_displayed()),
        num(a.solrad_annual),
        num(a.capacity_factor),
    ]);
    r.extend(monthly(&a.ac_monthly));
    r.extend(monthly(&a.solrad_monthly));
    r.extend(monthly(&a.poa_monthly));
    r.extend(monthly(&a.dc_monthly));
    r.extend(monthly(&a.soiling_monthly));
    r.push(a.errors.join("; "));
    r.push(a.warnings.join("; "));
    r
}

fn total_row(bundle: &LotBundle, pv: &PvResult, generated: &str, is_candidate: bool,
             variant_label: Option<&str>, sub: &Submission) -> Vec<String> {
    let mut r = common(bundle, pv, generated, is_candidate, variant_label, "LOT_TOTAL", sub);
    let sum_kw: f64 = pv.arrays.iter().map(|a| a.system_capacity_kw).sum();
    let sum_ac: f64 = pv.arrays.iter().filter_map(|a| a.ac_annual).sum();
    let panels: Vec<u32> = pv.arrays.iter().filter_map(|a| panels_for(bundle, a)).collect();
    r.extend([
        blank(),
        if panels.is_empty() { blank() } else { panels.iter().sum::<u32>().to_string() },
        sum_kw.to_string(),
        blank(), blank(), blank(), blank(),
        blank(), blank(), blank(),
        blank(), blank(), blank(),
        blank(), blank(),
        sum_ac.to_string(),
        pv.lot_total_kwh.to_string(),
        blank(),
        blank(),
    ]);
    r.extend(sum_monthly(&pv.arrays, |a| &a.ac_monthly));
    r.extend(vec![blank(); 12]);
    r.extend(vec![blank(); 12]);
    r.extend(sum_monthly(&pv.arrays, |a| &a.dc_monthly));
    r.extend(vec![blank(); 12]);
    let errs: Vec<String> = pv.arrays.iter().flat_map(|a| a.errors.clone()).collect();
    let warns: Vec<String> = pv.arrays.iter().flat_map(|a| a.warnings.clone()).collect();
    r.push(errs.join("; "));
    r.push(warns.join("; "));
    r
}

/// Identity of one Save in the central ledger: `<lot>-<job name>-v<N>`.
#[derive(Debug, Clone, PartialEq)]
pub struct Submission {
    pub key: String,
    pub version: u32,
}

/// Rows only (no BOM, no header) — what gets appended to `all_submissions.csv`.
fn rows(bundle: &LotBundle, pv: &PvResult, is_candidate: bool,
        variant_label: Option<&str>, generated: &str, sub: &Submission) -> String {
    let mut out = String::new();
    for (i, a) in pv.arrays.iter().enumerate() {
        out.push_str(&line(&array_row(bundle, pv, a, i, generated, is_candidate, variant_label, sub)));
    }
    out.push_str(&line(&total_row(bundle, pv, generated, is_candidate, variant_label, sub)));
    out
}

/// Render the lot CSV to a string (BOM included).
pub fn render(bundle: &LotBundle, pv: &PvResult, is_candidate: bool,
              variant_label: Option<&str>, generated: &str, sub: &Submission) -> String {
    let mut out = String::from("\u{feff}");
    out.push_str(&line(&header()));
    out.push_str(&rows(bundle, pv, is_candidate, variant_label, generated, sub));
    out
}

/// Write the lot CSV beside the audit PDF. Returns the file path written.
pub fn write_lot_csv(
    dir: &Path,
    bundle: &LotBundle,
    _arrays: &[ArrayInput],
    pv: &PvResult,
    is_candidate: bool,
    variant_label: Option<&str>,
    sub: &Submission,
) -> Result<PathBuf> {
    let path = dir.join(format!("{}.csv", lot_stem(bundle, variant_label)));
    let generated = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
    let body = render(bundle, pv, is_candidate, variant_label, &generated, sub);
    let mut f = fs::File::create(&path).with_context(|| format!("could not write {}", path.display()))?;
    f.write_all(body.as_bytes())?;
    f.flush()?;
    Ok(path)
}

// ── central ledger ──────────────────────────────────────────────────────────

const DEFAULT_LEDGER_DIR: &str = r"I:\Inventory and Purchasing\pv_watts_db";
const MASTER_FILE: &str = "all_submissions.csv";

/// Where submissions are filed centrally. Override with PVWATTS_LEDGER_DIR.
pub fn ledger_dir() -> PathBuf {
    std::env::var("PVWATTS_LEDGER_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_LEDGER_DIR))
}

/// `<lot code>-<job name>` — the part of the key that identifies the lot.
fn key_base(bundle: &LotBundle) -> String {
    let lot = bundle.lot.as_deref().filter(|s| !s.trim().is_empty())
        .or(bundle.lot_addr.as_deref()).unwrap_or("UNKNOWN_LOT");
    let job = bundle.job_name.as_deref().filter(|s| !s.trim().is_empty())
        .or(bundle.job.as_deref()).unwrap_or("UNKNOWN_JOB");
    format!("{}-{}", safe(lot), safe(job))
}

/// Next version for this lot+job: one past the highest `<base>-v<N>.csv` already
/// filed. An unreadable ledger folder yields v1; the write then reports why.
pub fn next_submission(ledger: &Path, bundle: &LotBundle) -> Submission {
    let base = key_base(bundle);
    let prefix = format!("{base}-v");
    let mut max = 0u32;
    if let Ok(rd) = fs::read_dir(ledger) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if let Some(rest) = name.strip_prefix(&prefix) {
                if let Some(n) = rest.strip_suffix(".csv").and_then(|n| n.parse::<u32>().ok()) {
                    max = max.max(n);
                }
            }
        }
    }
    let version = max + 1;
    Submission { key: format!("{prefix}{version}"), version }
}

/// File this Save in the ledger: its own `<key>.csv` plus rows appended to the
/// master. Returns the per-submission file path.
pub fn write_ledger(
    ledger: &Path,
    bundle: &LotBundle,
    pv: &PvResult,
    is_candidate: bool,
    variant_label: Option<&str>,
    sub: &Submission,
) -> Result<PathBuf> {
    fs::create_dir_all(ledger).with_context(|| format!("ledger folder unavailable: {}", ledger.display()))?;
    let generated = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
    let path = ledger.join(format!("{}.csv", sub.key));
    let body = render(bundle, pv, is_candidate, variant_label, &generated, sub);
    fs::write(&path, body.as_bytes()).with_context(|| format!("could not write {}", path.display()))?;

    let master = ledger.join(MASTER_FILE);
    let fresh = fs::metadata(&master).map(|m| m.len() == 0).unwrap_or(true);
    let mut f = fs::OpenOptions::new().create(true).append(true).open(&master)
        .with_context(|| format!("could not append {}", master.display()))?;
    if fresh {
        f.write_all("\u{feff}".as_bytes())?;
        f.write_all(line(&header()).as_bytes())?;
    }
    f.write_all(rows(bundle, pv, is_candidate, variant_label, &generated, sub).as_bytes())?;
    f.flush()?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle() -> LotBundle {
        LotBundle {
            lot_id: "id".into(), job: None, lot: None, lot_addr: None, zip: None,
            plan: None, builder: None, job_name: None, community_id: None, system: None,
            system_count: 0, options_lot: true, inverter_efficiency: None, wattage: None,
        }
    }

    fn pv(arrays: Vec<PvArray>) -> PvResult {
        PvResult {
            ok: true, zip: "93311".into(), lat: 35.3, lon: -119.1,
            api_key_source: "env".into(), lot_total_kwh: 7425, arrays,
            defaults: None, station_info: None,
        }
    }

    fn array() -> PvArray {
        PvArray {
            system_capacity_kw: 4.51, tilt: 18.0, azimuth: 213.0,
            inv_eff: Some(96.0), losses: Some(14.1), module_type: Some(1),
            array_type: Some(1), dc_ac_ratio: Some(1.2), gcr: Some(0.4),
            soiling_monthly: Some(vec![3.0; 12]),
            ac_annual: Some(7424.6), ac_monthly: Some(vec![350.0; 12]),
            solrad_annual: Some(6.09), solrad_monthly: Some(vec![6.0; 12]),
            poa_monthly: Some(vec![180.0; 12]), dc_monthly: Some(vec![400.0; 12]),
            capacity_factor: Some(18.79), errors: vec![], warnings: vec![],
        }
    }

    /// Header and rows are built by separate code paths; if they drift the file
    /// silently misaligns, so pin the width.
    fn sub() -> Submission { Submission { key: "425-Job-v1".into(), version: 1 } }

    #[test]
    fn every_row_matches_the_header_width() {
        let n = header().len();
        let p = pv(vec![array(), array()]);
        let b = bundle();
        for (i, a) in p.arrays.iter().enumerate() {
            assert_eq!(array_row(&b, &p, a, i, "now", true, None, &sub()).len(), n, "array row {i}");
        }
        assert_eq!(total_row(&b, &p, "now", true, None, &sub()).len(), n, "total row");
    }

    #[test]
    fn panels_recovered_from_kw_on_normal_lots_only() {
        let p = pv(vec![array()]);
        let mut b = bundle();
        let h = header();
        let col = h.iter().position(|c| c == "panels").unwrap();
        assert_eq!(array_row(&b, &p, &p.arrays[0], 0, "now", true, None, &sub())[col], "");
        b.options_lot = false; b.wattage = Some(410);
        assert_eq!(array_row(&b, &p, &p.arrays[0], 0, "now", true, None, &sub())[col], "11");
        assert_eq!(total_row(&b, &p, "now", true, None, &sub())[col], "11");
    }

    #[test]
    fn ledger_versions_count_per_lot_and_job() {
        let dir = std::env::temp_dir().join(format!("pvw_ledger_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut b = bundle();
        b.lot = Some("425".into()); b.job_name = Some("Sunset / Phase 2".into());
        assert_eq!(next_submission(&dir, &b), Submission { key: "425-Sunset _ Phase 2-v1".into(), version: 1 });
        let p = pv(vec![array()]);
        let s1 = next_submission(&dir, &b);
        write_ledger(&dir, &b, &p, true, None, &s1).unwrap();
        let s2 = next_submission(&dir, &b);
        assert_eq!(s2.version, 2);
        write_ledger(&dir, &b, &p, false, Some("8.2 kW"), &s2).unwrap();
        let master = fs::read_to_string(dir.join(MASTER_FILE)).unwrap();
        assert_eq!(master.matches("\r\n").count(), 1 + 2 * 2, "header + 2 rows per submission");
        assert!(dir.join("425-Sunset _ Phase 2-v2.csv").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn totals_sum_the_additive_series_only() {
        let p = pv(vec![array(), array()]);
        let row = total_row(&bundle(), &p, "now", false, None, &sub());
        let h = header();
        let at = |name: &str| row[h.iter().position(|c| c == name).unwrap()].clone();
        assert_eq!(at("dc_system_size_kw"), "9.02");
        assert_eq!(at("ac_jan_kwh"), "700");
        assert_eq!(at("solrad_jan_kwh_m2_day"), "");
        assert_eq!(at("capacity_factor_pct"), "");
    }

    #[test]
    fn escapes_embedded_commas() {
        assert_eq!(esc("a,b"), "\"a,b\"");
        assert_eq!(esc("plain"), "plain");
    }
}
