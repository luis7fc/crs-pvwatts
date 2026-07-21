//! Local axum server — serves the SPA and the JSON API that wraps the
//! Creatio read layer + sidecar. Single-user (runs on the coworker's PC): one
//! Creatio session lives in shared state after login.

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

#[derive(Clone)]
pub struct AppState {
    pub base_url: String,
    pub session: Arc<Mutex<Option<creatio::Session>>>,
    pub sidecar: Arc<sidecar::Sidecar>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
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

// ── handlers ────────────────────────────────────────────────────────────────
#[derive(Deserialize)]
struct LoginReq {
    username: String,
    password: String,
}

async fn login(State(st): State<AppState>, Json(req): Json<LoginReq>) -> Api {
    let cfg = creatio::CreatioConfig {
        base_url: st.base_url.clone(),
        username: req.username,
        password: req.password,
    };
    let sess = creatio::Session::login(&cfg).await?;
    *st.session.lock().await = Some(sess);
    Ok(Json(json!({"ok": true})))
}

async fn logout(State(st): State<AppState>) -> Api {
    if let Some(s) = st.session.lock().await.take() {
        s.logout().await;
    }
    Ok(Json(json!({"ok": true})))
}

async fn pool(State(st): State<AppState>) -> Api {
    let guard = st.session.lock().await;
    let sess = guard.as_ref().ok_or_else(|| anyhow::anyhow!("not logged in"))?;
    let lots = sess.fetch_pool(1500).await?;
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
        // guard dropped here — don't hold the Creatio lock across the sidecar call
    };
    match block {
        Some(b) => {
            let parsed = st.sidecar.parse_system_sizes(&req.plan_code, &b).await?;
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
    let pv = st.sidecar.run_pvwatts(&req.zip, req.inv_eff, &req.arrays).await?;
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
    // 1) always write the audit PDF
    let root = pdf::output_root();
    let path = pdf::write_audit_pdf(
        &root,
        &req.bundle,
        &req.arrays,
        &req.pv,
        req.is_candidate,
        req.variant_label.as_deref(),
    )?;

    // 2) WRITE to Creatio only for the user-confirmed candidate variant
    let mut wrote = false;
    let mut rows = 0i64;
    if req.is_candidate {
        let guard = st.session.lock().await;
        let sess = guard.as_ref().ok_or_else(|| anyhow::anyhow!("not logged in"))?;
        rows = sess.update_est_kwh(&req.bundle.lot_id, req.pv.lot_total_kwh).await?;
        wrote = true;
    }

    Ok(Json(json!({
        "ok": true,
        "pdf_path": path.display().to_string(),
        "wrote_creatio": wrote,
        "rows": rows,
    })))
}
