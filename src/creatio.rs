//! Creatio read layer for the PVWatts tool.
//!
//! Ports the probe-proven access pattern (2026-07-20/21):
//!   - Forms auth: POST /ServiceModel/AuthService.svc/Login -> BPMCSRF cookie+header
//!   - DataService: POST /0/DataService/json/SyncReply/SelectQuery
//!   - Session hygiene: login -> work -> logout (deterministic; see main)
//!
//! Login returns HTTP 200 even on bad/expired creds; success == 200 + a BPMCSRF
//! cookie, so a missing cookie is treated as an auth failure, not a data bug.
//!
//! Memory: we borrow directly into the parsed response (`&Value` rows) and only
//! allocate owned structs at the boundary — no cloning of whole row vectors.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::calc;

const DATA_SERVICE: &str = "/0/DataService/json/SyncReply/SelectQuery";
const UPDATE_SERVICE: &str = "/0/DataService/json/SyncReply/UpdateQuery";
const DVT_INTEGER: i64 = 4; // CrsEstAnnualKwhProductionLot writeback
/// UsrLotRecords column behind the "Estimated Monthly kWh Production" field on the
/// lot page. Written alongside the annual total on commit: the average of the
/// PVWatts monthly AC series, summed across the lot's arrays, rounded.
pub const MONTHLY_KWH_FIELD: &str = "SMEstimatedMonthlyKwHProduction";
const TIMEZONE_OFFSET_MIN: i64 = 480; // Pacific — tenant-local (LA) dates

// dataValueType enums used in equality filters (proven per column in probes)
const DVT_GUID: i64 = 0; // Product.Id, Opportunity.Id
const DVT_TEXT: i64 = 1; // SMSystemDetailObject.SMUsrLotRecords (matches fetch_creatio_lot_data.py)

/// Runtime Creatio credentials. In production these come from the login menu;
/// for dev they load from CREATIO_BASE_URL / CREATIO_USERNAME / CREATIO_PASSWORD.
pub struct CreatioConfig {
    pub base_url: String,
    pub username: String,
    pub password: String,
}

impl CreatioConfig {
    pub fn from_env() -> Result<Self> {
        let base_url = std::env::var("CREATIO_BASE_URL")
            .context("CREATIO_BASE_URL not set")?
            .trim_end_matches('/')
            .to_string();
        Ok(Self {
            base_url,
            username: std::env::var("CREATIO_USERNAME").context("CREATIO_USERNAME not set")?,
            password: std::env::var("CREATIO_PASSWORD").context("CREATIO_PASSWORD not set")?,
        })
    }
}

/// An authenticated Creatio session: a cookie-persisting client + the CSRF token.
pub struct Session {
    client: reqwest::Client,
    base_url: String,
    bpmcsrf: String,
}

/// A selectable lot from the pool (what the UI lists for the team to pick).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolLot {
    pub lot_id: String,
    pub job: Option<String>,
    pub lot: Option<String>,
    pub lot_addr: Option<String>,
    pub zip: Option<String>,
    pub plan: Option<String>,         // UsrLot_PlanElevation — set => possible options lot
    pub community: Option<String>,    // Opportunity display name
    pub community_id: Option<String>, // Opportunity GUID (for builder/job_name fetch)
    pub buyer_info: Option<String>,
}

/// One committed system for a lot (from SMSystemDetailObject). Absent => options branch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemDetail {
    pub name: Option<String>,
    pub panel_model: Option<String>,
    pub panel_qty: Option<u32>, // LOT TOTAL; team splits across arrays in the UI
    pub inverter_model: Option<String>,
    pub inverter_guid: Option<String>,
    pub size_dc: Option<f64>, // reconcile vs sum(array kW)
    pub size_ac: Option<f64>,
}

