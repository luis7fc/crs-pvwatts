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
    fn computes_kw() {
        assert_eq!(array_kw_dc(12, 455), 5.46);
    }
}
