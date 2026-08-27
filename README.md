# crs-pvwatts — PVWatts Generation Tool

Local browser app for the CRS consultation team. Reads the pending-lot pool from
Creatio, lets the team split each lot's panels into arrays (tilt/azimuth), runs
NREL PVWatts, writes a per-lot audit PDF + CSV to the I: drive, and (on confirm)
writes the estimated annual kWh back to Creatio.

The PDF is a **clone of the NREL PVWatts results page** — same grid, same tables,
same figures — one two-page block per modelled array (PVWatts models one array per
run), followed by a CRS lot-summary page. The CSV beside it carries every value on
the PDF: one row per array plus a `LOT_TOTAL` row.

It's a single self-contained Windows `.exe` — no installer, no DLLs (the web UI
is embedded; TLS uses Windows' built-in SChannel). Double-click → it opens the
browser at `http://127.0.0.1:8787`.

---

## Update / release workflow (read me)

The source lives here on GitHub. To ship an update, we **git pull on the Windows
VM and rebuild** — no zipping source over Teams.

```
Luis (Mac)                 Windows VM (build agent)              Coworker PCs
──────────                 ────────────────────────              ────────────
edit + commit              git pull                              (get the .exe
git push        ───────►   cargo build --release   ───────►      via Teams, run it)
                           → target\release\pvwatts_tool.exe
```

1. **Luis (Mac):** make changes, `git commit`, `git push`.
2. **Windows VM agent:** `git pull` → `cargo build --release` → the fresh exe is
   at `target\release\pvwatts_tool.exe`.
3. **Distribute:** share that exe on Teams. Teams blocks a bare `.exe`, so
   right-click → *Send to → Compressed (zipped) folder* and share the zip;
   recipient extracts, then runs (first launch: *"unknown publisher" → More info →
   Run anyway*, since it's unsigned).
4. Coworkers **keep their `pvwatts.env`** (saved login + settings) — only the exe
   is replaced.

---

## Build (Windows VM agent)

Prereqs on the VM (one time):
- **Rust** — install via `rustup` (https://rustup.rs).
- A **C++ linker** — the MSVC toolchain needs "Visual Studio C++ Build Tools"
  (rustup will say if it's missing). Alternatively use the GNU toolchain
  (`rustup default stable-x86_64-pc-windows-gnu`).

Then, every update:
```powershell
git pull
cargo build --release
# → target\release\pvwatts_tool.exe   (single self-contained file)
```
Nothing else is bundled — `assets/index.html` is compiled into the binary and TLS
is SChannel (no OpenSSL/NASM/cmake). No GitHub Actions needed.

---

## First-time setup on a coworker PC (Luis, remoting in)

1. Run the exe → browser opens.
2. **⚙ Settings** → paste the **sidecar key** (`N8N_TOOLS_API_KEY`). Save. (URLs
   and the I: PDF path have defaults; change only if needed.)
3. Sign in once with **that user's** Creatio credentials, "Remember on this PC" on.

After that the user just double-clicks the exe → it auto-logs in → pending pool.

---

## Using it

1. **Pool** = lots where `UsrChannel = Integrated`, buyer-info received, not yet
   consulted, and no estimate yet (the full set, paged from Creatio — ~10s load).
2. **Search** box filters the pool by job # / lot # / community (partial).
3. Pick a lot →
   - **Normal:** committed system → split its panels into arrays (tilt/azimuth).
   - **Options:** empty size → pick a size from the parsed Opportunity block.
4. **Run PVWatts** per array → summed lot total.
5. **Save:** writes the audit PDF **and the matching CSV** (same folder, same
   basename); if *Creatio Candidate* is checked (with a confirm), writes the total
   to `CrsEstAnnualKwhProductionLot` and the lot leaves the pool.

> **Why the kWh differs from a PVWatts web run.** CRS applies 3%/month soiling per
> the 7/20/2026 input rules; the public site defaults to none. Same system, the web
> reads ~3% higher. The page shows the 3% in its "Monthly Irradiance Loss" row and
> names it in the headline footnote, so the gap is on the page rather than hidden.
> The site's "system output may range from X to Y" band is **not** in the PVWatts
> JSON API, so the clone omits it instead of approximating it.

---

## Architecture

The exe talks to **only two hosts**, both reachable from the corporate network:

- **Creatio** (`citadelrs.creatio.com`) — read pool/system, write the estimate.
- **Sidecar** (`crs-n8n-tools-api.onrender.com`) — runs PVWatts (NREL) and the
  options-block parse (Claude) **server-side**, so the NREL/Anthropic keys never
  live on a coworker machine.

```
exe ──HTTP──► Creatio      (login, pool, per-lot bundle, writeback)
    ──HTTP──► Sidecar ──► NREL PVWatts   (/run/pvwatts)
                      └─► Claude parse    (/run/parse_system_sizes)
```

---

## Config

The exe reads config from environment variables, or from a `pvwatts.env` file next
to it (the Settings screen writes this for you). Real env vars override the file.

| Var | What | Default |
|---|---|---|
| `N8N_TOOLS_API_KEY` | sidecar key (required) | — |
| `SIDECAR_BASE_URL` | sidecar URL | `https://crs-n8n-tools-api.onrender.com` |
| `CREATIO_BASE_URL` | Creatio tenant | `https://citadelrs.creatio.com` |
| `PVWATTS_OUTPUT_ROOT` | PDF + CSV root (mapped I:) | `I:\Solar\1- New Construction\Production` |
| `CREATIO_USERNAME` / `_PASSWORD` | saved by "Remember" | — |
| `PORT` | localhost port | `8787` |

`pvwatts.env` is git-ignored (it holds secrets + a plaintext password from
"Remember"). The **sidecar's** `NREL_API_KEY` lives on Render, not here.

### Run for dev (Mac/Windows)
```bash
N8N_TOOLS_API_KEY=... PVWATTS_OUTPUT_ROOT=/tmp/pv cargo run   # opens :8787
```

---

## Layout

```
src/
  main.rs      start server + open browser; load pvwatts.env; auto-login
  server.rs    axum: SPA + JSON API (+ /api/settings, /api/status)
  creatio.rs   Creatio read layer (paged pool, per-lot bundle) + writeback
  sidecar.rs   sidecar client (PVWatts + parse-assist)
  calc.rs      wattage-from-model parser + kW math (tested)
  pdf.rs       per-lot audit PDF (NREL results-page clone) → I: drive
  csv.rs       per-lot CSV of every value on that PDF, written beside it
  config.rs    pvwatts.env read/write (remember creds, settings)
assets/index.html   single-page UI (login, settings, pool, search, run, commit)
```

### Eyeballing the PDF after a layout change
`pdf.rs` carries an ignored test that renders the Castle & Cooke Highgate 65 Lot 3
figures (the reference NREL print) so the output can be diffed against the real
page. `PVWATTS_SAMPLE_ARRAYS` fans it out to check the multi-array structure.

```bash
PVWATTS_SAMPLE_OUT=/tmp/sample cargo test --offline -- --ignored render_reference_sample
```

Every coordinate in `pdf.rs` is in PDF points measured off that print, top-left
origin, so they stay comparable to a fresh measurement off another NREL page.
