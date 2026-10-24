Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 564feb857476f3247a0693cb65aa2d44e3322d3a

# Binding a time series to an asset property leaves the asset's lastUpdateDate untouched, unlike every sibling write that changes a property's alias

_Filed: 2026-10-24T22:02:09Z_
_Operation: AssociateTimeSeriesToAssetProperty | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `564feb857476f3247a0693cb65aa2d44e3322d3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AssociateTimeSeriesToAssetProperty` changes a stored asset property — it sets that property's `alias` and merges the disassociated stream's samples into the property's history, via `store.set_property_alias(...)` — but never moves the owning asset's `last_update_date`. The two other operations in this crate that perform exactly the same write DO move it, and both say so in a comment:

- `UpdateAssetProperty` writes `asset.last_update_date = now;` under the comment "The asset carries the property that just changed, so its last-update instant moves with it."
- `DisassociateTimeSeriesFromAssetProperty` writes the same line under the comment "The asset carries the property that just changed, so its last-update instant moves with it — the same write `UpdateAssetProperty` makes when it rebinds or removes a property's alias."

Rebinding a property's alias is precisely what `AssociateTimeSeriesToAssetProperty` does, so by the crate's own stated rule it owes that write and does not make it. `set_property_alias` in `src/state.rs` does not make it either (lines 2435-2517 touch only the property's `alias` member and `property_alias_index`), so there is no path by which the instant moves.

The divergence is directly observable: `DescribeAsset.assetLastUpdateDate`, `DescribeTimeSeries.timeSeriesLastUpdateDate` and `ListTimeSeries`' summaries are all derived from `AssetRecord::last_update_date`. Associate -> disassociate -> associate therefore produces a lastUpdateDate that advances on the disassociate and freezes on the associate, so the reported "last update" is older than the last accepted write to that property.

## Evidence

The handler that omits the write — `src/ops/associate_time_series_to_asset_property.rs:284-289`:

    // The binding, and with it the samples the stream holds: `set_property_alias`
    // moves them onto the property's series, so the history the alias ingested and
    // the history written through the pair are one history from here on.
    store.set_property_alias(&asset_id, &property_id, Some(input.alias.clone()));

    Ok(AssociateTimeSeriesToAssetPropertyOutput)

(no `now` is even computed in this handler, and `asset.last_update_date` never appears in the file.)

The siblings that make it:
- `src/ops/update_asset_property.rs:335,350-352`: `if !store.set_property_alias(&asset_id, &property_id, input.property_alias.clone()) {` ... `// The asset carries the property that just changed, so its last-update` / `// instant moves with it.` / `asset.last_update_date = now;`
- `src/ops/disassociate_time_series_from_asset_property.rs:260-265`: `// The asset carries the property that just changed, so its last-update instant` / `// moves with it — the same write `UpdateAssetProperty` makes when it rebinds or` / `// removes a property's alias.` / `if let Some(asset) = store.assets.get_mut(&asset_id) { asset.last_update_date = now; }`

The shared helper makes no such write — `src/state.rs:2435-2452`: `pub fn set_property_alias(&mut self, asset_id: &str, property_id: &str, alias: Option<String>) -> bool { ... let previous = property.alias.take(); property.alias = alias.clone();` (nothing in the body assigns `last_update_date`).

Readers of the field: `src/ops/describe_asset.rs:212` (`asset_last_update_date: crate::state::to_date_time(record.last_update_date)`), `src/ops/describe_time_series.rs:327` (`time_series_last_update_date: crate::state::to_date_time(asset.last_update_date)`), `src/ops/list_time_series.rs:338`.

Test coverage: `tests/all/time_series.rs:266` asserts `body["timeSeriesLastUpdateDate"] == asset["assetLastUpdateDate"]` (the two stay coupled, so a fix moves both together); `tests/all/time_series_disassociation.rs:323-324` and `tests/all/update_asset.rs:222-224` pin the disassociate/update sides. No test asserts that an association must leave the instant unchanged.

## Steps to Reproduce

1. CreateAssetModel with one DOUBLE measurement property; CreateAsset from it; wait for ACTIVE.
2. `BatchPutAssetPropertyValue` with an entry addressed by `propertyAlias: "/plant/1/temp"` so a DISASSOCIATED time series is implicitly created.
3. `GET /assets/{assetId}` -> note `assetLastUpdateDate` (call it T0).
4. `POST /timeseries/associate` with `{alias:"/plant/1/temp", assetId, propertyId}` -> 200.
5. `GET /assets/{assetId}` again -> `assetLastUpdateDate` is still T0, although the property's alias and its whole sample history just changed.
6. `POST /timeseries/disassociate` with the same triple -> 200, then `GET /assets/{assetId}`: now `assetLastUpdateDate` HAS advanced. The inverse pair of operations disagrees about whether changing a property's alias is an update to the asset.
