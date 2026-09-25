//! Per-lot audit PDF — a clone of the NREL PVWatts results page, one block per
//! modelled array, plus a CRS summary page carrying the lot roll-up and the
//! Creatio inputs used (no creds). Written to the mapped I: drive.
//!
//! Written as PV_WATTS_{lot_addr}.pdf into the folder `folders::resolve` picked
//! (saved mapping, else {root}\{builder}\{job_name} if it exists, else the user
//! chooses). Nothing is ever created here — a missing folder is the user's call.
//! `root` defaults to the I: Production path on Windows; override with
//! PVWATTS_OUTPUT_ROOT for dev on any OS.
//!
//! LAYOUT: every coordinate below was measured off a real PVWatts print
//! (NREL results page -> Chrome print-to-PDF, US Letter). Units are PDF points
//! with a TOP-LEFT origin, which is how the reference measures; `py()` flips to
//! printpdf's bottom-left origin. Keep the constants in points so they stay
//! comparable to a fresh measurement off another NREL print.
//!
//! PVWatts models ONE array per run; a CRS lot is split across N arrays. So the
//! document is N two-page NREL-style blocks (the second page carries the
//! Performance Metrics table, exactly as the browser print spills it) followed
//! by one CRS summary page. A single-array lot therefore comes out as a
//! page-for-page clone plus the summary.

use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use printpdf::{BuiltinFont, Color, IndirectFontRef, Mm, PdfDocument, PdfLayerReference, Rgb};

use crate::creatio::LotBundle;
use crate::sidecar::{ArrayInput, PvArray, PvResult};

pub const DEFAULT_ROOT: &str = r"I:\Solar\1- New Construction\Production";

// ── page geometry (points, top-left origin) ─────────────────────────────────
const PAGE_W: f32 = 612.0;
const PAGE_H: f32 = 792.0;

const SIDEBAR_X: f32 = 28.5;
const SIDEBAR_W: f32 = 100.0;
const VRULE_L: f32 = 154.1;
const VRULE_R: f32 = 572.0;
const VRULE_TOP: f32 = 24.1;
const VRULE_BOT: f32 = 764.3;

const MAIN_X0: f32 = 166.3;
const MAIN_X1: f32 = 560.9;

// Monthly table columns.
const COL_MONTH_X1: f32 = 283.6;
const COL_RAD_X1: f32 = 444.8;

// Key/value tables (Location, PV System Specifications).
const KV_SPLIT: f32 = 341.9;
const KV_LABEL_X: f32 = 170.8;
const KV_VALUE_X: f32 = 346.4;
const SECTION_HDR_X: f32 = 169.1;

const ROW_H: f32 = 17.8;

// ── palette (sampled from the reference print) ──────────────────────────────
fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(Rgb::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, None))
}
fn c_orange() -> Color { rgb(255, 84, 0) }
fn c_blue() -> Color { rgb(0, 85, 151) }
fn c_navy() -> Color { rgb(0, 58, 104) }
fn c_gray_dk() -> Color { rgb(78, 78, 78) }
fn c_text() -> Color { rgb(51, 51, 51) }
fn c_text2() -> Color { rgb(45, 45, 45) }
fn c_italic() -> Color { rgb(34, 34, 34) }
fn c_rule() -> Color { rgb(160, 160, 160) }
fn c_vrule() -> Color { rgb(204, 204, 204) }
fn c_rule_soft() -> Color { rgb(229, 229, 229) }

// ── Helvetica metrics (AFM widths /1000em) ──────────────────────────────────
// printpdf's builtin fonts carry no width API, and the page needs centred and
// right-aligned cells, so the widths live here. ASCII 32..126; anything else
// falls back to a mid-width advance.
const W_REG: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278,
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556,
    1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778,
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556,
    333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556,
    556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];
const W_BOLD: [u16; 95] = [
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278,
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611,
    975, 722, 722, 722, 722, 667, 611, 778, 722, 278, 556, 722, 611, 833, 722, 778,
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 333, 278, 333, 584, 556,
    333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556, 278, 889, 611, 611,
    611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584,
];

#[derive(Clone, Copy, PartialEq)]
pub enum Face { Reg, Bold, Oblique }

/// Width of `s` at `size` points.
fn text_w(s: &str, size: f32, face: Face) -> f32 {
    let table = if face == Face::Bold { &W_BOLD } else { &W_REG };
    let mut total = 0.0f32;
    for ch in s.chars() {
        let w = match ch {
            ' '..='~' => table[(ch as usize) - 32] as f32,
            '°' => 400.0,
            '®' => 737.0,
            '·' => 278.0,
            '—' => 1000.0,
            '–' => 556.0,
            _ => 556.0,
        };
        total += w;
    }
    total * size / 1000.0
}

