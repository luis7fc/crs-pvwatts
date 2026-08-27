//! PVWatts Generation Tool — local browser app for the CRS consultation team.
//!
//!   calc.rs     — wattage-from-model + kW math (tested)
//!   creatio.rs  — Creatio read layer (pool, per-lot bundle) + writeback
//!   sidecar.rs  — sidecar client (PVWatts + parse-assist; the only open-net dep)
//!   pdf.rs      — per-lot audit PDF (NREL results-page clone) -> I: drive
//!   csv.rs      — per-lot CSV of every value on that PDF, written beside it
//!   server.rs   — axum: serves the SPA + JSON API
//!
//! Runs on the coworker's PC: starts a localhost server and opens the browser.
//! The exe talks only to Creatio + the sidecar (both corporate-reachable); the
//! NREL/Anthropic keys stay server-side.
#![allow(dead_code)]

mod calc;
mod config;
mod creatio;
mod csv;
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
    let creatio_base = std::env::var("CREATIO_BASE_URL")
        .unwrap_or_else(|_| "https://citadelrs.creatio.com".to_string());
    let sidecar_base = std::env::var("SIDECAR_BASE_URL")
        .unwrap_or_else(|_| "https://crs-n8n-tools-api.onrender.com".to_string());
    let output_root = std::env::var("PVWATTS_OUTPUT_ROOT")
        .unwrap_or_else(|_| pdf::DEFAULT_ROOT.to_string());

    // Sidecar is optional at startup — if the key isn't set yet, the UI's Settings
    // screen collects it and rebuilds the client. Key comes from the OS credential
    // vault (or legacy/override env, or a build-time baked value).
    let sidecar_key = config::get_sidecar_key()
        .or_else(|| option_env!("N8N_TOOLS_API_KEY").map(str::to_string));
    let sidecar = sidecar_key.and_then(|k| sidecar::Sidecar::new(sidecar_base.clone(), k).ok());
    if sidecar.is_none() {
        eprintln!("sidecar key not set — configure it in the browser Settings screen");
    }

    let state = server::AppState {
        cfg: Arc::new(Mutex::new(server::AppConfig {
            creatio_base_url: creatio_base.clone(),
            sidecar_base_url: sidecar_base,
            output_root,
        })),
        session: Arc::new(Mutex::new(None)),
        sidecar: Arc::new(Mutex::new(sidecar)),
    };
    // Auto-login from saved config + vaulted password so it's one-time per PC.
    if let Ok(u) = std::env::var("CREATIO_USERNAME") {
        if !u.is_empty() {
            if let Some(p) = config::get_creatio_password(&u) {
                let cfg = creatio::CreatioConfig { base_url: creatio_base.clone(), username: u, password: p };
                match creatio::Session::login(&cfg).await {
                    Ok(s) => {
                        *state.session.lock().await = Some(s);
                        println!("auto-logged in from saved credentials");
                    }
                    Err(e) => eprintln!("saved credentials didn't work ({e}) — showing login"),
                }
            }
        }
    }

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
