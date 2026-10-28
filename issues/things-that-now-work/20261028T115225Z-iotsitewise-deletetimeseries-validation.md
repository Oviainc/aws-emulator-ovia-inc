Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a

# DeleteTimeSeries deletes a time series that still has data segments, leaving dataset segments naming a stream the store no longer holds

_Filed: 2026-10-28T11:52:25Z_
_Operation: DeleteTimeSeries | Class: validation_

> _Note: this issue was identified against the code base at commit `0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The modelled documentation for DeleteTimeSeries states a precondition on the delete: a time series may not be deleted while any of its data segments still exist. The handler never looks at the segments. Neither the workspace-scoped path nor the account-scoped path consults `store.dataset_data_segments` (the map whose records carry the `time_series_id` the segment covers), so the stream — alias binding, samples and capture source — is removed while datasets still hold segments that name it by `timeSeriesId`.

Two consequences: (1) the documented refusal never happens, and (2) the store is left holding data segments whose time series is gone, which ListDatasetDataSegments/ListDatasetDataSegmentRelationships keep reporting and which BatchDeleteDatasetDataSegments then has to special-case. Sibling deletes in this same crate do evaluate exactly this kind of dependency before removing anything (DeleteAsset refuses while hierarchy children exist; DeletePipeline refuses while executions are active), and the state module already exposes the per-time-series index this check needs.

## Evidence

Model text (smithy_query(query="doc", shape="DeleteTimeSeries")), also copied into the handler header at services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/delete_time_series.rs:50-51:
  "You can't delete a time series until all of its data segments have been deleted from session datasets."

Handler — the whole delete, with no mention of a segment anywhere in the file (grep for "segment" in delete_time_series.rs matches only the doc line 51):
  src/ops/delete_time_series.rs:259-277
    "if let Some(workspace) = input.workspace_name.as_deref() { let time_series_id = store.workspace_series(workspace, form)...; store.forget_capture_source(workspace, &time_series_id); ... return Ok(DeleteTimeSeriesOutput); }"
  src/ops/delete_time_series.rs:333-343
    "match &stream { crate::state::SeriesKey::Property(asset_id, property_id) => { store.forget_property_series(asset_id, property_id, now); } crate::state::SeriesKey::Disassociated(alias) => { store.forget_disassociated_time_series(alias); } }"

The state the check would read, already in the crate:
  src/state.rs:3978 "pub dataset_data_segments: HashMap<String, Vec<DataSegmentRecord>>,"
  src/state.rs:1728 "pub time_series_id: String," (on DataSegmentRecord)
  src/state.rs:4648-4653 "pub fn overlapping_data_segment_association(&self, time_series_id: &str, start: &TimeInNanos, end: &TimeInNanos) -> Option<(&str, &DataSegmentRecord)>" — already searches EVERY dataset's segments for one time series.

Sibling deletes that do refuse on a live dependency:
  src/ops/delete_asset.rs:224-232 "if store.has_hierarchy_children(&id) { ... \"The asset {id} has child assets associated with its hierarchies...\" }"
  src/ops/delete_pipeline.rs:182-197 "let active = store.active_pipeline_execution_count(&workspace, &name, now); if active > 0 { return Err(... ConflictingOperationException ...) }"

That the crate knows this dangling state can be produced:
  src/ops/batch_delete_dataset_data_segments.rs:313-315 "A segment whose time series the store no longer holds has no data left to delete, which is not a reason to keep the segment."

Tests: tests/all/time_series_ingestion.rs:472 (delete identifies one series), tests/all/workspace_time_series.rs:735-805 and tests/all/refusal_message_rule.rs:282-287 cover DeleteTimeSeries; none of them creates a data segment first, so no test pins the current permissiveness.

## Steps to Reproduce

1. CreateWorkspace, then CreateDataset twice in it: one SESSION dataset (the source) and one CURATED dataset (the holder).
2. Ingest samples for a time series in that workspace (BatchPutAssetPropertyValue / the alias-addressed form) and note its timeSeriesId via ListTimeSeries.
3. BatchAssociateDataSegmentsToDataset on the curated dataset with an entry naming that timeSeriesId and a non-empty [start, end) span; confirm failedAssociations is empty and ListDatasetDataSegments reports the segment.
4. Call DeleteTimeSeries for that stream (by alias, or by assetId+propertyId, with or without workspaceName).
5. Observed: HTTP 200 and the stream is gone. Expected per the quoted model sentence: the delete is refused while the segment exists.
6. Call ListDatasetDataSegments on the curated dataset again: the segment is still listed, naming a timeSeriesId no stored stream carries.
