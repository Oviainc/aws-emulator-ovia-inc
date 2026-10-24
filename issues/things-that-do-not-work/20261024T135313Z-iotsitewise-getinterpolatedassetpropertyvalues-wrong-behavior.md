Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 30fa86cd244b801d1c6064a0a321e2bf32f37967

# propertyAlias resolves only to associated properties, so a disassociated data stream is answered 404

_Filed: 2026-10-24T13:53:13Z_
_Operation: GetInterpolatedAssetPropertyValues | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `30fa86cd244b801d1c6064a0a321e2bf32f37967`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a `propertyAlias` names ONE data stream, whether or not an asset property is currently associated with it. A stream created by alias-addressed ingestion, or left behind by `DisassociateTimeSeriesFromAssetProperty`, still holds its samples and is addressable by its alias — exactly as `GetAssetPropertyValue`, `GetAssetPropertyValueHistory`, `GetAssetPropertyAggregates`, `BatchGetAssetPropertyValue` and `BatchGetAssetPropertyAggregates` all treat it in this same crate. Interpolating over such a stream must return interpolated values, not a missing-resource refusal.

Actual: this handler resolves an alias only through `store.property_alias_index` — the index of aliases an asset property carries — so an alias that names a disassociated stream resolves to nothing and the call answers 404 ResourceNotFoundException ("The property alias ... could not be found") even though the stream exists, is reported by DescribeTimeSeries/ListTimeSeries, and holds samples. The operation is the only read in the service that does not go through the shared `resolve_alias_series` resolver, so one stream is addressable by five reads and invisible to the sixth.

## Evidence

Spec: `GetInterpolatedAssetPropertyValuesRequest.propertyAlias` is documented as the data-stream alias ("To identify an asset property, you must specify one of the following: ... A propertyAlias"), the same member and wording the other property reads carry; `DeleteTimeSeries`/`DescribeTimeSeries` document a disassociated time series as identified by its alias alone, so an alias that names one names a real resource.

Handler: `src/ops/get_interpolated_asset_property_values.rs:231-235` uses `store.property_alias_index.get(alias)` and 404s on a miss, versus `src/ops/get_asset_property_value_history.rs:253-255`, `get_asset_property_value.rs:205-207`, `get_asset_property_aggregates.rs:336-338`, `batch_get_asset_property_value.rs:334-339` and `batch_get_asset_property_aggregates.rs:420-425`, which all use `store.resolve_alias_series(alias)` and reach the disassociated record. The same handler then reads samples only from `store.property_values[asset_id][property_id]` (lines 347-350), so a disassociated stream's samples are unreachable even if the lookup succeeded.

Tests: `tests/all/interpolated_property_values.rs:937-946` only covers an alias that resolves to NOTHING; no test addresses an existing disassociated stream by alias, so no test asserts the current refusal.

## Steps to Reproduce

1. BatchPutAssetPropertyValue with an entry addressed by `propertyAlias=/company/windfarm/3/turbine/7/temperature` carrying several DOUBLE TQVs a few seconds apart -> 200; this creates a disassociated time series.
2. DescribeTimeSeries?alias=/company/windfarm/3/turbine/7/temperature -> 200, the stream exists with dataType DOUBLE.
3. GetAssetPropertyValue?propertyAlias=<same alias> -> 200 with the latest value (so the stream is readable).
4. GetInterpolatedAssetPropertyValues?propertyAlias=<same alias>&type=LINEAR_INTERPOLATION&intervalInSeconds=10&quality=GOOD&startTimeInSeconds=..&endTimeInSeconds=.. -> 404 ResourceNotFoundException, where AWS interpolates over the stream's samples.
