# PVWatts Generation Tool

Local browser app for the CRS consultation team. Pulls lots from Creatio, lets
the team split each lot's panels into arrays (tilt/azimuth), runs NREL PVWatts,
writes a per-lot audit PDF to the I: drive, and (on confirm) writes the estimated
annual kWh back to Creatio.

## Architecture

The `.exe` runs on a coworker's PC and talks to **only two hosts**, both
reachable from the corporate network:

- **Creatio** (`citadelrs.creatio.com`) — read lot/system data, write the estimate
- **Sidecar** (`crs-n8n-tools-api.onrender.com`) — runs PVWatts and the
  options-block parse server-side, so the NREL + Anthropic keys never live on a
  coworker machine and the `.exe` stays tiny.

```
exe  ──HTTP──> Creatio      (login, pool, per-lot bundle, writeback)
     ──HTTP──> Sidecar ──> NREL PVWatts  (/run/pvwatts)
                       └─> Claude parse   (/run/parse_system_sizes, options lots)
```

## Data flow

1. Sign in with your own Creatio credentials (nothing stored).
2. Pool = lots where `UsrChannel=Integrated`, buyer-info received, not yet
   consulted, no estimate yet.
3. Pick a lot → normal branch (committed system → split its panels into arrays)
   or options branch (empty size → pick a size from the parsed Opportunity block).
4. Run PVWatts per array → summed lot total.
5. Save: writes the audit PDF; if "Creatio Candidate" is checked (with confirm),
   writes the total to `CrsEstAnnualKwhProductionLot` (the lot then leaves the pool).

## Run (dev)

```bash
# config (or use a pvwatts.env file — see below)
export CREATIO_BASE_URL=https://citadelrs.creatio.com
export N8N_TOOLS_API_KEY=...          # sidecar key
export SIDECAR_BASE_URL=https://crs-n8n-tools-api.onrender.com   # optional (default)
export PVWATTS_OUTPUT_ROOT="I:\Solar\1- New Construction\Production"  # PDF root; local dir for dev
cargo run
# opens http://127.0.0.1:8787
```

## Hand-off config (`pvwatts.env`)

Ship `pvwatts.exe` next to a `pvwatts.env` file so coworkers don't set env vars:

```
N8N_TOOLS_API_KEY=...
CREATIO_BASE_URL=https://citadelrs.creatio.com
SIDECAR_BASE_URL=https://crs-n8n-tools-api.onrender.com
PVWATTS_OUTPUT_ROOT=I:\Solar\1- New Construction\Production
```

`pvwatts.env` is git-ignored (it holds the sidecar key). Real env vars override it.

## Windows build

- **CI (recommended):** push to GitHub → the `build-windows` Action builds
  `target/release/pvwatts_tool.exe` natively on `windows-latest`; download it from
  the run's Artifacts.
- **Local cross-compile (Mac/Linux):** needs `mingw-w64`.
  ```bash
  brew install mingw-w64            # macOS
  rustup target add x86_64-pc-windows-gnu
  cargo build --release --target x86_64-pc-windows-gnu
  ```
  (reqwest uses rustls, not OpenSSL, so there is no C TLS dependency to cross-build.)

## Layout

```
src/
  main.rs      start server + open browser; load pvwatts.env
  server.rs    axum: SPA + JSON API
  creatio.rs   Creatio read layer (pool, bundle) + writeback (UpdateQuery)
  sidecar.rs   sidecar client (PVWatts + parse-assist)
  calc.rs      wattage-from-model parser + kW math (tested)
  pdf.rs       per-lot audit PDF -> I: drive
assets/index.html   single-page UI
```