/// Everything the PVWatts run + audit PDF need for one lot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LotBundle {
    pub lot_id: String,
    pub job: Option<String>,
    pub lot: Option<String>,
    pub lot_addr: Option<String>,
    pub zip: Option<String>,
    pub plan: Option<String>,
    pub builder: Option<String>,   // Opportunity.Account  -> save-path {builder}
    pub job_name: Option<String>,  // Opportunity.Title    -> save-path {job_name}
    pub system: Option<SystemDetail>,
    pub system_count: usize,       // >1 would be unusual (multi-array not modeled in Creatio)
    /// True when the lot takes the options branch: no committed system row, OR a
    /// row whose panel qty and kW DC are both blank/zero (record exists but was
    /// never sized). Single source of truth for the UI's mode switch.
    #[serde(default)]
    pub options_lot: bool,
    pub inverter_efficiency: Option<f64>, // Product.SMInverterEfficiency
    pub wattage: Option<u32>,      // parsed from panel model -> UI live kW = panels*W/1000
}

impl Session {
    pub async fn login(cfg: &CreatioConfig) -> Result<Session> {
        let client = reqwest::Client::builder()
            .cookie_store(true) // persists .ASPXAUTH + BPMCSRF across calls
            .timeout(std::time::Duration::from_secs(60))
            .build()?;

        let url = format!("{}/ServiceModel/AuthService.svc/Login", cfg.base_url);
        let resp = client
            .post(&url)
            .json(&json!({
                "UserName": cfg.username,
                "UserPassword": cfg.password,
                "TimeZoneOffset": TIMEZONE_OFFSET_MIN,
                "ClaimList": [{ "Key": "rememberMeCheckbox", "Value": false }],
            }))
            .send()
            .await
            .context("Creatio login request failed to send")?
            .error_for_status()
            .context("Creatio login returned an HTTP error")?;

        // 200 alone is not success — Creatio 200s on bad creds. Require BPMCSRF.
        let bpmcsrf = resp
            .cookies()
            .find(|c| c.name() == "BPMCSRF")
            .map(|c| c.value().to_string())
            .context("login returned 200 but no BPMCSRF cookie (bad/expired credentials?)")?;

        Ok(Session { client, base_url: cfg.base_url.clone(), bpmcsrf })
    }

    /// Best-effort logout — keep the shared license pool clean. Never panics.
    pub async fn logout(&self) {
        let url = format!("{}/ServiceModel/AuthService.svc/Logout", self.base_url);
        let _ = self.client.post(&url).header("BPMCSRF", &self.bpmcsrf).send().await;
    }

