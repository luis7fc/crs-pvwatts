# PVWatts Tool — Locked Creatio Schema & Data Flow

Tenant: `https://citadelrs.creatio.com` · Forms auth (`AuthService.svc/Login` → BPMCSRF).
All confirmed by read-only probe 2026-07-20. Session hygiene: one login → work → logout.

## 1. The lot pool (what shows in the UI to pick from)
Query `UsrLotRecords` WHERE:
- `UsrChannel` **= Integrated**  (lookup GUID `1ced1e2c-c6ba-4295-9587-e453ee88216d`, dataValueType 10)
- `UsrBuyerInfoReceived`  **IS NOT blank**   (buyer info received)
- `SMConsultationComplete` **IS blank**       (not yet consulted)
- `CrsEstAnnualKwhProductionLot` **IS blank/0** (no estimate yet — this is our writeback field)

Validated: 150/2000 have buyer-info; ~85/400 lots are channel=Integrated; full predicate → a real awaiting-consultation set.

## 1b. "Options" branch — when the lot's committed system size is EMPTY/0
Empty size ≠ un-runnable; it means the buyer has plan **options** (possibly several sizes).
- Lot plan code = `UsrLotRecords.UsrLot_PlanElevation` (e.g. `3526B`, `1336-A`, `2A`, `N317`, `3/A` — **messy formats**).
- Opportunity holds the block in **`UsrSystemSizePerPlan`** (confirmed; DOM data-item-marker "System Sizes").
- ⚠ **Block is inconsistent free-text — DO NOT auto-match.** Real variants observed:
  `Plan 1526 – 5.28kW / 12 panels` · `N317 - 4.40kW` (letter is part of plan) · `All plans - 4.40kW`
  (wildcard) · `Plan 5036/1336 – 4.86kW standard with 6.48kW upgrade` (dual code, 2 sizes, typos) ·
  `Plan 1 (7377 Goldenrod) - 4.455kW` (number in parens) · comma-separated multi-plan lines ·
  kW-only lines with no panel count.
- **Design = assisted-manual:** show raw block + best-effort parsed candidate sizes (kW, panels),
  highlight the lot plan code, **user confirms** which size(s) apply. Never commit a match silently.
- Block gives **kW directly** → feeds PVWatts `system_capacity` (no panel×wattage in this branch).
- Multiple selected sizes → one PVWatts run per size. Fires ONLY when committed size is empty/0.

## 2. Per-lot inputs
| Need | Source | Notes |
|---|---|---|
| Zip (PVWatts location) | `UsrLotRecords.UsrZipCode` | **may be empty → FLAG lot, skip run** |
| Panel model → wattage | `SMSystemDetailObject.SMSolarPanelModule` | parse last int in 250–800 band (validated) |
| Panel qty (LOT TOTAL) | `SMSystemDetailObject.SMSolarPanelQty` | team splits across arrays in UI |
| System kW DC (reconcile) | `SMSystemDetailObject.SMSystemSizeDC` | must ≈ Σ(array kW) |
| Inverter model | `SMSystemDetailObject.SMInverter` (lookup GUID) | → Product |
| Inverter efficiency | `Product.SMInverterEfficiency` (join by GUID) | Enphase→97, Tesla→97.5 |
| Link lot→system | `SMSystemDetailObject.SMUsrLotRecords` = lot `Id` | 1 system row per lot |

`SMSystemDetailObject` holds ONE system per lot → multi-array is a UI construct, not Creatio data.

## 3. Save path (per-lot PDF, human-readable, PVWatts + Creatio inputs/response, no creds)
```
I:\Solar\1- New Construction\Production\{builder}\{job_name}\Consultations\{lot_addr}\{PV_WATTS_lot_addr}.pdf
```
- `{builder}`   = `Opportunity.Account`  (e.g. "Bonadelle Homes")  ← NOT CrsBuilderSPEC (that's a bool)
- `{job_name}`  = `Opportunity.Title`    (e.g. "Mission Oaks")
- `{lot_addr}`  = `UsrLotRecords.UsrLotNumberPlusAddress` (e.g. "33 - 3196 Scoon Place")
- Opportunity reached via `UsrLotRecords.UsrLot_Community` (lookup GUID) → `Opportunity`
- Create `builder` / `job_name` / `lot_addr` folders if absent.

## 4. Per-array PVWatts call (defaults from "PV Watts Input Requirements 7/20/2026")
One request per array; sum `ac_annual` → lot total.
`system_capacity = array_panel_count × wattage ÷ 1000` · `module_type=1` (Premium) ·
`array_type=1` (fixed roof) · `losses=14.1` · `dc_ac_ratio=1.2` · `gcr=0.4` ·
`soiling=3×12` · `inv_eff` from Product · `tilt`/`azimuth` user-entered · `address`=zip.

## 5. Writeback (WRITE — handle with care, confirm before firing)
After run: `UpdateQuery` `UsrLotRecords.CrsEstAnnualKwhProductionLot` = Σ annual kWh.
Once written, the lot leaves the pool. Consultation-complete stays a human step.

## Still open (not schema)
- NREL/PVWatts API key + confirm PVWatts accepts bare zip in `address` (else geocode → lat/lon).
- Confirm coworker PCs reach Creatio without VPN.
- Login menu: each user enters their own Creatio creds at runtime.