/// Greedy word wrap to `max_w` points.
fn wrap(s: &str, size: f32, face: Face, max_w: f32) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in s.split_whitespace() {
        let probe = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if text_w(&probe, size, face) <= max_w || line.is_empty() {
            line = probe;
        } else {
            out.push(std::mem::take(&mut line));
            line = word.to_string();
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

pub fn output_root() -> PathBuf {
    std::env::var("PVWATTS_OUTPUT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_ROOT))
}

/// Strip characters illegal in Windows path components.
pub fn safe(component: &str) -> String {
    let cleaned: String = component
        .chars()
        .map(|c| if r#"\/:*?"<>|"#.contains(c) { '_' } else { c })
        .collect();
    let t = cleaned.trim().trim_matches('.').trim();
    if t.is_empty() { "UNKNOWN".to_string() } else { t.to_string() }
}

/// The basename shared by the PDF and the CSV.
pub fn lot_stem(bundle: &LotBundle, variant_label: Option<&str>) -> String {
    let lot_addr = safe(bundle.lot_addr.as_deref().unwrap_or("UNKNOWN_LOT"));
    match variant_label {
        Some(v) => format!("PV_WATTS_{}_{}", lot_addr, safe(v)),
        None => format!("PV_WATTS_{lot_addr}"),
    }
}

// ── NREL sidebar boilerplate (verbatim from the results page) ───────────────
// Reproduced as-is: these are the caveats that belong with the numbers, and
// dropping them would leave a page that looks like a PVWatts print but quietly
// omits the model's own stated limits.
const CAUTION: &str = "Caution: Photovoltaic system performance predictions calculated by PVWatts\u{00ae} include many inherent assumptions and uncertainties and do not reflect variations between PV technologies nor site-specific characteristics except as represented by PVWatts\u{00ae} inputs. For example, PV modules with better performance are not differentiated within PVWatts\u{00ae} from lesser performing modules. Both NREL and private companies provide more sophisticated PV modeling tools (such as the System Advisor Model at //sam.nrel.gov) that allow for more precise and complex modeling of PV systems.";

const EXPECTED_RANGE: &str = "The expected range is based on 30 years of actual weather data at the given location and is intended to provide an indication of the variation you might see. For more information, please refer to this NREL report: The Error Report.";

const DISCLAIMER_1: &str = "Disclaimer: The PVWatts\u{00ae} Model (\"Model\") is provided by the National Renewable Energy Laboratory (\"NREL\"), which is operated by the Alliance for Sustainable Energy, LLC (\"Alliance\") for the U.S. Department Of Energy (\"DOE\") and may be used for any purpose whatsoever.";

const DISCLAIMER_2: &str = "The names DOE/NREL/ALLIANCE shall not be used in any representation, advertising, publicity or other manner whatsoever to endorse or promote any entity that adopts or uses the Model. DOE/NREL/ALLIANCE shall not provide any support, consulting, training or assistance of any kind with regard to the use of the Model or any updates, revisions or new versions of the Model.";

const DISCLAIMER_3: &str = "YOU AGREE TO INDEMNIFY DOE/NREL/ALLIANCE, AND ITS AFFILIATES, OFFICERS, AGENTS, AND EMPLOYEES AGAINST ANY CLAIM OR DEMAND, INCLUDING REASONABLE ATTORNEYS' FEES, RELATED TO YOUR USE, RELIANCE, OR ADOPTION OF THE MODEL FOR ANY PURPOSE WHATSOEVER. THE MODEL IS PROVIDED BY DOE/NREL/ALLIANCE 'AS IS' AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING BUT NOT LIMITED TO THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE EXPRESSLY DISCLAIMED. IN NO EVENT SHALL DOE/NREL/ALLIANCE BE LIABLE FOR ANY SPECIAL, INDIRECT OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER, INCLUDING BUT NOT LIMITED TO CLAIMS ASSOCIATED WITH THE LOSS OF DATA OR PROFITS, WHICH MAY RESULT FROM ANY ACTION IN CONTRACT, NEGLIGENCE OR OTHER TORTIOUS CLAIM THAT ARISES OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THE MODEL.";

const DISCLAIMER_4: &str = "The energy output range is based on analysis of 30 years of historical weather data, and is intended to provide an indication of the possible interannual variability in generation for a Fixed (open rack) PV system at this location.";

const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June",
    "July", "August", "September", "October", "November", "December",
];
const MONTHS_ABBR: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "June", "July", "Aug", "Sept", "Oct", "Nov", "Dec",
];

// ── drawing surface ─────────────────────────────────────────────────────────
fn mmx(pt: f32) -> Mm { Mm(pt * 25.4 / 72.0) }
/// Flip a top-left y (how the reference is measured) to printpdf's bottom-left.
fn mmy(y_top: f32) -> Mm { Mm((PAGE_H - y_top) * 25.4 / 72.0) }

struct Canvas<'a> {
    layer: PdfLayerReference,
    reg: &'a IndirectFontRef,
    bold: &'a IndirectFontRef,
    obl: &'a IndirectFontRef,
}

impl<'a> Canvas<'a> {
    fn font(&self, f: Face) -> &IndirectFontRef {
        match f {
            Face::Reg => self.reg,
            Face::Bold => self.bold,
            Face::Oblique => self.obl,
        }
    }

    /// `y_top` is the top of the text's line box, matching how the reference was
    /// measured; the baseline sits one ascender below it.
    fn text(&self, s: &str, size: f32, x: f32, y_top: f32, face: Face, col: Color) {
        if s.is_empty() {
            return;
        }
        self.layer.set_fill_color(col);
        self.layer.use_text(s, size, mmx(x), mmy(y_top + size * 0.905), self.font(face));
    }

    fn text_center(&self, s: &str, size: f32, cx: f32, y_top: f32, face: Face, col: Color) {
        self.text(s, size, cx - text_w(s, size, face) / 2.0, y_top, face, col);
    }

    fn text_right(&self, s: &str, size: f32, right: f32, y_top: f32, face: Face, col: Color) {
        self.text(s, size, right - text_w(s, size, face), y_top, face, col);
    }

    fn fill(&self, x0: f32, y0: f32, x1: f32, y1: f32, col: Color) {
        self.layer.set_fill_color(col);
        let r = printpdf::Rect::new(mmx(x0), mmy(y1), mmx(x1), mmy(y0))
            .with_mode(printpdf::path::PaintMode::Fill);
        self.layer.add_rect(r);
    }

    /// The reference draws every rule as a thin filled rect, not a stroke.
    fn hrule(&self, x0: f32, x1: f32, y: f32, col: Color) {
        self.fill(x0, y, x1, y + 0.6, col);
    }

    fn vrule(&self, x: f32, y0: f32, y1: f32, col: Color) {
        self.fill(x, y0, x + 1.1, y1, col);
    }
}

// ── formatting ──────────────────────────────────────────────────────────────
fn thousands(n: i64) -> String {
    let neg = n < 0;
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if neg { format!("-{out}") } else { out }
}

/// Whole degrees where the value is whole, one decimal otherwise — the NREL page
/// prints "18\u{00b0}", never "18.0\u{00b0}".
fn deg(v: f64) -> String {
    if (v - v.round()).abs() < 0.05 {
        format!("{:.0}\u{00b0}", v)
    } else {
        format!("{:.1}\u{00b0}", v)
    }
}

fn trim_num(v: f64) -> String {
    if (v - v.round()).abs() < 0.005 {
        format!("{:.0}", v)
    } else {
        format!("{v}")
    }
}

fn opt(o: &Option<String>) -> String {
    o.clone().unwrap_or_else(|| "\u{2014}".to_string())
}

// ── sidebar ─────────────────────────────────────────────────────────────────
fn draw_sidebar(c: &Canvas, bundle: &LotBundle, pv: &PvResult, generated: &str,
                array_note: Option<String>, is_candidate: bool) {
    // Page identity, where the NREL wordmark sits on the original. Their logo
    // artwork is deliberately not reproduced.
    c.text("PVWatts", 13.0, SIDEBAR_X, 30.0, Face::Bold, c_blue());
    c.text("\u{00ae}", 6.0, SIDEBAR_X + text_w("PVWatts", 13.0, Face::Bold) + 0.5, 28.0, Face::Bold, c_blue());
    c.text("CALCULATOR  \u{00b7}  NREL PVWATTS v8", 4.6, SIDEBAR_X, 48.0, Face::Reg, c_gray_dk());

    let size = 5.0;
    let lead = 6.7;
    let mut y = 70.2;
    let para = |c: &Canvas, s: &str, y: &mut f32| {
        for ln in wrap(s, size, Face::Reg, SIDEBAR_W) {
            c.text(&ln, size, SIDEBAR_X, *y, Face::Reg, c_text());
            *y += lead;
        }
        *y += 6.7;
    };
    para(c, CAUTION, &mut y);
    para(c, EXPECTED_RANGE, &mut y);
    c.hrule(SIDEBAR_X, SIDEBAR_X + SIDEBAR_W, y - 3.0, c_rule_soft());
    y += 6.0;
    para(c, DISCLAIMER_1, &mut y);
    para(c, DISCLAIMER_2, &mut y);
    para(c, DISCLAIMER_3, &mut y);
    para(c, DISCLAIMER_4, &mut y);

    // CRS audit strip — the part of this page that is ours, kept out of the
    // cloned column so the results block stays a faithful reproduction.
    let mut y = y.max(600.0);
    c.hrule(SIDEBAR_X, SIDEBAR_X + SIDEBAR_W, y, c_rule_soft());
    y += 6.0;
    c.text("CRS AUDIT RECORD", 5.5, SIDEBAR_X, y, Face::Bold, c_gray_dk());
    y += 8.0;
    let kv = |c: &Canvas, k: &str, v: String, y: &mut f32| {
        c.text(k, 4.6, SIDEBAR_X, *y, Face::Bold, c_gray_dk());
        for ln in wrap(&v, 4.6, Face::Reg, SIDEBAR_W) {
            *y += 5.4;
            c.text(&ln, 4.6, SIDEBAR_X, *y, Face::Reg, c_text());
        }
        *y += 7.0;
    };
    kv(c, "Generated", generated.to_string(), &mut y);
    kv(c, "Job / Lot", format!("{} / {}", opt(&bundle.job), opt(&bundle.lot)), &mut y);
    kv(c, "Address", opt(&bundle.lot_addr), &mut y);
    kv(c, "Builder", opt(&bundle.builder), &mut y);
    kv(c, "Community", opt(&bundle.job_name), &mut y);
    if let Some(note) = array_note {
        kv(c, "Array", note, &mut y);
    }
    kv(c, "Lot total", format!("{} kWh/yr", thousands(pv.lot_total_kwh)), &mut y);
    kv(c, "Creatio", if is_candidate {
        "Candidate \u{2014} written to CrsEstAnnualKwhProductionLot".to_string()
    } else {
        "Audit only \u{2014} not written".to_string()
    }, &mut y);
}

// ── the cloned NREL results page ────────────────────────────────────────────
fn kv_section(c: &Canvas, title: &str, hdr_y: f32, rows: &[(String, String, bool)]) -> f32 {
    c.hrule(MAIN_X0, MAIN_X1, hdr_y - 6.4, c_rule());
    c.text(title, 8.9, SECTION_HDR_X, hdr_y, Face::Bold, c_blue());
    let start = hdr_y + 17.0;
    c.hrule(MAIN_X0, MAIN_X1, start - 0.5, c_rule());
    for (i, (k, v, italic)) in rows.iter().enumerate() {
        let top = start + i as f32 * ROW_H;
        c.text(k, 7.8, KV_LABEL_X, top + 4.6, Face::Bold, c_text());
        let face = if *italic { Face::Oblique } else { Face::Bold };
        c.text(v, 7.8, KV_VALUE_X, top + 4.6, face, c_text());
        c.hrule(MAIN_X0, MAIN_X1, top + ROW_H - 0.5, c_rule());
    }
    start + rows.len() as f32 * ROW_H
}

fn draw_results_page(c: &Canvas, pv: &PvResult, a: &PvArray) {
    c.vrule(VRULE_L, VRULE_TOP, VRULE_BOT, c_vrule());
    c.vrule(VRULE_R, VRULE_TOP, VRULE_BOT, c_vrule());

    // ── headline ──
    c.text("RESULTS", 22.2, MAIN_X0, 36.0, Face::Bold, c_orange());
    let headline = a
        .ac_annual
        .map(|v| thousands(v.round() as i64))
        .unwrap_or_else(|| "\u{2014}".to_string());
    c.text_right(&headline, 33.3, 474.9, 40.6, Face::Bold, c_navy());
    // Sized to the reference's measured extent (474.9..552.6pt) rather than its
    // nominal point size: NREL sets this in a condensed face that Helvetica
    // would overrun past the right rule.
    c.text(" kWh/Year*", 16.0, 476.0, 51.5, Face::Reg, c_gray_dk());

    // The NREL page prints an expected-output range here. It is NOT in the v8
    // JSON response (it comes from 30 years of historical weather, web UI only),
    // so rather than approximate it the slot carries the inputs that make this
    // run differ from a plain web run.
    let losses = a.losses.map(trim_num).unwrap_or_else(|| "?".into());
    let soil = a
        .soiling_monthly
        .as_ref()
        .and_then(|s| s.first().copied())
        .map(trim_num)
        .unwrap_or_else(|| "0".into());
    c.text_right(
        &format!("* CRS standard inputs: {losses}% system losses, {soil}%/month soiling."),
        7.0, MAIN_X1, 79.4, Face::Oblique, c_italic(),
    );
    c.text_right(
        "A PVWatts web run without the soiling allowance reads higher.",
        7.0, MAIN_X1, 88.0, Face::Oblique, c_italic(),
    );

    // ── monthly table ──
    let c0 = (MAIN_X0 + COL_MONTH_X1) / 2.0;
    let c1 = (COL_MONTH_X1 + COL_RAD_X1) / 2.0;
    let c2 = (COL_RAD_X1 + MAIN_X1) / 2.0;

    c.hrule(MAIN_X0, MAIN_X1, 97.4, c_rule());
    c.text_center("Month", 8.9, c0, 101.0, Face::Bold, c_blue());
    c.text_center("Solar Radiation", 8.9, c1, 101.0, Face::Bold, c_blue());
    c.text_center("AC Energy", 8.9, c2, 101.0, Face::Bold, c_blue());

    // "( kWh / m² / day )" with a real raised 2, as the page sets it.
    let (p1, p2, p3) = ("( kWh / m", "2", " / day )");
    let total = text_w(p1, 6.7, Face::Bold) + text_w(p2, 5.0, Face::Bold) + text_w(p3, 6.7, Face::Bold);
    let mut x = c1 - total / 2.0;
    c.text(p1, 6.7, x, 116.9, Face::Bold, c_text2());
    x += text_w(p1, 6.7, Face::Bold);
    c.text(p2, 5.0, x, 113.6, Face::Bold, c_text2());
    x += text_w(p2, 5.0, Face::Bold);
    c.text(p3, 6.7, x, 116.9, Face::Bold, c_text2());
    c.text_center("( kWh )", 6.7, c2, 113.6, Face::Bold, c_text2());
    c.hrule(MAIN_X0, MAIN_X1, 129.1, c_rule());

    let ac = a.ac_monthly.clone().unwrap_or_default();
    let rad = a.solrad_monthly.clone().unwrap_or_default();
    for i in 0..12 {
        let top = 129.6 + i as f32 * ROW_H;
        c.text_center(MONTHS[i], 8.9, c0, top + 3.1, Face::Bold, c_blue());
        if let Some(v) = rad.get(i) {
            c.text_center(&format!("{v:.2}"), 7.8, c1, top + 4.7, Face::Bold, c_text());
        }
        if let Some(v) = ac.get(i) {
            c.text_center(&thousands(v.round() as i64), 7.8, c2, top + 4.7, Face::Bold, c_text());
        }
    }

    // Annual row. The AC figure is the sum of the printed cells so the column
    // adds up; see PvArray::ac_annual_displayed.
    c.hrule(MAIN_X0, MAIN_X1, 343.1, c_rule());
    c.text("Annual", 10.0, SECTION_HDR_X, 349.0, Face::Bold, c_orange());
    if !rad.is_empty() {
        let mean = rad.iter().sum::<f64>() / rad.len() as f64;
        let annual = a.solrad_annual.unwrap_or(mean);
        c.text_center(&format!("{annual:.2}"), 10.0, c1, 349.0, Face::Bold, c_orange());
    }
    if let Some(v) = a.ac_annual_displayed() {
        c.text_center(&thousands(v), 10.0, c2, 349.0, Face::Bold, c_orange());
    }
    c.hrule(MAIN_X0, MAIN_X1, 365.8, c_rule());

    // ── location and station ──
    let st = pv.station_info.clone().unwrap_or_default();
    let slat = st.lat.unwrap_or(pv.lat);
    let slon = st.lon.unwrap_or(pv.lon);
    let ns = if slat >= 0.0 { "N" } else { "S" };
    let ew = if slon < 0.0 { "W" } else { "E" };
    kv_section(c, "Location and Station Identification", 383.9, &[
        ("Requested Location".into(), format!("{}, USA", pv.zip), false),
        ("Weather Data Source".into(), format!("Lat, Lng: {:.2}, {:.2}", slat, slon), false),
        ("Latitude".into(), format!("{:.2}\u{00b0} {ns}", slat.abs()), false),
        ("Longitude".into(), format!("{:.2}\u{00b0} {ew}", slon.abs()), false),
    ]);
    if let Some(mi) = st.distance_mi() {
        c.text(&format!("{mi:.1} mi"), 7.8, 442.3, 423.3, Face::Bold, c_text());
    }

    // ── PV system specifications ──
    let f = |o: Option<f64>| o.map(trim_num).unwrap_or_else(|| "\u{2014}".to_string());
    kv_section(c, "PV System Specifications", 478.4, &[
        ("DC System Size".into(), format!("{} kW", trim_num(a.system_capacity_kw)), false),
        ("Module Type".into(), a.module_type_label().to_string(), false),
        ("Array Type".into(), a.array_type_label().to_string(), false),
        ("System Losses".into(), format!("{}%", f(a.losses)), false),
        ("Array Tilt".into(), deg(a.tilt), false),
        ("Array Azimuth".into(), deg(a.azimuth), false),
        ("DC to AC Size Ratio".into(), f(a.dc_ac_ratio), false),
        ("Inverter Efficiency".into(), format!("{}%", f(a.inv_eff)), false),
        ("Ground Coverage Ratio".into(), f(a.gcr), false),
        ("Albedo".into(), "From weather file".into(), true),
        ("Bifacial".into(), "No (0)".into(), false),
    ]);

    // ── monthly irradiance loss (the soiling array) ──
    let soil = a.soiling_monthly.clone().unwrap_or_else(|| vec![0.0; 12]);
    c.text("Monthly Irradiance Loss", 7.8, KV_LABEL_X, 722.3, Face::Bold, c_text());
    for half in 0..2 {
        let (lbl_y, val_y) = if half == 0 { (700.0, 713.4) } else { (731.1, 744.5) };
        for i in 0..6 {
            let idx = half * 6 + i;
            let cx = 358.6 + i as f32 * 24.4;
            c.text_center(MONTHS_ABBR[idx], 7.8, cx, lbl_y, Face::Bold, c_text());
            let v = soil.get(idx).copied().unwrap_or(0.0);
            c.text_center(&format!("{}%", trim_num(v)), 7.8, cx, val_y, Face::Bold, c_text());
        }
    }
    c.hrule(MAIN_X0, MAIN_X1, 755.4, c_rule());
}

/// Page 2 of an NREL block — the Performance Metrics table the browser print
/// spills onto its own page.
fn draw_metrics_page(c: &Canvas, a: &PvArray) {
    c.vrule(VRULE_L, 28.5, 70.2, c_vrule());
    c.vrule(VRULE_R, 28.5, 70.2, c_vrule());
    c.hrule(MAIN_X0, MAIN_X1, 28.5, c_rule());
    c.text("Performance Metrics", 8.9, SECTION_HDR_X, 34.9, Face::Bold, c_blue());
    c.hrule(MAIN_X0, MAIN_X1, 51.3, c_rule());
    c.text("DC Capacity Factor", 7.8, KV_LABEL_X, 56.5, Face::Bold, c_text());
    let cf = a
        .capacity_factor
        .map(|v| format!("{v:.1}%"))
        .unwrap_or_else(|| "\u{2014}".to_string());
    c.text(&cf, 7.8, KV_VALUE_X, 56.5, Face::Bold, c_text());
    c.hrule(MAIN_X0, MAIN_X1, 69.6, c_rule());
}

// ── CRS summary page ────────────────────────────────────────────────────────
// PVWatts models one array per run, so the lot roll-up that actually goes to
// Creatio has no equivalent on the NREL page. It gets its own page rather than
// being wedged into the cloned layout.
fn draw_summary_page(c: &Canvas, bundle: &LotBundle, pv: &PvResult, is_candidate: bool,
                     variant_label: Option<&str>, generated: &str) {
    const L: f32 = 56.7;
    const R: f32 = 555.3;
    let mut y = 56.0;

    c.text("PVWatts Generation Report", 18.0, L, y, Face::Bold, c_navy());
    y += 24.0;
    c.text("Lot summary \u{2014} CRS record of the run", 9.0, L, y, Face::Reg, c_gray_dk());
    c.text_right(generated, 9.0, R, y, Face::Reg, c_gray_dk());
    y += 16.0;
    c.hrule(L, R, y, c_rule());
    y += 14.0;

    let section = |c: &Canvas, t: &str, y: &mut f32| {
        c.text(t, 10.0, L, *y, Face::Bold, c_blue());
        *y += 15.0;
    };
    let row = |c: &Canvas, k: &str, v: &str, y: &mut f32| {
        c.text(k, 8.5, L, *y, Face::Bold, c_text());
        c.text(v, 8.5, L + 130.0, *y, Face::Reg, c_text());
        *y += 13.0;
    };

    section(c, "Lot", &mut y);
    row(c, "Job / Lot", &format!("{} / {}", opt(&bundle.job), opt(&bundle.lot)), &mut y);
    row(c, "Address", &opt(&bundle.lot_addr), &mut y);
    row(c, "Builder", &opt(&bundle.builder), &mut y);
    row(c, "Community / Job", &opt(&bundle.job_name), &mut y);
    row(c, "Zip", &format!("{}  (lat {:.4}, lon {:.4})", pv.zip, pv.lat, pv.lon), &mut y);
    if let Some(v) = variant_label {
        row(c, "Options variant", v, &mut y);
    }
    y += 8.0;

    section(c, "Creatio system inputs", &mut y);
    if let Some(sys) = &bundle.system {
        row(c, "Panel", &format!("{}  \u{00d7} {}", opt(&sys.panel_model),
            sys.panel_qty.map(|q| q.to_string()).unwrap_or_else(|| "?".into())), &mut y);
        row(c, "Inverter", &opt(&sys.inverter_model), &mut y);
        row(c, "Inverter efficiency", &bundle.inverter_efficiency
            .map(|e| format!("{e}%")).unwrap_or_else(|| "?".into()), &mut y);
        row(c, "System size DC", &sys.size_dc.map(|d| format!("{d:.2} kW"))
            .unwrap_or_else(|| "?".into()), &mut y);
        if bundle.options_lot {
            row(c, "Committed system", "unsized (0 panels / 0 kW) \u{2014} options lot, size chosen from the plan block", &mut y);
        }
    } else {
        row(c, "Committed system", "none \u{2014} options lot, size chosen from the plan block", &mut y);
    }
    y += 8.0;

    // ── per-array table ──
    section(c, "Arrays modelled", &mut y);
    let cols = [L, 150.0, 225.0, 300.0, 400.0, 490.0];
    let hdr = ["Array", "kW DC", "Tilt", "Azimuth", "AC kWh/yr", "Cap. factor"];
    for (i, h) in hdr.iter().enumerate() {
        if i == 0 {
            c.text(h, 8.0, cols[i], y, Face::Bold, c_gray_dk());
        } else {
            c.text_right(h, 8.0, cols[i], y, Face::Bold, c_gray_dk());
        }
    }
    y += 4.0;
    c.hrule(L, R, y + 6.0, c_rule());
    y += 12.0;
    let mut sum_kw = 0.0f64;
    for (i, a) in pv.arrays.iter().enumerate() {
        sum_kw += a.system_capacity_kw;
        c.text(&format!("Array {}", i + 1), 8.5, cols[0], y, Face::Reg, c_text());
        c.text_right(&trim_num(a.system_capacity_kw), 8.5, cols[1], y, Face::Reg, c_text());
        c.text_right(&deg(a.tilt), 8.5, cols[2], y, Face::Reg, c_text());
        c.text_right(&deg(a.azimuth), 8.5, cols[3], y, Face::Reg, c_text());
        c.text_right(&a.ac_annual.map(|v| thousands(v.round() as i64))
            .unwrap_or_else(|| "\u{2014}".into()), 8.5, cols[4], y, Face::Reg, c_text());
        c.text_right(&a.capacity_factor.map(|v| format!("{v:.1}%"))
            .unwrap_or_else(|| "\u{2014}".into()), 8.5, cols[5], y, Face::Reg, c_text());
        y += 13.0;
        for w in a.warnings.iter().chain(a.errors.iter()) {
            c.text(&format!("\u{2022} {w}"), 7.0, cols[0] + 10.0, y, Face::Oblique, c_orange());
            y += 10.0;
        }
    }
    c.hrule(L, R, y + 1.0, c_rule());
    y += 10.0;
    c.text("LOT TOTAL", 10.0, cols[0], y, Face::Bold, c_orange());
    c.text_right(&trim_num(sum_kw), 10.0, cols[1], y, Face::Bold, c_orange());
    c.text_right(&format!("{} kWh/yr", thousands(pv.lot_total_kwh)), 10.0, cols[4], y, Face::Bold, c_orange());
    y += 20.0;

    // Reconcile the split against what Creatio holds — a mismatch here is the
    // usual sign the team split the wrong panel count.
    if let Some(sz) = bundle.system.as_ref().and_then(|s| s.size_dc).filter(|sz| *sz > 0.0) {
        let delta = sum_kw - sz;
        let msg = if delta.abs() < 0.005 {
            format!("Array split matches the Creatio system size ({sz:.2} kW).")
        } else {
            format!("Array split is {:.2} kW against Creatio's {:.2} kW \u{2014} delta {:+.2} kW.", sum_kw, sz, delta)
        };
        c.text(&msg, 8.0, L, y, Face::Oblique, if delta.abs() < 0.005 { c_gray_dk() } else { c_orange() });
        y += 18.0;
    }

    section(c, "Creatio writeback", &mut y);
    let wb = if is_candidate {
        format!("YES \u{2014} {} kWh written to CrsEstAnnualKwhProductionLot", thousands(pv.lot_total_kwh))
    } else {
        "No \u{2014} audit only, Creatio not modified".to_string()
    };
    row(c, "Status", &wb, &mut y);
    y += 8.0;

    section(c, "Weather station", &mut y);
    let st = pv.station_info.clone().unwrap_or_default();
    row(c, "Source", &st.weather_data_source.clone().unwrap_or_else(|| "\u{2014}".into()), &mut y);
    row(c, "Station", &format!("{} {}  ({})",
        st.city.clone().unwrap_or_default(),
        st.state.clone().unwrap_or_else(|| "\u{2014}".into()),
        st.solar_resource_file.clone().unwrap_or_else(|| "\u{2014}".into())), &mut y);
    if let Some(mi) = st.distance_mi() {
        row(c, "Distance", &format!("{mi:.1} mi from the lot zip centroid"), &mut y);
    }
    row(c, "NREL key source", &pv.api_key_source, &mut y);
}

// ── document assembly ───────────────────────────────────────────────────────
/// Render the report to bytes: one two-page NREL-style block per modelled
/// array, then the CRS lot summary.
pub fn render(
    bundle: &LotBundle,
    _arrays: &[ArrayInput],
    pv: &PvResult,
    is_candidate: bool,
    variant_label: Option<&str>,
) -> Result<Vec<u8>> {
    let (w, h) = (Mm(215.9), Mm(279.4)); // US Letter
    let (doc, page0, layer0) = PdfDocument::new("PVWatts Generation Report", w, h, "Layer 1");
    let reg = doc.add_builtin_font(BuiltinFont::Helvetica)?;
    let bold = doc.add_builtin_font(BuiltinFont::HelveticaBold)?;
    let obl = doc.add_builtin_font(BuiltinFont::HelveticaOblique)?;
    let generated = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();

    let needed = pv.arrays.len() * 2 + 1;
    let mut slots = vec![(page0, layer0)];
    for _ in 1..needed {
        slots.push(doc.add_page(w, h, "Layer 1"));
    }

    let canvas = |i: usize| Canvas {
        layer: doc.get_page(slots[i].0).get_layer(slots[i].1),
        reg: &reg,
        bold: &bold,
        obl: &obl,
    };

    let n = pv.arrays.len();
    let mut slot = 0usize;
    for (i, a) in pv.arrays.iter().enumerate() {
        let c = canvas(slot);
        slot += 1;
        let note = if n > 1 { Some(format!("Array {} of {}", i + 1, n)) } else { None };
        draw_sidebar(&c, bundle, pv, &generated, note, is_candidate);
        draw_results_page(&c, pv, a);

        let c = canvas(slot);
        slot += 1;
        draw_metrics_page(&c, a);
    }

    let c = canvas(slot);
    draw_summary_page(&c, bundle, pv, is_candidate, variant_label, &generated);

    let mut buf = BufWriter::new(Vec::new());
    doc.save(&mut buf)?;
    Ok(buf.into_inner()?)
}

/// Build the audit PDF and write it into `dir` (must already exist).
/// Returns the file path written.
pub fn write_audit_pdf(
    dir: &Path,
    bundle: &LotBundle,
    arrays: &[ArrayInput],
    pv: &PvResult,
    is_candidate: bool,
    variant_label: Option<&str>,
) -> Result<PathBuf> {
    let path = dir.join(format!("{}.pdf", lot_stem(bundle, variant_label)));

    let bytes = render(bundle, arrays, pv, is_candidate, variant_label)?;
    let file = fs::File::create(&path).with_context(|| format!("could not write {}", path.display()))?;
    let mut w = BufWriter::new(file);
    use std::io::Write;
    w.write_all(&bytes)?;
    w.flush()?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_groups() {
        assert_eq!(thousands(7425), "7,425");
        assert_eq!(thousands(350), "350");
        assert_eq!(thousands(1234567), "1,234,567");
    }

    #[test]
    fn degrees_drop_trailing_zero() {
        assert_eq!(deg(18.0), "18\u{00b0}");
        assert_eq!(deg(213.0), "213\u{00b0}");
        assert_eq!(deg(18.5), "18.5\u{00b0}");
    }

    #[test]
    fn wraps_within_width() {
        for line in wrap(CAUTION, 5.0, Face::Reg, SIDEBAR_W) {
            assert!(text_w(&line, 5.0, Face::Reg) <= SIDEBAR_W + 0.01, "overflow: {line}");
        }
    }
}

#[cfg(test)]
mod sample {
    use super::*;
    use crate::creatio::SystemDetail;
    use crate::sidecar::{PvArray, StationInfo};

    /// Visual check, not an assertion: renders the Castle &amp; Cooke Highgate 65
    /// Lot 3 figures (the reference NREL print) to $PVWATTS_SAMPLE_OUT so the
    /// layout can be diffed page-for-page against the real thing.
    ///
    ///   cargo test --offline -- --ignored render_reference_sample
    /// The document is N two-page blocks plus a summary; N == 0 (a run that
    /// returned nothing) must still produce a valid one-page report.
    #[test]
    fn renders_for_any_array_count() {
        for n in [0usize, 1, 3] {
            let bundle = LotBundle {
                lot_id: "id".into(), job: None, lot: None, lot_addr: None, zip: None,
                plan: None, builder: None, job_name: None, community_id: None, system: None,
                system_count: 0, options_lot: true, inverter_efficiency: None, wattage: None,
            };
            let pv = PvResult {
                ok: true, zip: "93311".into(), lat: 35.3, lon: -119.1,
                api_key_source: "env".into(), lot_total_kwh: 0,
                arrays: vec![PvArray {
                    system_capacity_kw: 4.0, tilt: 20.0, azimuth: 180.0,
                    inv_eff: None, losses: None, module_type: None, array_type: None,
                    dc_ac_ratio: None, gcr: None, soiling_monthly: None,
                    ac_annual: None, ac_monthly: None, solrad_annual: None,
                    solrad_monthly: None, poa_monthly: None, dc_monthly: None,
                    capacity_factor: None, errors: vec![], warnings: vec![],
                }; n],
                defaults: None, station_info: None,
            };
            let bytes = render(&bundle, &[], &pv, false, None)
                .unwrap_or_else(|e| panic!("render failed for {n} arrays: {e}"));
            assert!(bytes.len() > 1000, "suspiciously small output for {n} arrays");
        }
    }

    #[test]
    #[ignore]
    fn render_reference_sample() {
        let out = std::env::var("PVWATTS_SAMPLE_OUT")
            .unwrap_or_else(|_| "target/sample".to_string());

        let bundle = LotBundle {
            lot_id: "00000000-0000-0000-0000-000000000000".into(),
            job: Some("7354-6".into()),
            lot: Some("3".into()),
            lot_addr: Some("Lot 3".into()),
            zip: Some("93311".into()),
            plan: Some("9D-R".into()),
            builder: Some("Castle & Cooke".into()),
            job_name: Some("Highgate 65 Series".into()),
            community_id: None,
            system: Some(SystemDetail {
                name: Some("Lot 3 system".into()),
                panel_model: Some("Q Cell 410".into()),
                panel_qty: Some(11),
                inverter_model: Some("Enphase IQ8MC".into()),
                inverter_guid: None,
                size_dc: Some(4.51),
                size_ac: Some(3.76),
            }),
            system_count: 1,
            options_lot: false,
            inverter_efficiency: Some(96.0),
            wattage: Some(410),
        };

        // Verbatim from the reference print / a live v8 call with its inputs.
        let arr = PvArray {
            system_capacity_kw: 4.51,
            tilt: 18.0,
            azimuth: 213.0,
            inv_eff: Some(96.0),
            losses: Some(14.08),
            module_type: Some(1),
            array_type: Some(1),
            dc_ac_ratio: Some(1.2),
            gcr: Some(0.4),
            soiling_monthly: Some(vec![0.0; 12]),
            ac_annual: Some(7424.606084647787),
            ac_monthly: Some(vec![
                350.0, 455.0, 642.0, 716.0, 830.0, 808.0, 785.0, 764.0, 682.0, 593.0, 440.0, 359.0,
            ]),
            solrad_annual: Some(6.086298463808716),
            solrad_monthly: Some(vec![
                3.17, 4.65, 6.00, 7.07, 7.99, 8.31, 7.93, 7.70, 7.01, 5.68, 4.24, 3.28,
            ]),
            poa_monthly: Some(vec![98.2; 12]),
            dc_monthly: Some(vec![370.0; 12]),
            capacity_factor: Some(18.79285525986839),
            errors: vec![],
            warnings: vec![],
        };

        // Optional fan-out so the N-array structure can be eyeballed too:
        //   PVWATTS_SAMPLE_ARRAYS=3 cargo test --offline -- --ignored render_reference_sample
        let n: usize = std::env::var("PVWATTS_SAMPLE_ARRAYS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        let arrays: Vec<PvArray> = (0..n)
            .map(|i| {
                let mut a = arr.clone();
                a.azimuth = 180.0 + 15.0 * i as f64;
                a
            })
            .collect();

        let pv = PvResult {
            ok: true,
            zip: "93311".into(),
            lat: 35.33,
            lon: -119.14,
            api_key_source: "env".into(),
            lot_total_kwh: 7425,
            arrays,
            defaults: None,
            station_info: Some(StationInfo {
                lat: Some(35.33),
                lon: Some(-119.14),
                elev: Some(110.65),
                tz: Some(-8.0),
                location: Some("96251".into()),
                city: Some(String::new()),
                state: Some("California".into()),
                country: Some("United States".into()),
                solar_resource_file: Some("96251.csv".into()),
                distance: Some(2092.0),
                weather_data_source: Some("NSRDB PSM V3 GOES tmy-2020 3.2.0".into()),
            }),
        };

        let bytes = render(&bundle, &[], &pv, true, None).expect("render");
        std::fs::write(format!("{out}.pdf"), &bytes).expect("write pdf");
        let csv = crate::csv::render(&bundle, &pv, true, None, "2026-08-27 12:00", &crate::csv::Submission { key: "sample-v1".into(), version: 1 });
        std::fs::write(format!("{out}.csv"), csv.as_bytes()).expect("write csv");
        eprintln!("wrote {out}.pdf and {out}.csv");
    }
}