    /// Run a DataService SelectQuery. Logs the response body before failing so a
    /// 500 (unknown schema/column) is never opaque (vault lesson 2026-05-04).
    async fn select(&self, payload: Value) -> Result<Value> {
        let url = format!("{}{}", self.base_url, DATA_SERVICE);
        let resp = self
            .client
            .post(&url)
            .header("BPMCSRF", &self.bpmcsrf)
            .json(&payload)
            .send()
            .await
            .context("DataService request failed to send")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("DataService HTTP {}: {}", status, &body[..body.len().min(1500)]);
        }
        let data: Value = resp.json().await.context("DataService returned non-JSON")?;
        if data.get("success").and_then(Value::as_bool) == Some(false) {
            bail!("DataService success=false: {}", truncate(&data.to_string(), 1000));
        }
        Ok(data)
    }

    /// Fetch the FULL selectable lot pool by paging through all lot records.
    /// Predicate: UsrChannel=Integrated AND UsrBuyerInfoReceived set AND
    /// SMConsultationComplete blank AND CrsEstAnnualKwhProductionLot blank/0.
    ///
    /// Filtered CLIENT-SIDE (the DataService server-side null/lookup filters 500
    /// or silently no-op — vault-confirmed). DataService caps rowCount, so we page
    /// with rowsOffset ordered by Id. ~100k lots -> ~12s, ~281 pending.
    pub async fn fetch_pool(&self) -> Result<Vec<PoolLot>> {
        const COLS: &[&str] = &[
            "Id", "UsrJobNumber", "UsrName", "UsrLotNumberPlusAddress", "UsrZipCode",
            "UsrLot_PlanElevation", "UsrLot_Community", "UsrChannel",
            "UsrBuyerInfoReceived", "SMConsultationComplete", "CrsEstAnnualKwhProductionLot",
        ];
        const CHUNK: i64 = 5000;
        const BATCH: usize = 6; // pages fetched concurrently per round

        let mut pool = Vec::new();
        let mut base: i64 = 0;
        loop {
            // Fire BATCH page requests at once (each page is independent — offset +
            // Id order — so concurrency is safe on the one shared session).
            let futs = (0..BATCH).map(|i| self.select(pool_page_payload(COLS, CHUNK, base + (i as i64) * CHUNK)));
            let results = futures::future::join_all(futs).await;

            let mut done = false;
            for res in results {
                let data = res?;
                let rows = rows_of(&data);
                if rows.len() < CHUNK as usize {
                    done = true; // this page is the last
                }
                for r in rows {
                    let channel_ok = disp(field(r, "UsrChannel")).as_deref() == Some("Integrated");
                    if channel_ok
                        && !blank(field(r, "UsrBuyerInfoReceived"))
                        && blank(field(r, "SMConsultationComplete"))
                        && blank(field(r, "CrsEstAnnualKwhProductionLot"))
                    {
                        pool.push(PoolLot {
                            lot_id: disp(field(r, "Id")).unwrap_or_default(),
                            job: disp(field(r, "UsrJobNumber")),
                            lot: disp(field(r, "UsrName")),
                            lot_addr: disp(field(r, "UsrLotNumberPlusAddress")),
                            zip: disp(field(r, "UsrZipCode")),
                            plan: disp(field(r, "UsrLot_PlanElevation")),
                            community: disp(field(r, "UsrLot_Community")),
                            community_id: guid(field(r, "UsrLot_Community")),
                            buyer_info: disp(field(r, "UsrBuyerInfoReceived")),
                        });
                    }
                }
            }
            base += (BATCH as i64) * CHUNK;
            if done || base > 500_000 {
                break;
            }
        }
        Ok(pool)
    }

    /// Resolve a picked lot's full read bundle: system detail (panel model/qty,
    /// inverter, size) + Product inverter efficiency + Opportunity builder/job_name.
    pub async fn fetch_lot_bundle(&self, lot: &PoolLot) -> Result<LotBundle> {
        // 1) committed system(s) for this lot
        const SYS: &[&str] = &[
            "Id", "SMName", "SMSolarPanelModule", "SMSolarPanelQty",
            "SMInverter", "SMSystemSizeAC", "SMSystemSizeDC",
        ];
        let sys_data = self
            .select(select_payload(
                "SMSystemDetailObject",
                SYS,
                -1,
                Some(equals_filter("SMUsrLotRecords", &lot.lot_id, DVT_TEXT)),
            ))
            .await?;
        let sys_rows = rows_of(&sys_data);
        let system_count = sys_rows.len();
        let system = sys_rows.first().map(|r| SystemDetail {
            name: disp(field(r, "SMName")),
            panel_model: disp(field(r, "SMSolarPanelModule")),
            panel_qty: int(field(r, "SMSolarPanelQty")),
            inverter_model: disp(field(r, "SMInverter")),
            inverter_guid: guid(field(r, "SMInverter")),
            size_dc: num(field(r, "SMSystemSizeDC")),
            size_ac: num(field(r, "SMSystemSizeAC")),
        });

        // 2) inverter efficiency via the SMInverter -> Product join
        let mut inverter_efficiency = None;
        if let Some(g) = system.as_ref().and_then(|s| s.inverter_guid.as_deref()) {
            let pd = self
                .select(select_payload(
                    "Product",
                    &["Id", "Name", "SMInverterEfficiency"],
                    1,
                    Some(equals_filter("Id", g, DVT_GUID)),
                ))
                .await?;
            inverter_efficiency = rows_of(&pd)
                .first()
                .and_then(|p| num(field(p, "SMInverterEfficiency")));
        }

        // 3) Opportunity -> builder (Account) + job_name (Title)
        let (mut builder, mut job_name) = (None, None);
        if let Some(cid) = lot.community_id.as_deref() {
            let od = self
                .select(select_payload(
                    "Opportunity",
                    &["Id", "Title", "Account"],
                    1,
                    Some(equals_filter("Id", cid, DVT_GUID)),
                ))
                .await?;
            if let Some(o) = rows_of(&od).first() {
                builder = disp(field(o, "Account"));
                job_name = disp(field(o, "Title"));
            }
        }

        let wattage = system
            .as_ref()
            .and_then(|s| s.panel_model.as_deref())
            .and_then(calc::wattage_from_model);
        let options_lot = match &system {
            None => true,
            Some(s) => s.panel_qty.unwrap_or(0) == 0 && s.size_dc.unwrap_or(0.0) <= 0.0,
        };

        Ok(LotBundle {
            lot_id: lot.lot_id.clone(),
            job: lot.job.clone(),
            lot: lot.lot.clone(),
            lot_addr: lot.lot_addr.clone(),
            zip: lot.zip.clone(),
            plan: lot.plan.clone(),
            builder,
            job_name,
            system,
            system_count,
            options_lot,
            inverter_efficiency,
            wattage,
        })
    }

    /// Opportunity free-text system-sizes block (options branch input).
    pub async fn fetch_system_sizes_block(&self, community_id: &str) -> Result<Option<String>> {
        let od = self
            .select(select_payload(
                "Opportunity",
                &["Id", "UsrSystemSizePerPlan"],
                1,
                Some(equals_filter("Id", community_id, DVT_GUID)),
            ))
            .await?;
        Ok(rows_of(&od)
            .first()
            .and_then(|o| disp(field(o, "UsrSystemSizePerPlan"))))
    }

    /// WRITE: set the lot's estimated annual kWh (CrsEstAnnualKwhProductionLot) and
    /// the estimated monthly kWh (MONTHLY_KWH_FIELD) in one UpdateQuery.
    /// Once written the lot leaves the pool. Call ONLY on explicit user confirm.
    pub async fn update_est_kwh(&self, lot_id: &str, kwh: i64, monthly_kwh: i64) -> Result<i64> {
        // Envelope mirrors the two proven writeback clients (crs-n8n-tools-api
        // creatio_writeback.py, inventory_tracker_app pickup_api.py):
        //   - operationType 2 = Update. 1 is INSERT — the server then dereferences
        //     an absent record and 500s with a bare NullReferenceException.
        //   - columnValues items are FLAT ({expressionType, parameter}); the
        //     nested "expression" wrapper is SelectQuery-only.
        //   - primary-column macros filter (macrosType 34), not columnPath "Id".
        let payload = json!({
            "rootSchemaName": "UsrLotRecords",
            "operationType": 2, // Update
            "includeProcessExecutionData": true,
            "columnValues": { "items": {
                "CrsEstAnnualKwhProductionLot": {
                    "expressionType": 2,
                    "parameter": { "dataValueType": DVT_INTEGER, "value": kwh }
                },
                MONTHLY_KWH_FIELD: {
                    "expressionType": 2,
                    "parameter": { "dataValueType": DVT_INTEGER, "value": monthly_kwh }
                }
            }},
            "filters": primary_id_filter(lot_id),
            "isForceUpdate": false,
        });
        let url = format!("{}{}", self.base_url, UPDATE_SERVICE);
        let resp = self
            .client
            .post(&url)
            .header("BPMCSRF", &self.bpmcsrf)
            .json(&payload)
            .send()
            .await
            .context("UpdateQuery failed to send")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("UpdateQuery HTTP {}: {}", status, &body[..body.len().min(1000)]);
        }
        let data: Value = resp.json().await.context("UpdateQuery returned non-JSON")?;
        if data.get("success").and_then(Value::as_bool) == Some(false) {
            bail!("UpdateQuery success=false: {}", truncate(&data.to_string(), 600));
        }
        // Silent-no-op trap: Creatio returns success=true when the filter matched
        // nothing. rowsAffected counts MATCHED rows, so an idempotent re-write still
        // reports >=1 — 0 means the lot wasn't found and must not read as success.
        let rows = data.get("rowsAffected").and_then(Value::as_i64).unwrap_or(0);
        if rows <= 0 {
            bail!("UpdateQuery affected {} rows for lot {} — record not found / filter miss; nothing was written", rows, lot_id);
        }
        Ok(rows)
    }
}

