//! Lot list (pool) filters, read from a plain text file on the I: drive so the
//! team can change them without a new release. Re-read on every pool load, so
//! an edit takes effect on the next Refresh.
//!
//! Format: `key = value`, one per line; `#` starts a comment; keys are not case
//! sensitive; lists are comma-separated; yes/no for switches. A missing key keeps
//! its built-in default, so an empty or missing file behaves exactly like 2.1.2.
//! Mistakes (unknown key, bad yes/no) are reported in the UI, never fatal.
//!
//! Override the file location with PVWATTS_FILTERS_FILE.

use std::fs;
use std::path::PathBuf;

use serde::Serialize;

pub const DEFAULT_FILE: &str =
    r"I:\Solar\Field Ops\Field Operations Admin Processes\API_TOOLS\pvwatts\pvwatts_filters.txt";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PoolFilters {
    /// Lot channels (UsrChannel display values) shown. Empty = any channel (`*`).
    pub channels: Vec<String>,
    pub require_buyer_info_received: bool,
    pub require_consultation_not_complete: bool,
    pub require_no_kwh_yet: bool,
    /// Community (Opportunity title) names hidden. Lowercased.
    pub exclude_communities: Vec<String>,
    /// Builder (Opportunity.Account) names hidden. Lowercased.
    pub exclude_builders: Vec<String>,
    /// Job numbers (UsrJobNumber) hidden. Lowercased.
    pub exclude_jobs: Vec<String>,
}

impl Default for PoolFilters {
    fn default() -> Self {
        PoolFilters {
            channels: vec!["integrated".into(), "cash only".into()],
            require_buyer_info_received: true,
            require_consultation_not_complete: true,
            require_no_kwh_yet: true,
            exclude_communities: vec![],
            exclude_builders: vec![],
            exclude_jobs: vec![],
        }
    }
}

fn norm(s: &str) -> String {
    s.trim().to_lowercase()
}

fn listed(list: &[String], v: Option<&str>) -> bool {
    v.is_some_and(|v| list.contains(&norm(v)))
}

impl PoolFilters {
    pub fn channel_ok(&self, channel: Option<&str>) -> bool {
        self.channels.is_empty() || listed(&self.channels, channel)
    }

    /// True when any exclude list hides this lot.
    pub fn excluded(&self, community: Option<&str>, builder: Option<&str>, job: Option<&str>) -> bool {
        listed(&self.exclude_communities, community)
            || listed(&self.exclude_builders, builder)
            || listed(&self.exclude_jobs, job)
    }
}

/// What was loaded, where from, and anything wrong with the file — for the UI.
#[derive(Debug, Clone, Serialize)]
pub struct Loaded {
    pub filters: PoolFilters,
    pub source: String,
    pub warnings: Vec<String>,
}

