Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d905081120311aa2c64faab09af576215e2cd502

# An Outpost never acquires a hardware asset, so ListAssets answers an empty list for every Outpost that can exist

_Filed: 2026-10-24T17:28:32Z_
_Operation: ListAssets | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d905081120311aa2c64faab09af576215e2cd502`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListAssets` reports `OutpostRecord::assets`, and no request this emulator serves ever writes that vector. `CreateOutpost` initialises it to `Vec::new()` and no other handler touches it, so over the wire the operation is a canned empty answer: for every Outpost, in every region, with any `SupportedHardwareType`, it replies HTTP 200 with `{"Assets": []}` and no `NextToken`.

Expected (real Outposts): an Outpost whose hardware has been installed reports one `AssetInfo` per rack slot / server — each with `AssetId`, `RackId`, `AssetType` (`COMPUTE` / `STORAGE` / `POWERSHELF` / `SWITCH` / `NETWORKING`), `AssetLocation.RackElevation` and, for compute assets, `ComputeAttributes` (`HostId`, `State`, `InstanceFamilies`, `MaxVcpus`). This is doubly visible here because the emulator *does* move a freshly created Outpost to `LifeCycleStatus: ACTIVE` half a second after `CreateOutpost` — and a real Outpost only reaches an active lifecycle status once hardware is installed, so an ACTIVE Outpost with zero assets is a state the real service never reports.

Actual: the handler's entire filtering (`AssetTypeFilter`, `HostIdFilter`, `StatusFilter`) and paging path is unreachable through the API, and the dangling-asset references elsewhere in the crate follow from the same missing state — `GetOutpostSupportedInstanceTypes?AssetId=…` answers `NotFoundException` for *every* asset id because `record.assets` is always empty.

The fix is to materialise the hardware an Outpost holds from state the service does own (e.g. derive assets for an Outpost once it reaches its ACTIVE lifecycle status, keyed off `SupportedHardwareType` / the Outpost's generation) so that an asset id a caller reads from `ListAssets` can then be used as `StartCapacityTask$AssetId` and `GetOutpostSupportedInstanceTypes$AssetId`.

## Evidence

Spec: `com.amazonaws.outposts#ListAssets` is documented "Lists the hardware assets for the specified Outpost. Use filters to return specific results" and models `Assets` (`AssetListDefinition`) plus three filters (`HostIdFilter`, `StatusFilter`, `AssetTypeFilter`) and `MaxResults`/`NextToken` — an operation whose whole purpose is to report installed hardware. `AssetInfo` models `AssetId`, `RackId`, `AssetType`, `ComputeAttributes`, `AssetLocation`.

Handler/state: `src/ops/list_assets.rs` lines ~197-229 pages `record.assets` only; `src/state.rs` line ~348 declares `pub assets: Vec<AssetInfo>` and `src/ops/create_outpost.rs` line ~295 (`assets: Vec::new()`) is the only initialisation. `grep -n 'assets' services/service-com-amazonaws-outposts-outpostsolafservice/src` finds no other write — the field is read by `list_assets.rs` and by `get_outpost_supported_instance_types.rs` (line ~258) and written nowhere.

Tests: `tests/all/outpost_assets.rs` has to seed assets by poking the in-memory store (`fn record(outposts, outpost_id, assets, case)` at line ~197, used by every case) because no HTTP request can produce one — confirming no API path ever yields a non-empty `Assets`. No test asserts that a created/ACTIVE Outpost reports hardware.

This is the same class of hollow answer already filed for `GetRenewalPricing` (`20261024T071625Z-outposts-getrenewalpricing-wrong-behavior`), on a different operation and a different piece of state.

## Steps to Reproduce

1. CreateSite, then CreateOutpost against that site with SupportedHardwareType=RACK; keep the returned OutpostId.
2. Wait ~1s and GET /outposts/{OutpostId} — the Outpost reports LifeCycleStatus ACTIVE.
3. GET /outposts/{OutpostId}/assets — observe HTTP 200 with {"Assets": []} for an Outpost the service itself calls ACTIVE.
4. Repeat with AssetTypeFilter=COMPUTE, StatusFilter=ACTIVE, HostIdFilter=h-… and any MaxResults — the answer is identical and never carries a NextToken, so no filter or page bound is observable.
5. GET /outposts/{OutpostId}/supportedInstanceTypes?AssetId=1234567890 — 404 NotFoundException for every asset id, because no asset can exist to be named.
