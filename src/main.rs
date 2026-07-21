//! PVWatts Generation Tool — local browser app for the CRS consultation team.
//!
//!   calc.rs     — wattage-from-model + kW math (tested)
//!   creatio.rs  — Creatio read layer (pool, per-lot bundle) + writeback
//!   sidecar.rs  — sidecar client (PVWatts + parse-assist; the only open-net dep)
//!   pdf.rs      — per-lot audit PDF -> I: drive
//!   server.rs   — axum: serves the SPA + JSON API
//!
//! Runs on the coworker's PC: starts a localhost server and opens the browser.
//! The exe talks only to Creatio + the sidecar (both corporate-reachable); the
//! NREL/Anthropic keys stay server-side.
#![allow(dead_code)]

mod calc;
mod creatio;
mod pdf;
mod server;
mod sidecar;

use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Mutex;

/// Load config from a `pvwatts.env` file (KEY=VALUE) next to the exe or in cwd,
/// so a handed-off .exe works without the coworker setting environment vars.
/// Ship `pvwatts.exe` + `pvwatts.env` (holds N8N_TOOLS_API_KEY, optional
/// CREATIO_BASE_URL / SIDECAR_BASE_URL). Real env vars take precedence.
fn load_local_env() {
    let mut candidates = vec![std::path::PathBuf::from("pvwatts.env")];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("pvwatts.env"));
        }
    }
    for path in candidates {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                let (k, v) = (k.trim(), v.trim().trim_matches('"'));
                if std::env::var(k).is_err() {
                    std::env::set_var(k, v);
                }
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    load_local_env();
    let base_url = std::env::var("CREATIO_BASE_URL")
        .unwrap_or_else(|_| "https://citadelrs.creatio.com".to_string());
    let sidecar = Arc::new(sidecar::Sidecar::from_env()?);

    let state = server::AppState {
        base_url,
        session: Arc::new(Mutex::new(None)),
        sidecar,
    };
    let app = server::router(state);

    let port: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8787);
    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let url = format!("http://{addr}");

    println!("PVWatts tool running at {url}  (Ctrl-C to stop)");
    let _ = open::that(&url); // best-effort auto-open

    axum::serve(listener, app).await?;
    Ok(())
}
