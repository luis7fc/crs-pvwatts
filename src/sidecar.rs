//! Sidecar client — the exe's only "open internet" dependency.
//!
//! Coworker PCs reach Creatio + this sidecar (both corporate-reachable); the
//! sidecar (Render, open egress) does the NREL PVWatts calls and the Claude
//! parse-assist. This keeps the NREL/Anthropic keys server-side (never on a
//! coworker machine) and the exe tiny.
//!
//! Endpoints (POST, x-api-key):
//!   /run/pvwatts            -> zip + inv_eff + arrays  => full v8 outputs per array
//!   /run/parse_system_sizes -> plan_code + block       => candidate sizes (options branch)
//!
//! The sidecar returns the whole PVWatts v8 `outputs` block plus an echo of the
//! inputs it actually sent. Everything the cloned NREL results page prints comes
//! from that response — the PDF never re-derives an input, so a defaults change
//! on the sidecar cannot silently desync the page from the run that produced it.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

const DEFAULT_BASE: &str = "https://crs-n8n-tools-api.onrender.com";

#[derive(Clone)]
pub struct Sidecar {
    client: reqwest::Client,
    base: String,
    api_key: String,
}

/// One array to model (team-split of the lot's panels).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArrayInput {
    pub system_capacity_kw: f64,
    pub tilt: f64,
    pub azimuth: f64,
}

// ── /run/pvwatts response ───────────────────────────────────────────────────

/// The run's resolved PVWatts inputs, echoed by the sidecar.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PvDefaults {
    pub module_type: Option<i64>,
    pub array_type: Option<i64>,
    pub losses: Option<f64>,
    pub dc_ac_ratio: Option<f64>,
    pub gcr: Option<f64>,
    pub soiling: Option<f64>,
}

/// Weather station PVWatts actually resolved for the lat/lon.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StationInfo {
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub elev: Option<f64>,
    pub tz: Option<f64>,
    pub location: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub country: Option<String>,
    pub solar_resource_file: Option<String>,
    /// Metres from the requested point to the station.
    pub distance: Option<f64>,
    pub weather_data_source: Option<String>,
}

