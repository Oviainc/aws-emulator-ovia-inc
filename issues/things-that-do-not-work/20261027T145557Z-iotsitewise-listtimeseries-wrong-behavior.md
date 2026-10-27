Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 83df1cdc32a5bdf510e34eb14380f1c3f1e573c9

# A workspace-scoped ListTimeSeries always answers an empty page, on the strength of a comment that denies the crate records a workspace for any time series

_Filed: 2026-10-27T14:55:57Z_
_Operation: ListTimeSeries | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `83df1cdc32a5bdf510e34eb14380f1c3f1e573c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListTimeSeries` short-circuits any request carrying `workspaceName`: once the workspace resolves, it returns an empty `timeSeriesSummaries` list without looking at any stored record. The comment above it justifies this with "A workspace it DOES hold holds no time series — nothing that ingests one records a workspace for it".

That premise is false inside this crate. The store keeps `capture_sources: HashMap<workspace, HashMap<time_series_id, CaptureSourceRecord>>` plus a `capture_source_aliases` index, each record carrying `workspace_name`, `time_series_id`, `alias` and a `data_type`; `put_capture_segment` writes them keyed by workspace; `GetCaptureData` resolves one BY the very identifiers a `TimeSeriesSummary` reports (`capture_source_by_id` / `capture_source_by_alias`); and `forget_workspace` deletes them when the workspace goes. `DeleteWorkspace`'s own comment names them explicitly as "the workspace-keyed video streams a capture source holds".

So a workspace that holds capture sources is reported as holding no time series: a hollow 200 that says "this workspace has nothing" where the crate can enumerate the streams and `GetCaptureData` will happily read them. The same false premise is pasted into `DescribeTimeSeries` (describe_time_series.rs:253-271) and `DeleteTimeSeries` (delete_time_series.rs:202-218), which unconditionally 404 a workspace-scoped lookup — including for an alias the crate's own `capture_source_by_alias` index resolves — so the fix has three copies to correct.

## Evidence

The handler and its premise:
- services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/list_time_series.rs:176-194 — "// `workspaceName` scopes the listing to one SiteWise workspace: ... A workspace it DOES hold holds no time / series — nothing that ingests one records a workspace for it — so the / listing is empty rather than refused" followed by "return Ok(ListTimeSeriesOutput { time_series_summaries: Vec::new(), next_token: None, workspace_name: Some(workspace.to_string()) });"

The same crate, contradicting it:
- services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/state.rs:56-60 — "A / workspace's capture sources — the video streams `GetCaptureData` reads — live / in [`...::capture_sources`], keyed by / the workspace and the source's `timeSeriesId`, with / [`...::capture_source_aliases`] the / index the alias form of that lookup resolves through."
- services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/state.rs:4718-4729 — `put_capture_segment` inserting `CaptureSourceRecord { workspace_name: workspace_name.to_string(), time_series_id: time_series_id.to_string(), alias: None, data_type: data_type.to_string(), segments: Vec::new() }`.
- services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/delete_workspace.rs:186-191 — "a TIME / SERIES is not among the contents counted here because this store holds none / inside a workspace — `DeleteTimeSeries` scoped to one resolves to nothing — / and the workspace-keyed video streams a capture source holds are ingested / data rather than a resource a caller creates and deletes one at a time" (an argument for not BLOCKING the delete, not for hiding the streams from a listing — and it names the streams the listing denies).
- services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/get_capture_data.rs:229-242 — the read that resolves exactly those streams per workspace via `store.capture_source_by_id(&input.workspace_name, time_series_id)` / `store.capture_source_by_alias(&input.workspace_name, alias)`.

Tests: tests/all/capture_data.rs populates capture sources through `store.put_capture_segment(workspace, id, alias, data_type, segment(..))` (lines 161, 760, 779) and asserts GetCaptureData reads them; no test asserts the empty workspace-scoped listing, so nothing pins the current answer.

## Steps to Reproduce

1. CreateWorkspace(workspaceName='w1').
2. Seed a capture source for 'w1' the way tests/all/capture_data.rs does (store.put_capture_segment("w1", <timeSeriesId>, Some(alias), "VIDEO-MP4", segment(..))).
3. GetCaptureData(workspaceName='w1', timeSeriesId=<timeSeriesId>, startTime/endTime spanning the segment) -> 200 with the video, proving the workspace holds that time series.
4. ListTimeSeries(workspaceName='w1') -> 200 with timeSeriesSummaries == [], and DescribeTimeSeries(workspaceName='w1', alias=<alias>) -> 404 "The time series the request identifies could not be found in the workspace w1."