pub fn file_path() -> PathBuf {
    std::env::var("PVWATTS_FILTERS_FILE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| crate::share::to_local(s.trim()))
        .unwrap_or_else(|| crate::share::to_local(DEFAULT_FILE))
}

/// Read the filter file; built-in defaults when it's missing or unreadable.
pub fn load() -> Loaded {
    let path = file_path();
    match fs::read_to_string(&path) {
        Ok(text) => {
            let (filters, warnings) = parse(&text);
            Loaded { filters, source: path.display().to_string(), warnings }
        }
        Err(e) => Loaded {
            filters: PoolFilters::default(),
            source: "built-in defaults".into(),
            warnings: vec![format!("filter file not read ({}): {e}", path.display())],
        },
    }
}

fn split_list(v: &str) -> Vec<String> {
    v.split(',').map(norm).filter(|s| !s.is_empty()).collect()
}

fn yes_no(v: &str) -> Option<bool> {
    match norm(v).as_str() {
        "yes" | "y" | "true" | "on" | "1" => Some(true),
        "no" | "n" | "false" | "off" | "0" => Some(false),
        _ => None,
    }
}

pub fn parse(text: &str) -> (PoolFilters, Vec<String>) {
    let mut f = PoolFilters::default();
    let mut warnings = Vec::new();
    for (i, raw) in text.trim_start_matches('\u{feff}').lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let n = i + 1;
        let Some((k, v)) = line.split_once('=') else {
            warnings.push(format!("line {n}: no '=' — ignored: {line}"));
            continue;
        };
        let key = norm(k);
        let mut switch = |slot: &mut bool| match yes_no(v) {
            Some(b) => *slot = b,
            None => warnings.push(format!("line {n}: {key} must be yes or no — kept the default")),
        };
        match key.as_str() {
            "channels" => {
                let list = split_list(v);
                if list.iter().any(|c| c == "*") {
                    f.channels = vec![];
                } else if list.is_empty() {
                    warnings.push(format!("line {n}: channels is empty — kept the default (use * for any channel)"));
                } else {
                    f.channels = list;
                }
            }
            "require_buyer_info_received" => switch(&mut f.require_buyer_info_received),
            "require_consultation_not_complete" => switch(&mut f.require_consultation_not_complete),
            "require_no_kwh_yet" => switch(&mut f.require_no_kwh_yet),
            "exclude_communities" => f.exclude_communities = split_list(v),
            "exclude_builders" => f.exclude_builders = split_list(v),
            "exclude_jobs" => f.exclude_jobs = split_list(v),
            _ => warnings.push(format!("line {n}: unknown setting '{}' — ignored", k.trim())),
        }
    }
    (f, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_is_the_212_behaviour() {
        let (f, w) = parse("# nothing here\n\n");
        assert_eq!(f, PoolFilters::default());
        assert!(w.is_empty());
    }

    #[test]
    fn the_shipped_file_parses_clean() {
        let (f, w) = parse(include_str!("../pvwatts_filters.txt"));
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(f, PoolFilters::default());
    }

    #[test]
    fn settings_lists_and_case() {
        let (f, w) = parse(
            "Channels = Integrated, CASH only , Retail\n\
             require_no_kwh_yet = No   # show re-runs\n\
             exclude_communities = DNU Test Community, Other One\n\
             exclude_builders = KB Home\n\
             exclude_jobs = 70850-026\n",
        );
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(f.channels, ["integrated", "cash only", "retail"]);
        assert!(!f.require_no_kwh_yet && f.require_buyer_info_received);
        assert!(f.channel_ok(Some("Retail")) && !f.channel_ok(Some("Other")) && !f.channel_ok(None));
        assert!(f.excluded(Some("dnu test community"), None, None));
        assert!(f.excluded(None, Some(" KB HOME "), None));
        assert!(f.excluded(None, None, Some("70850-026")));
        assert!(!f.excluded(Some("Millau"), Some("Century Communities"), Some("70850-027")));
    }

    #[test]
    fn star_means_any_channel() {
        let (f, _) = parse("channels = *");
        assert!(f.channel_ok(Some("Anything")) && f.channel_ok(None));
    }

    #[test]
    fn mistakes_warn_and_keep_defaults() {
        let (f, w) = parse("require_buyer_info_received = maybe\nchannels =\nfoo = 1\njust text\n");
        assert_eq!(f, PoolFilters::default());
        assert_eq!(w.len(), 4, "{w:?}");
        assert!(w[0].starts_with("line 1:") && w[3].starts_with("line 4:"));
    }

    #[test]
    fn missing_file_falls_back_to_defaults() {
        std::env::set_var("PVWATTS_FILTERS_FILE", std::env::temp_dir().join("pvw_no_such_filters.txt"));
        let l = load();
        std::env::remove_var("PVWATTS_FILTERS_FILE");
        assert_eq!(l.filters, PoolFilters::default());
        assert_eq!(l.source, "built-in defaults");
        assert_eq!(l.warnings.len(), 1);
    }
}
