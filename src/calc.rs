//! Business math that turns team-entered + Creatio data into PVWatts inputs.

/// Pull the panel wattage out of a Creatio module model string.
/// e.g. "CAN 455" -> 455, "Q.PEAK DUO BLK ML-G10+ 410" -> 410.
/// Rule (confirm w/ Luis): the wattage is the last standalone integer in the name.
pub fn wattage_from_model(model: &str) -> Option<u32> {
    model
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<u32>().ok())
        .filter(|w| (250..=800).contains(w)) // sanity band for a PV module, avoids matching "G10"
        .last()
}

/// Modules whose Creatio name carries no wattage (solar shingles etc.).
/// Matched case-insensitively on the full name.
const KNOWN_WATTAGE: &[(&str, u32)] = &[
    ("GAF Energy TLS-2", 57), // Timberline Solar shingle; 129 x 57 W = 7.35 kW in Creatio
];

/// Wattage for a lot's system: name rule first, then the known-models table,
/// then Creatio's own size (kW DC / panel qty, rounded to whole watts).
pub fn resolve_wattage(model: Option<&str>, size_dc: Option<f64>, panel_qty: Option<u32>) -> Option<u32> {
    if let Some(m) = model {
        if let Some(w) = wattage_from_model(m) {
            return Some(w);
        }
        let m = m.trim();
        if let Some((_, w)) = KNOWN_WATTAGE.iter().find(|(n, _)| n.eq_ignore_ascii_case(m)) {
            return Some(*w);
        }
    }
    match (size_dc, panel_qty) {
        (Some(kw), Some(q)) if kw > 0.0 && q > 0 => {
            let w = (kw * 1000.0 / q as f64).round() as u32;
            (w > 0).then_some(w)
        }
        _ => None,
    }
}

/// kW DC for one array = panel_count * watts_per_panel / 1000.
pub fn array_kw_dc(panel_count: u32, watts_per_panel: u32) -> f64 {
    (panel_count * watts_per_panel) as f64 / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_model() {
        assert_eq!(wattage_from_model("CAN 455"), Some(455));
    }

    #[test]
    fn ignores_noise_numbers() {
        // "G10" must not win; the 410 is the real wattage.
        assert_eq!(wattage_from_model("Q.PEAK DUO BLK ML-G10+ 410"), Some(410));
    }

    #[test]
    fn gaf_shingle_uses_known_table() {
        assert_eq!(wattage_from_model("GAF Energy TLS-2"), None);
        assert_eq!(resolve_wattage(Some("GAF Energy TLS-2"), None, None), Some(57));
        assert_eq!(resolve_wattage(Some("gaf energy tls-2 "), Some(0.0), Some(0)), Some(57));
    }

    #[test]
    fn unknown_unnumbered_model_derives_from_creatio_size() {
        // 129 panels, 7.35 kW -> 56.98 -> 57 W
        assert_eq!(resolve_wattage(Some("Mystery Shingle"), Some(7.35), Some(129)), Some(57));
        assert_eq!(resolve_wattage(Some("Mystery Shingle"), None, Some(129)), None);
        assert_eq!(resolve_wattage(None, Some(4.16), Some(0)), None);
    }

    #[test]
    fn name_rule_still_wins() {
        assert_eq!(resolve_wattage(Some("Silfab SIL-440 QD"), Some(9.99), Some(10)), Some(440));
    }

    #[test]
    fn computes_kw() {
        assert_eq!(array_kw_dc(12, 455), 5.46);
    }
}
