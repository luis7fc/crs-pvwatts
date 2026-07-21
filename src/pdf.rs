//! Per-lot audit PDF — human-readable record of the PVWatts inputs + response and
//! the Creatio inputs used (no creds), written to the mapped I: drive.
//!
//! Path template (from the business spec):
//!   {root}\{builder}\{job_name}\Consultations\{lot_addr}\PV_WATTS_{lot_addr}.pdf
//! Missing folders are created. `root` defaults to the I: Production path on
//! Windows; override with PVWATTS_OUTPUT_ROOT for dev on any OS.

use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use printpdf::{BuiltinFont, Mm, PdfDocument};

use crate::creatio::LotBundle;
use crate::sidecar::{ArrayInput, PvResult};

pub const DEFAULT_ROOT: &str = r"I:\Solar\1- New Construction\Production";

pub fn output_root() -> PathBuf {
    std::env::var("PVWATTS_OUTPUT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_ROOT))
}

/// Strip characters illegal in Windows path components.
fn safe(component: &str) -> String {
    let cleaned: String = component
        .chars()
        .map(|c| if r#"\/:*?"<>|"#.contains(c) { '_' } else { c })
        .collect();
    let t = cleaned.trim().trim_matches('.').trim();
    if t.is_empty() { "UNKNOWN".to_string() } else { t.to_string() }
}

/// Build the audit PDF and write it under the lot's Consultations folder.
/// Returns the file path written.
pub fn write_audit_pdf(
    root: &Path,
    bundle: &LotBundle,
    arrays: &[ArrayInput],
    pv: &PvResult,
    is_candidate: bool,
    variant_label: Option<&str>,
) -> Result<PathBuf> {
    let builder = safe(bundle.builder.as_deref().unwrap_or("UNKNOWN_BUILDER"));
    let job_name = safe(bundle.job_name.as_deref().unwrap_or("UNKNOWN_JOB"));
    let lot_addr = safe(bundle.lot_addr.as_deref().unwrap_or("UNKNOWN_LOT"));

    let dir = root.join(builder).join(job_name).join("Consultations").join(&lot_addr);
    fs::create_dir_all(&dir).with_context(|| format!("could not create {}", dir.display()))?;

    let fname = match variant_label {
        Some(v) => format!("PV_WATTS_{}_{}.pdf", lot_addr, safe(v)),
        None => format!("PV_WATTS_{lot_addr}.pdf"),
    };
    let path = dir.join(fname);

    let bytes = render(bundle, arrays, pv, is_candidate, variant_label)?;
    let file = fs::File::create(&path).with_context(|| format!("could not write {}", path.display()))?;
    let mut w = BufWriter::new(file);
    // printpdf saves via BufWriter
    use std::io::Write;
    w.write_all(&bytes)?;
    Ok(path)
}

/// Render the PDF to bytes.
pub fn render(
    bundle: &LotBundle,
    arrays: &[ArrayInput],
    pv: &PvResult,
    is_candidate: bool,
    variant_label: Option<&str>,
) -> Result<Vec<u8>> {
    // US Letter
    let (doc, page, layer_idx) = PdfDocument::new("PVWatts Generation Report", Mm(215.9), Mm(279.4), "Layer 1");
    let reg = doc.add_builtin_font(BuiltinFont::Helvetica)?;
    let bold = doc.add_builtin_font(BuiltinFont::HelveticaBold)?;
    let layer = doc.get_page(page).get_layer(layer_idx);

    let left: f32 = 15.0;
    let mut y: f32 = 265.0;
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();

    macro_rules! line {
        ($t:expr, $size:expr, $font:expr) => {{
            layer.use_text($t, $size as f32, Mm(left), Mm(y), $font);
            y -= ($size as f32) * 0.42 + 1.5;
        }};
        (gap) => {{ y -= 3.5; }};
    }

    line!("PVWatts Generation Report", 18.0, &bold);
    line!(format!("Generated {now}"), 9.0, &reg);
    line!(gap);

    line!("Lot", 12.0, &bold);
    line!(format!("Job {} · Lot {}", opt(&bundle.job), opt(&bundle.lot)), 10.0, &reg);
    line!(format!("{}", opt(&bundle.lot_addr)), 10.0, &reg);
    line!(format!("Builder: {}   Community/Job: {}", opt(&bundle.builder), opt(&bundle.job_name)), 10.0, &reg);
    line!(format!("Zip: {}   (PVWatts lat {:.4}, lon {:.4})", pv.zip, pv.lat, pv.lon), 10.0, &reg);
    line!(gap);

    line!("Creatio system inputs", 12.0, &bold);
    if let Some(sys) = &bundle.system {
        line!(format!("Panel: {}   Total qty: {}", opt(&sys.panel_model), sys.panel_qty.map(|q| q.to_string()).unwrap_or_else(|| "?".into())), 10.0, &reg);
        line!(format!("Inverter: {}   Efficiency: {}%", opt(&sys.inverter_model), bundle.inverter_efficiency.map(|e| e.to_string()).unwrap_or_else(|| "?".into())), 10.0, &reg);
        line!(format!("Creatio system size DC: {} kW", sys.size_dc.map(|d| format!("{d:.2}")).unwrap_or_else(|| "?".into())), 10.0, &reg);
    } else if let Some(v) = variant_label {
        line!(format!("Options lot — selected variant: {v}"), 10.0, &reg);
    }
    line!(gap);

    line!("PVWatts fixed inputs (7/20/2026 defaults)", 12.0, &bold);
    line!("Module: Premium · Array: Fixed roof · Losses 14.1% · DC/AC 1.2 · GCR 0.4 · Monthly soiling 3%", 9.0, &reg);
    line!(gap);

    line!("Arrays", 12.0, &bold);
    for (i, (a, r)) in arrays.iter().zip(pv.arrays.iter()).enumerate() {
        let kwh = r.ac_annual.map(|v| format!("{v:.0}")).unwrap_or_else(|| "?".into());
        line!(format!("Array {}: {:.2} kW · tilt {}° · azimuth {}°  ->  {} kWh/yr",
            i + 1, a.system_capacity_kw, a.tilt, a.azimuth, kwh), 10.0, &reg);
    }
    line!(gap);

    line!(format!("LOT TOTAL: {} kWh/yr", pv.lot_total_kwh), 13.0, &bold);
    let wb = if is_candidate { "YES — written to Creatio CrsEstAnnualKwhProductionLot" } else { "no (audit only)" };
    line!(format!("Creatio candidate: {wb}"), 10.0, &reg);
    line!(gap);

    if let Some(si) = &pv.station_info {
        let src = si.get("weather_data_source").and_then(|v| v.as_str()).unwrap_or("?");
        let st = si.get("state").and_then(|v| v.as_str()).unwrap_or("?");
        line!(format!("Weather source: {src} ({st})"), 8.0, &reg);
    }

    let mut buf = BufWriter::new(Vec::new());
    doc.save(&mut buf)?;
    Ok(buf.into_inner()?)
}

fn opt(o: &Option<String>) -> String {
    o.clone().unwrap_or_else(|| "—".to_string())
}
