//! Sidecar client — the exe's only "open internet" dependency.
//!
//! Coworker PCs reach Creatio + this sidecar (both corporate-reachable); the
//! sidecar (Render, open egress) does the NREL PVWatts calls and the Claude
//! parse-assist. This keeps the NREL/Anthropic keys server-side (never on a
//! coworker machine) and the exe tiny.
//!
//! Endpoints (POST, x-api-key):
//!   /run/pvwatts            -> zip + inv_eff + arrays  => per-array + lot-total kWh
//!   /run/parse_system_sizes -> plan_code + block       => candidate sizes (options branch)

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

const DEFAULT_BASE: &str = "https://crs-n8n-tools-api.onrender.com";

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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PvArray {
    pub system_capacity_kw: f64,
    pub tilt: f64,
    pub azimuth: f64,
    pub ac_annual: Option<f64>,
    pub ac_monthly: Option<Vec<f64>>,
    pub solrad_annual: Option<f64>,
    #[serde(default)]
    pub errors: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
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
    pub station_info: Option<serde_json::Value>,
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
    /// Dev: SIDECAR_BASE_URL (default prod) + N8N_TOOLS_API_KEY from env.
    pub fn from_env() -> Result<Self> {
        let base = std::env::var("SIDECAR_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE.to_string());
        let api_key = std::env::var("N8N_TOOLS_API_KEY").context("N8N_TOOLS_API_KEY not set")?;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(90))
            .build()?;
        Ok(Self { client, base, api_key })
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
