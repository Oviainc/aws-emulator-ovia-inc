Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d204f113841562be393bb0d029d4ee5d3cf48852

# DeleteAsset destroys its properties' time series without the data-segment dependency check DeleteTimeSeries refuses on — dataset segments are left naming a stream the store no longer holds

_Filed: 2026-10-28T23:16:57Z_
_Operation: DeleteAsset | Class: validation_

> _Note: this issue was identified against the code base at commit `d204f113841562be393bb0d029d4ee5d3cf48852`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An asset property's data stream is addressable as a time series whose `TimeSeriesId` is the `(assetId, propertyId)` pair (`state.rs:4825-4841`, `series_by_time_series_id`), and a dataset data segment names exactly such a `timeSeriesId` (BatchAssociateDataSegmentsToDataset resolves an entry through `store.series_by_time_series_id(&entry.time_series_id)`). This crate treats a held data segment as a dependency that BLOCKS removal of the stream it covers: DeleteTimeSeries refuses the delete while any dataset holds a segment of the series, quoting the model sentence for it, and the state module's own index doc says a segment left behind "would name a stream the store no longer holds".

DeleteAsset evaluates only the HIERARCHY dependency (children, and being a child) and then calls `store.forget_asset(&id)`, which does `self.property_values.remove(asset_id)` — dropping every property series of that asset, samples and all, with no data-segment question asked. After the delete, `dataset_data_segments` still holds segments whose `time_series_id` is `<assetId>|<propertyId>`; `series_by_time_series_id` now resolves them to nothing, so ListDatasetDataSegments / ListDatasetDataSegmentRelationships keep reporting segments covering a stream no read can reach — precisely the state DeleteTimeSeries exists to prevent. DeleteDataset's own comment asserts the opposite of what the code does, claiming DeleteAsset already applies this rule ("the same rule, and the same mapping, as `DeleteAsset` and `DeleteWorkspace` give a delete whose resource is still depended on").

The same hole exists one step away, in the property-removal path UpdateAssetModel drives: `propagate_asset_model_definitions` removes the series and alias of every property the new model definition drops (`state.rs:5734-5752`) without consulting the per-series segment index either. Fixing the shared question in one place (the existing `data_segment_of_time_series` helper) closes both.

## Evidence

The sibling that refuses this exact removal, quoting the model:
  src/ops/delete_time_series.rs:338-347 —
    "// \"You can't delete a time series until all of its data segments have been
     // deleted from session datasets.\" A data segment names the time series whose
     // data it covers, so a segment any dataset still holds for this stream depends
     // on it: removing the stream would leave that segment covering data nothing
     // holds and naming a stream no read can resolve. Checked BEFORE anything is
     // removed [...]
     if let Some(holder) = segment_holder(&store, &deleted_time_series_id) {
         return Err(segments_remain(&deleted_time_series_id, &holder));
     }"
  and the helper it asks it through, src/ops/delete_time_series.rs:389-399 —
    "/// The dataset still holding a data segment of `time_series_id`, if any holds one:
     /// the dependency that stops the delete, read through the one per-series index the
     /// state exposes [...]
     store.data_segment_of_time_series(time_series_id)"

The state module's statement of the invariant, src/state.rs:4653-4666 —
    "/// A data segment of `time_series_id` any dataset of this store holds,
     /// paired with the dataset holding it, or `None` when none does.
     /// The dependency `DeleteTimeSeries` refuses to break [...] EVERY segment naming the series counts,
     /// whichever dataset holds it and whichever dataset its data came from [...] so either one left
     /// behind would name a stream the store no longer holds."

The handler that skips it — only hierarchies are checked:
  src/ops/delete_asset.rs:210-243 —
    "// A destructive call cannot go through while another resource still depends on
     // the one it removes, and an asset in a hierarchy is depended on from BOTH sides [...]
     if store.has_hierarchy_children(&id) { ... }
     if let Some((parent, hierarchy)) = store.hierarchy_parent(&id) { ... }"
  then src/ops/delete_asset.rs:245-247 —
    "// The asset and everything keyed off it (its property series, its aliases, its
     // hierarchy associations) go away [...]
     store.forget_asset(&id);"
  which drops the streams, src/state.rs:5373-5380 —
    "pub fn forget_asset(&mut self, asset_id: &str) -> Option<AssetRecord> {
         let record = self.assets.remove(asset_id)?;
         [...]
         self.property_values.remove(asset_id);"
  (no call to `data_segment_of_time_series` anywhere in delete_asset.rs).

The crate asserting that DeleteAsset DOES apply this class of rule:
  src/ops/delete_dataset.rs:228-231 — "it is `InvalidRequestException` (400) and not the retryable
  `ConflictingOperationException` (409) [...] the same rule, and the same mapping,
  as `DeleteAsset` and `DeleteWorkspace` give a delete whose resource is still
  depended on."

A segment can name an asset-property stream, so the case is reachable:
  src/ops/batch_associate_data_segments_to_dataset.rs:373-379 — "// A segment is a slice of ONE time series' data, so an entry naming a
  // time series no stored data stream carries selects nothing [...]
  if store.series_by_time_series_id(&entry.time_series_id).is_none()"
  and src/state.rs:4834-4841 — "if let Some((asset_id, property_id)) = time_series_id.split_once(TIME_SERIES_ID_SEPARATOR) { return self.assets.get(asset_id).filter(|asset| asset.property(property_id).is_some()).map(...) }"

The same omission on the model-update path: src/state.rs:5644-5651 — "and \"If you remove a property from an asset model, IoT SiteWise deletes all previous data for that property\" [...] a property the model no longer defines goes, and with it its stored time series and its alias binding" implemented at src/state.rs:5734-5740 — "for property in dropped_properties { if let Some(series) = self.property_values.get_mut(&asset_id) { series.remove(&property.id); ..." — again with no segment question.

Test coverage: tests/all/delete_time_series_data_segments.rs:6-11 pins the rule for DeleteTimeSeries ONLY — "The rule these suites pin: a time series may not be deleted while a data segment covering it is still held, so the delete is REFUSED [...] and removes nothing until every such segment is gone, whichever dataset holds it, whichever dataset its data came from, however the request identifies the stream and whichever scope it addresses it in." The suite never sends a DELETE to /assets/{assetId}; nothing pins DeleteAsset's current answer.

## Steps to Reproduce

1. Create an asset model with one MEASUREMENT property and CreateAsset from it; note assetId A and propertyId P.
2. BatchPutAssetPropertyValue some TQVs for (A, P), so the pair-derived time series `A|P` holds data.
3. CreateDataset a SESSION dataset S and a CURATED dataset C in the same workspace.
4. POST /datasets/{C}/data-segments/associate with an entry naming sourceDatasetId=S, timeSeriesId=`A|P` and a non-empty span → 200 with no failedAssociations, so C holds a segment of `A|P`.
5. DELETE /timeseries?assetId=A&propertyId=P → 400 InvalidRequestException "The time series A|P has data segments in the dataset C. Delete them before deleting it." (the rule, as delete_time_series.rs:338-347 implements it).
6. DELETE /assets/A → observed: 202 with assetStatus DELETING; the asset, its properties and `property_values[A]` are gone (state.rs:5378).
7. GET /datasets/{C}/data-segments?workspaceName=... → the segment naming `A|P` is still reported, but no read can resolve that stream any more (series_by_time_series_id returns None), which is the state state.rs:4653-4666 says must never arise.
8. Expected, by the rule the crate states and DeleteTimeSeries implements: DeleteAsset is refused (terminal InvalidRequestException, naming the dataset holding the segment) and removes nothing until those segments are gone.
