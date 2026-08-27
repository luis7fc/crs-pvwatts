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

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::creatio::LotBundle;
use crate::pdf::lot_paths;
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
        "array_index", "dc_system_size_kw", "module_type", "module_type_label",
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
          variant_label: Option<&str>, record_type: &str) -> Vec<String> {
    let sys = bundle.system.as_ref();
    let st = pv.station_info.clone().unwrap_or_default();
    vec![
        record_type.to_string(),
        generated.to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
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

fn array_row(bundle: &LotBundle, pv: &PvResult, a: &PvArray, idx: usize, generated: &str,
             is_candidate: bool, variant_label: Option<&str>) -> Vec<String> {
    let mut r = common(bundle, pv, generated, is_candidate, variant_label, "ARRAY");
    r.extend([
        (idx + 1).to_string(),
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
             variant_label: Option<&str>) -> Vec<String> {
    let mut r = common(bundle, pv, generated, is_candidate, variant_label, "LOT_TOTAL");
    let sum_kw: f64 = pv.arrays.iter().map(|a| a.system_capacity_kw).sum();
    let sum_ac: f64 = pv.arrays.iter().filter_map(|a| a.ac_annual).sum();
    r.extend([
        blank(),
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

/// Render the lot CSV to a string (BOM included).
pub fn render(bundle: &LotBundle, pv: &PvResult, is_candidate: bool,
              variant_label: Option<&str>, generated: &str) -> String {
    let mut out = String::from("\u{feff}");
    out.push_str(&line(&header()));
    for (i, a) in pv.arrays.iter().enumerate() {
        out.push_str(&line(&array_row(bundle, pv, a, i, generated, is_candidate, variant_label)));
    }
    out.push_str(&line(&total_row(bundle, pv, generated, is_candidate, variant_label)));
    out
}

/// Write the lot CSV beside the audit PDF. Returns the file path written.
pub fn write_lot_csv(
    root: &Path,
    bundle: &LotBundle,
    _arrays: &[ArrayInput],
    pv: &PvResult,
    is_candidate: bool,
    variant_label: Option<&str>,
) -> Result<PathBuf> {
    let (dir, stem) = lot_paths(root, bundle, variant_label);
    fs::create_dir_all(&dir).with_context(|| format!("could not create {}", dir.display()))?;
    let path = dir.join(format!("{stem}.csv"));
    let generated = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
    let body = render(bundle, pv, is_candidate, variant_label, &generated);
    let mut f = fs::File::create(&path).with_context(|| format!("could not write {}", path.display()))?;
    f.write_all(body.as_bytes())?;
    f.flush()?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle() -> LotBundle {
        LotBundle {
            lot_id: "id".into(), job: None, lot: None, lot_addr: None, zip: None,
            plan: None, builder: None, job_name: None, system: None,
            system_count: 0, inverter_efficiency: None, wattage: None,
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
    #[test]
    fn every_row_matches_the_header_width() {
        let n = header().len();
        let p = pv(vec![array(), array()]);
        let b = bundle();
        for (i, a) in p.arrays.iter().enumerate() {
            assert_eq!(array_row(&b, &p, a, i, "now", true, None).len(), n, "array row {i}");
        }
        assert_eq!(total_row(&b, &p, "now", true, None).len(), n, "total row");
    }

    #[test]
    fn totals_sum_the_additive_series_only() {
        let p = pv(vec![array(), array()]);
        let row = total_row(&bundle(), &p, "now", false, None);
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