// ── DataService payload + polymorphic-value helpers ─────────────────────────

fn select_payload(root: &str, cols: &[&str], row_count: i64, filter: Option<Value>) -> Value {
    let items: serde_json::Map<String, Value> = cols
        .iter()
        .map(|c| {
            (
                (*c).to_string(),
                json!({ "expression": { "expressionType": 0, "columnPath": c } }),
            )
        })
        .collect();
    let mut q = json!({
        "rootSchemaName": root,
        "operationType": 0,
        "includeProcessExecutionData": false,
        "columns": { "items": items },
        "isDistinct": false,
        "rowCount": row_count,
        "rowsOffset": -1,
        "isPageable": false,
        "allColumns": false,
        "useLocalization": true,
    });
    if let Some(f) = filter {
        q["filters"] = f;
    }
    q
}

/// Paginated SelectQuery ordered by Id (stable paging past the rowCount cap).
fn pool_page_payload(cols: &[&str], row_count: i64, offset: i64) -> Value {
    let mut items = serde_json::Map::new();
    for c in cols {
        let mut expr = json!({ "expression": { "expressionType": 0, "columnPath": c } });
        if *c == "Id" {
            expr["orderDirection"] = json!(1); // ascending — consistent page order
            expr["orderPosition"] = json!(0);
        }
        items.insert((*c).to_string(), expr);
    }
    json!({
        "rootSchemaName": "UsrLotRecords",
        "operationType": 0,
        "includeProcessExecutionData": false,
        "columns": { "items": items },
        "isDistinct": false,
        "rowCount": row_count,
        "rowsOffset": offset,
        "isPageable": true,
        "allColumns": false,
        "useLocalization": true,
    })
}

