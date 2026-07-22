//! Local axum server — serves the SPA and the JSON API that wraps the Creatio
//! read layer + sidecar. Single-user (runs on the coworker's PC). Config
//! (sidecar key/URL, Creatio URL, PDF root) is runtime-settable via /api/settings
//! and persisted to pvwatts.env, so setup is done in the browser, not a file.

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::{creatio, pdf, sidecar};

pub struct AppConfig {
    pub creatio_base_url: String,
    pub sidecar_base_url: String,
    pub output_root: String,
}

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Mutex<AppConfig>>,
    pub session: Arc<Mutex<Option<creatio::Session>>>,
    pub sidecar: Arc<Mutex<Option<sidecar::Sidecar>>>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/status", get(status))
        .route("/api/settings", get(get_settings).post(post_settings))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/pool", get(pool))
        .route("/api/bundle", post(bundle))
        .route("/api/options", post(options))
        .route("/api/run", post(run))
        .route("/api/commit", post(commit))
        .with_state(state)
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../assets/index.html"))
}

// ── error boundary ──────────────────────────────────────────────────────────
struct AppError(anyhow::Error);
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok": false, "error": self.0.to_string()}))).into_response()
    }
}
impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(e: E) -> Self {
        Self(e.into())
    }
}
type Api = Result<Json<Value>, AppError>;

// ── status + settings ───────────────────────────────────────────────────────
async fn status(State(st): State<AppState>) -> Json<Value> {
    let logged_in = st.session.lock().await.is_some();
    let configured = st.sidecar.lock().await.is_some();
    let saved_user = std::env::var("CREATIO_USERNAME").ok().filter(|s| !s.is_empty());
    Json(json!({"ok": true, "logged_in": logged_in, "configured": configured, "saved_user": saved_user}))
}

async fn get_settings(State(st): State<AppState>) -> Json<Value> {
    let c = st.cfg.lock().await;
    let key_set = st.sidecar.lock().await.is_some();
    Json(json!({
        "ok": true, "configured": key_set, "key_set": key_set,
        "sidecar_url": c.sidecar_base_url, "creatio_base_url": c.creatio_base_url, "output_root": c.output_root,
    }))
}

#[derive(Deserialize)]
struct SettingsReq {
    sidecar_url: Option<String>,
    sidecar_key: Option<String>,
    creatio_base_url: Option<String>,
    output_root: Option<String>,
}

async fn post_settings(State(st): State<AppState>, Json(req): Json<SettingsReq>) -> Api {
    let mut kv: Vec<(String, String)> = Vec::new();
    let sidecar_base;
    {
        let mut c = st.cfg.lock().await;
        if let Some(v) = req.sidecar_url.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            c.sidecar_base_url = v.trim_end_matches('/').to_string();
        }
        if let Some(v) = req.creatio_base_url.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            c.creatio_base_url = v.trim_end_matches('/').to_string();
        }
        if let Some(v) = req.output_root.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            c.output_root = v.to_string();
            std::env::set_var("PVWATTS_OUTPUT_ROOT", v);
        }
        kv.push(("SIDECAR_BASE_URL".into(), c.sidecar_base_url.clone()));
        kv.push(("CREATIO_BASE_URL".into(), c.creatio_base_url.clone()));
        kv.push(("PVWATTS_OUTPUT_ROOT".into(), c.output_root.clone()));
        sidecar_base = c.sidecar_base_url.clone();
    }
    if let Some(k) = req.sidecar_key.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        crate::config::save_sidecar_key(k)?; // -> OS credential vault (not the file)
        std::env::set_var("N8N_TOOLS_API_KEY", k); // for the immediate rebuild this session
    }
    crate::config::save_kv(&kv.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect::<Vec<_>>())?;

    // Rebuild the sidecar client with the current URL + key.
    let key = crate::config::get_sidecar_key()
        .or_else(|| option_env!("N8N_TOOLS_API_KEY").map(str::to_string));
    if let Some(k) = key {
        *st.sidecar.lock().await = Some(sidecar::Sidecar::new(sidecar_base, k)?);
    }
    Ok(Json(json!({"ok": true, "configured": st.sidecar.lock().await.is_some()})))
}

// ── auth ────────────────────────────────────────────────────────────────────
#[derive(Deserialize)]
struct LoginReq {
    username: String,
    password: String,
    #[serde(default)]
    remember: Option<bool>,
}