impl StationInfo {
    /// The NREL page prints the station offset in miles.
    pub fn distance_mi(&self) -> Option<f64> {
        self.distance.map(|m| m / 1609.344)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PvArray {
    pub system_capacity_kw: f64,
    pub tilt: f64,
    pub azimuth: f64,

    // Inputs echoed back by the sidecar (all optional: an older sidecar omits them).
    #[serde(default)]
    pub inv_eff: Option<f64>,
    #[serde(default)]
    pub losses: Option<f64>,
    #[serde(default)]
    pub module_type: Option<i64>,
    #[serde(default)]
    pub array_type: Option<i64>,
    #[serde(default)]
    pub dc_ac_ratio: Option<f64>,
    #[serde(default)]
    pub gcr: Option<f64>,
    /// 12 monthly soiling percentages — the page's "Monthly Irradiance Loss" row.
    #[serde(default)]
    pub soiling_monthly: Option<Vec<f64>>,

    // PVWatts v8 outputs.
    pub ac_annual: Option<f64>,
    pub ac_monthly: Option<Vec<f64>>,
    pub solrad_annual: Option<f64>,
    #[serde(default)]
    pub solrad_monthly: Option<Vec<f64>>,
    #[serde(default)]
    pub poa_monthly: Option<Vec<f64>>,
    #[serde(default)]
    pub dc_monthly: Option<Vec<f64>>,
    #[serde(default)]
    pub capacity_factor: Option<f64>,

    #[serde(default)]
    pub errors: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl PvArray {
    /// PVWatts module_type -> the label the NREL page prints.
    pub fn module_type_label(&self) -> &'static str {
        match self.module_type {
            Some(0) => "Standard",
            Some(1) => "Premium",
            Some(2) => "Thin film",
            _ => "Premium",
        }
    }

    /// PVWatts array_type -> the label the NREL page prints.
    pub fn array_type_label(&self) -> &'static str {
        match self.array_type {
            Some(0) => "Fixed (open rack)",
            Some(1) => "Fixed (roof mount)",
            Some(2) => "1-Axis",
            Some(3) => "1-Axis Backtracking",
            Some(4) => "2-Axis",
            _ => "Fixed (roof mount)",
        }
    }

    /// The page's Annual AC row is the sum of the twelve ROUNDED monthly cells,
    /// not round(ac_annual) — so the printed column always adds up. The NREL
    /// reference PDF shows 7,424 in this row against a 7,425 headline for
    /// exactly that reason. Falls back to ac_annual with no monthly series.
    pub fn ac_annual_displayed(&self) -> Option<i64> {
        match &self.ac_monthly {
            Some(m) if m.len() == 12 => Some(m.iter().map(|v| v.round() as i64).sum()),
            _ => self.ac_annual.map(|v| v.round() as i64),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PvResult {
    pub ok: bool,
    pub zip: String,
    pub lat: f64,
    pub lon: f64,
    pub api_key_source: String,
    pub lot_total_kwh: i64,
    pub arrays: Vec<PvArray>,
    #[serde(default)]
    pub defaults: Option<PvDefaults>,
    #[serde(default)]
    pub station_info: Option<StationInfo>,
}

// ── /run/parse_system_sizes response (options branch) ───────────────────────
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseOption {
    pub kw_dc: f64,
    pub panels: Option<u32>,
    pub label: String,
    pub source_line: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseResult {
    pub ok: bool,
    pub matched: bool,
    pub confidence: String,
    pub normalized_plan: String,
    pub options: Vec<ParseOption>,
    #[serde(default)]
    pub warnings: Vec<String>,
    pub raw_block: String,
}

impl Sidecar {
    pub fn new(base: String, api_key: String) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(90))
            .build()?;
        Ok(Self { client, base, api_key })
    }

    /// Sidecar key resolution: runtime env / pvwatts.env (N8N_TOOLS_API_KEY),
    /// else the value baked at build time (CI sets it as a secret), else error.
    /// Users never enter this — it is the tool's shared key, not a per-user cred.
    pub fn from_env() -> Result<Self> {
        let base = std::env::var("SIDECAR_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE.to_string());
        let api_key = std::env::var("N8N_TOOLS_API_KEY")
            .ok()
            .or_else(|| option_env!("N8N_TOOLS_API_KEY").map(str::to_string))
            .context("N8N_TOOLS_API_KEY not set (env/pvwatts.env, or bake at build)")?;
        Self::new(base, api_key)
    }

    /// The sidecar base URL (for rebuilds after a settings change).
    pub fn base_url(&self) -> &str {
        &self.base
    }

    /// Run PVWatts for a lot's arrays via the sidecar. Returns per-array + summed kWh.
    pub async fn run_pvwatts(&self, zip: &str, inv_eff: f64, arrays: &[ArrayInput]) -> Result<PvResult> {
        let mut body = json!({ "zip": zip, "inv_eff": inv_eff, "arrays": arrays });
        // Dev convenience: pass a local NREL key if present. In prod the sidecar's
        // Render env NREL_API_KEY is used and the exe sends no key.
        if let Ok(k) = std::env::var("NREL_API_KEY") {
            body["api_key"] = json!(k);
        }
        self.post::<PvResult>("/run/pvwatts", &body).await
    }

    /// Options branch: parse a lot plan's size options from the Opportunity block.
    pub async fn parse_system_sizes(&self, plan_code: &str, block: &str) -> Result<ParseResult> {
        let body = json!({ "plan_code": plan_code, "block": block });
        self.post::<ParseResult>("/run/parse_system_sizes", &body).await
    }

    async fn post<T: for<'de> Deserialize<'de>>(&self, path: &str, body: &serde_json::Value) -> Result<T> {
        let resp = self
            .client
            .post(format!("{}{}", self.base, path))
            .header("x-api-key", &self.api_key)
            .json(body)
            .send()
            .await
            .with_context(|| format!("sidecar {path} request failed to send"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("sidecar {} {}: {}", path, status, text.chars().take(600).collect::<String>());
        }
        resp.json::<T>().await.with_context(|| format!("sidecar {path} returned unexpected JSON"))
    }
}