/// Primary-column (Id) equality filter for writes — macrosType 34 addresses the
/// root schema's primary column directly. This is the shape both proven Python
/// writeback clients use for UpdateQuery.
fn primary_id_filter(record_id: &str) -> Value {
    json!({
        "items": { "primaryColumnFilter": {
            "filterType": 1,
            "comparisonType": 3,
            "isEnabled": true,
            "trimDateTimeParameterToDate": false,
            "leftExpression": { "expressionType": 1, "functionType": 1, "macrosType": 34 },
            "rightExpression": { "expressionType": 2, "parameter": { "dataValueType": DVT_GUID, "value": record_id } },
        }},
        "logicalOperation": 0,
        "isEnabled": true,
        "filterType": 6,
    })
}

/// Single-column equality filter. `dvt` is the dataValueType proven for that column.
fn equals_filter(column: &str, value: &str, dvt: i64) -> Value {
    json!({
        "items": { "f": {
            "filterType": 1,
            "comparisonType": 3,
            "isEnabled": true,
            "leftExpression": { "expressionType": 0, "columnPath": column },
            "rightExpression": { "expressionType": 2, "parameter": { "dataValueType": dvt, "value": value } },
        }},
        "logicalOperation": 0,
        "isEnabled": true,
        "filterType": 6,
    })
}

/// Borrow the `rows` array out of a DataService response (empty slice if absent).
fn rows_of(data: &Value) -> &[Value] {
    data.get("rows").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

fn field<'a>(row: &'a Value, key: &str) -> &'a Value {
    row.get(key).unwrap_or(&Value::Null)
}

/// Display value: lookup `{value,displayValue}` -> displayValue; bare string -> itself.
fn disp(v: &Value) -> Option<String> {
    match v {
        Value::Object(m) => m
            .get("displayValue")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

/// GUID from a lookup value.
fn guid(v: &Value) -> Option<String> {
    match v {
        Value::Object(m) => m
            .get("value")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        _ => None,
    }
}

/// Blank = empty date/string, 0/absent integer, or a lookup with no displayValue.
fn blank(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.is_empty() || s == "0",
        Value::Number(n) => n.as_f64().map_or(true, |f| f == 0.0),
        Value::Object(_) => disp(v).is_none(),
        _ => false,
    }
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64()
}

fn int(v: &Value) -> Option<u32> {
    v.as_u64()
        .map(|n| n as u32)
        .or_else(|| v.as_f64().map(|f| f as u32))
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}
