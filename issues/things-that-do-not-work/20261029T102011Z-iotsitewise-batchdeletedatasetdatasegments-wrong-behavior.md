Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 99c19747b30de4fba88ebdfb7af58fcb882c179f

# BatchDeleteDatasetDataSegments can never delete anything: no operation of this service ever records a data segment on a SESSION dataset, so every entry is answered RESOURCE_NOT_FOUND

_Filed: 2026-10-29T10:20:11Z_
_Operation: BatchDeleteDatasetDataSegments | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `99c19747b30de4fba88ebdfb7af58fcb882c179f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler is implemented in full — it gates the request on the dataset being a SESSION dataset, addresses each entry by `(timeSeriesId, startTimestamp, endTimestamp)`, removes the segment, deletes the underlying samples inside the span and bumps the dataset version — but over the wire none of that can ever run, because no code path in this crate ever stores a data segment on a session dataset.

`store.dataset_data_segments` is written in exactly one place (`put_dataset_data_segment`), and that writer is called from exactly one handler: BatchAssociateDataSegmentsToDataset, which refuses any target dataset whose stored `datasetType` is not CURATED. So the segment log of a dataset whose type is SESSION — the only type this handler admits apart from a dataset stored with no type at all — is permanently absent, `delete_dataset_data_segment` returns `false` for every entry, and the response is always `errors` carrying one RESOURCE_NOT_FOUND element per entry with `datasetVersion` unchanged. The branch that implements the documented side effect ("Deleting a data segment deletes the underlying time series data for the segment's time range", `forget_series_window`) is unreachable from any HTTP request.

The producer the model names for a session dataset's own data is a bulk import job into it (`CreateBulkImportJob.datasetId` = "The ID of the session dataset to ingest data into"), and this crate's CreateBulkImportJob records only a job record — it calls no segment writer. The crate's own test harness states the hole: it has to reach into `crate::state` to seed segments because no operation produces them.

The same root cause makes ListDatasetDataSegments on a session dataset permanently empty.

## Evidence

Model text for the operation (smithy_query(query="doc", shape="BatchDeleteDatasetDataSegments")): "Deletes a batch of data segments from a session dataset. Deleting a data segment deletes the underlying time series data for the segment's time range."

The handler's SESSION-only gate — services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/batch_delete_dataset_data_segments.rs:273-279:
  "if let Some(dataset_type) = dataset_type.filter(|held| held != DATASET_TYPE_SESSION) {
        return Err(invalid_request(format!(
            \"Data segments can be deleted only from a {DATASET_TYPE_SESSION} dataset, but the dataset {} is a {dataset_type} dataset.\","

The per-entry answer that is therefore always taken — src/ops/batch_delete_dataset_data_segments.rs:304-318:
  "if !store.delete_dataset_data_segment(&input.dataset_id, &entry.time_series_id, &entry.start_timestamp, &entry.end_timestamp) {
        errors.push(failed(entry, ERROR_CODE_RESOURCE_NOT_FOUND, crate::state::not_found_message(crate::state::RESOURCE_DATA_SEGMENT, &entry.time_series_id)));
        continue;"
and the unreachable data deletion right after it, src/ops/batch_delete_dataset_data_segments.rs:321-333 — "// \"Deleting a data segment deletes the underlying time series data for the segment's time range\" ... store.forget_series_window(&stream, &entry.start_timestamp, &entry.end_timestamp);"

Why the lookup always misses — src/state.rs:4714-4720:
  "pub fn delete_dataset_data_segment(... ) -> bool {
        let Some(held) = self.dataset_data_segments.get(dataset_id) else {
            return false;
        };"

The ONLY writer, and its ONLY call site — src/state.rs:4595 "pub fn put_dataset_data_segment(" / src/state.rs:4608 "let held = self.dataset_data_segments.entry(dataset_id.to_string()).or_default();", called from src/ops/batch_associate_data_segments_to_dataset.rs:432 "if !store.put_dataset_data_segment(" — whose own gate forbids a session dataset as the holder, src/ops/batch_associate_data_segments_to_dataset.rs:303-308:
  "if let Some(dataset_type) = dataset_type.filter(|held| held != DATASET_TYPE_CURATED) {
        return Err(invalid_request(format!(
            \"Data segments can be associated only with a {DATASET_TYPE_CURATED} dataset, but the dataset {} is a {dataset_type} dataset.\","
(grep for `put_dataset_data_segment` across the crate returns only state.rs and that one handler.)

The crate admitting the gap, in its own test harness — tests/all/dataset_data_segments.rs:570-573:
  "/// Seed `spans` as the data segments of `dataset_id`, through the producer entry point the state exposes — the operations that associate or ingest a dataset's segments are not implemented yet, so this is where a dataset's segments come from today."
and the seeding itself, tests/all/dataset_data_segments.rs:583-591 — "let stored = store.put_dataset_data_segment(dataset_id, DataSegmentRecord { source_dataset_id: dataset_id.to_string(), ... });" (source == holder, a combination no handler can produce).

The producer the model names, recorded but never acted on — src/ops/create_bulk_import_job.rs:63-64 (the modelled contract copied into the header): "* `datasetId` ... — The ID of the session dataset to ingest data into. Specify this field, together with workspaceName, to ingest data into a session dataset in a workspace." — and that handler never calls any segment writer.

Test coverage: the behaviour with segments present is pinned only by suites that seed through the state API (tests/all/dataset_data_segments.rs:574-594, tests/all/list_dataset_data_segments.rs, tests/all/delete_time_series_data_segments.rs:353). No test drives a session dataset's segments through HTTP, so nothing pins the always-empty wire behaviour.

## Steps to Reproduce

1. POST /workspaces with workspaceName=press-floor to create a workspace.
2. POST /datasets with {"datasetName":"session-1","datasetType":"SESSION","workspaceName":"press-floor"} -> 201 with a datasetId; the dataset reads back ACTIVE.
3. Ingest data for it with CreateBulkImportJob naming that datasetId (the modelled producer of a session dataset's data) and/or BatchPutAssetPropertyValue, and wait for the job to report COMPLETED.
4. GET /datasets/{datasetId}/data-segments?workspaceName=press-floor -> 200 with dataSegments: [] (no segment exists).
5. POST /datasets/{datasetId}/data-segments/batch-delete with {"workspaceName":"press-floor","deleteDataSegmentEntries":[{"timeSeriesId":"<any id>","startTimestamp":{...},"endTimestamp":{...}}]}.
6. Observed: 200 with datasetVersion unchanged and errors[0].errorCode = RESOURCE_NOT_FOUND for every entry, whatever the entry names — and no time series samples are removed.
7. Expected per the operation's own documentation ("Deletes a batch of data segments from a session dataset. Deleting a data segment deletes the underlying time series data for the segment's time range"): an entry naming data the session dataset holds removes the segment and the samples in its span. Reaching that state requires a producer; today only an in-process `store.put_dataset_data_segment` call can create one (tests/all/dataset_data_segments.rs:583).