async fn login(State(st): State<AppState>, Json(req): Json<LoginReq>) -> Api {
    let base_url = st.cfg.lock().await.creatio_base_url.clone();
    let cfg = creatio::CreatioConfig { base_url, username: req.username.clone(), password: req.password.clone() };
    let sess = creatio::Session::login(&cfg).await?;
    *st.session.lock().await = Some(sess);
    if req.remember != Some(false) {
        let _ = crate::config::save_creatio_creds(&req.username, &req.password);
    }
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
struct LogoutReq {
    #[serde(default)]
    forget: bool,
}

async fn logout(State(st): State<AppState>, Json(req): Json<LogoutReq>) -> Api {
    if let Some(s) = st.session.lock().await.take() {
        s.logout().await;
    }
    if req.forget {
        let _ = crate::config::forget_creatio_creds();
    }
    Ok(Json(json!({"ok": true})))
}

// ── data ────────────────────────────────────────────────────────────────────
async fn pool(State(st): State<AppState>) -> Api {
    let guard = st.session.lock().await;
    let sess = guard.as_ref().ok_or_else(|| anyhow::anyhow!("not logged in"))?;
    let lots = sess.fetch_pool().await?;
    Ok(Json(json!({"ok": true, "lots": lots})))
}

async fn bundle(State(st): State<AppState>, Json(lot): Json<creatio::PoolLot>) -> Api {
    let guard = st.session.lock().await;
    let sess = guard.as_ref().ok_or_else(|| anyhow::anyhow!("not logged in"))?;
    let b = sess.fetch_lot_bundle(&lot).await?;
    Ok(Json(json!({"ok": true, "bundle": b})))
}

#[derive(Deserialize)]
struct OptReq {
    community_id: Option<String>,
    plan_code: String,
}

async fn options(State(st): State<AppState>, Json(req): Json<OptReq>) -> Api {
    let community_id = req.community_id.ok_or_else(|| anyhow::anyhow!("lot has no linked opportunity"))?;
    let block = {
        let guard = st.session.lock().await;
        let sess = guard.as_ref().ok_or_else(|| anyhow::anyhow!("not logged in"))?;
        sess.fetch_system_sizes_block(&community_id).await?
    };
    match block {
        Some(b) => {
            let sidecar = st.sidecar.lock().await.clone()
                .ok_or_else(|| anyhow::anyhow!("sidecar not configured — open Settings"))?;
            let parsed = sidecar.parse_system_sizes(&req.plan_code, &b).await?;
            Ok(Json(json!({"ok": true, "block": b, "parsed": parsed})))
        }
        None => Ok(Json(json!({"ok": true, "block": null, "parsed": null}))),
    }
}

#[derive(Deserialize)]
struct RunReq {
    zip: String,
    inv_eff: f64,
    arrays: Vec<sidecar::ArrayInput>,
}

async fn run(State(st): State<AppState>, Json(req): Json<RunReq>) -> Api {
    let sidecar = st.sidecar.lock().await.clone()
        .ok_or_else(|| anyhow::anyhow!("sidecar not configured — open Settings"))?;
    let pv = sidecar.run_pvwatts(&req.zip, req.inv_eff, &req.arrays).await?;
    Ok(Json(json!({"ok": true, "pv": pv})))
}

#[derive(Deserialize)]
struct CommitReq {
    bundle: creatio::LotBundle,
    arrays: Vec<sidecar::ArrayInput>,
    pv: sidecar::PvResult,
    is_candidate: bool,
    variant_label: Option<String>,
}

async fn commit(State(st): State<AppState>, Json(req): Json<CommitReq>) -> Api {
    let root = pdf::output_root();
    let path = pdf::write_audit_pdf(&root, &req.bundle, &req.arrays, &req.pv, req.is_candidate, req.variant_label.as_deref())?;

    let mut wrote = false;
    let mut rows = 0i64;
    if req.is_candidate {
        let guard = st.session.lock().await;
        let sess = guard.as_ref().ok_or_else(|| anyhow::anyhow!("not logged in"))?;
        rows = sess.update_est_kwh(&req.bundle.lot_id, req.pv.lot_total_kwh).await?;
        wrote = true;
    }
    Ok(Json(json!({"ok": true, "pdf_path": path.display().to_string(), "wrote_creatio": wrote, "rows": rows})))
}
