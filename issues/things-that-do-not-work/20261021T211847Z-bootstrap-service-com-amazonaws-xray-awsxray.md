Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the AWS X-Ray emulator

_Filed: 2026-10-21T21:18:47Z_
_Operation: service-com-amazonaws-xray-awsxray | Class: stub_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 38 operations in `services/service-com-amazonaws-xray-awsxray/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented`, and the crate holds no state. Nothing works end-to-end, so an X-Ray client (segment ingest via the daemon/SDK, then console-style trace lookup) cannot complete a single call.

Implement a FIRST SLICE of 6 operations (restJson1, sigv4 signing name `xray`, all POST routes) so the core ingest -> query -> read loop and the Group lifecycle work statefully. Every one of these MUST compute its response from persisted state — no canned bodies, no echoing the input back, no generating an id that is never stored.

### 1. PutTraceSegments — `POST /TraceSegments`
- Input: `TraceSegmentDocuments` (list<string>, required) — each entry is a JSON segment document.
- Behaviour: parse each document; take `trace_id`, `id`, `name`, `start_time`, `end_time`, `error`/`fault`/`throttle`, `http`, `parent_id`. Store the raw document verbatim (BatchGetTraces must return it byte-for-byte) under its trace. A document that is not valid JSON, or is missing `trace_id`/`id`, goes into `UnprocessedTraceSegments` as `UnprocessedTraceSegment{Id, ErrorCode, Message}` rather than failing the whole call.
- Output: `UnprocessedTraceSegments` — empty list when all documents were accepted.

### 2. GetTraceSummaries — `POST /TraceSummaries`
- Input: `StartTime` (required), `EndTime` (required), `TimeRangeType`, `Sampling`, `FilterExpression`, `NextToken`.
- Behaviour: select the stored traces whose segment `start_time` falls in [StartTime, EndTime) and build each `TraceSummary` from the persisted segments: `Id`, `Duration` (max end_time - min start_time across segments), `ResponseTime`, `HasError`/`HasFault`/`HasThrottle`/`IsPartial`, `Http` (url/method/status from the root segment's `http` block), `ServiceIds`/`ServiceNames`, `Annotations` if present. Set `TracesProcessedCount` to the number of traces scanned and `ApproximateTime` to now. A trace just written by PutTraceSegments MUST appear here. `FilterExpression` may initially be accepted and ignored only if unfiltered results are still state-derived (a later gap issue covers filter evaluation).
- Output: `TraceSummaries`, `ApproximateTime`, `TracesProcessedCount`, `NextToken` (honour the @paginated contract: emit a token only when results were truncated, and resume from it).

### 3. BatchGetTraces — `POST /Traces`
- Input: `TraceIds` (list<string>, required), `NextToken`.
- Behaviour: for each requested id return `Trace{Id, Duration, LimitExceeded, Segments:[Segment{Id, Document}]}` rebuilt from the stored segment documents; ids with no stored segments go to `UnprocessedTraceIds`. `Document` must be the exact string passed to PutTraceSegments.
- Output: `Traces`, `UnprocessedTraceIds`, `NextToken`.

### 4. CreateGroup — `POST /CreateGroup`
- Input: `GroupName` (required, len 1..32), `FilterExpression`, `InsightsConfiguration`, `Tags`.
- Behaviour: insert a group keyed by name; mint and persist `GroupARN` of the form `arn:aws:xray:<region>:<account>:group/<GroupName>/<12-hex-id>`. Reject a missing/over-length name with `InvalidRequestException` (400).
- Output: `Group{GroupName, GroupARN, FilterExpression, InsightsConfiguration}` — the stored record, not the raw input.

### 5. GetGroup / GetGroups — `POST /GetGroup`, `POST /Groups`
- GetGroup input: `GroupName` or `GroupARN` (either resolves the same record); output `Group`. Unknown group or neither field supplied -> `InvalidRequestException` (400).
- GetGroups: output `Groups` (list<GroupSummary> built from the stored map, including every group created above) plus `NextToken` honouring the @paginated contract.

### 6. DeleteGroup — `POST /DeleteGroup`
- Input: `GroupName` or `GroupARN`. Remove the record; a subsequent GetGroup must fail and GetGroups must no longer list it. Deleting an unknown group -> `InvalidRequestException` (400). Output has no members.

## State Model
Add to `src/state.rs` (`ComAmazonawsXrayAwsxrayState`, reached via `crate::state::service_state(&*self.engine)`):

- `traces: BTreeMap<String /*trace_id*/, TraceRecord>` where
  `TraceRecord { id: String, segments: Vec<SegmentRecord>, first_start: f64, last_end: f64 }`
- `SegmentRecord { id: String, parent_id: Option<String>, name: String, document: String /*verbatim JSON as submitted*/, start_time: f64, end_time: Option<f64>, error: bool, fault: bool, throttle: bool, http: Option<HttpRecord>, annotations: BTreeMap<String, serde_json::Value> }`
- `HttpRecord { client_ip: Option<String>, http_method: Option<String>, url: Option<String>, user_agent: Option<String>, response_status: Option<i32> }`
- `groups: BTreeMap<String /*GroupName*/, GroupRecord>` where
  `GroupRecord { group_name: String, group_arn: String, filter_expression: Option<String>, insights_configuration: Option<InsightsConfigurationRecord>, tags: Vec<(String, String)> }`
- plus an index `group_arn_to_name: BTreeMap<String, String>` so GetGroup/DeleteGroup can resolve by ARN.

All six handlers read and mutate ONLY this shared store (never `engine::InMemoryEngine::new()` inside a handler), and each must delete its `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line when implemented.

## Out of scope for this slice
The remaining 32 operations (sampling rules, insights, service graph, telemetry records, resource policies, encryption config, trace retrieval, tagging, indexing rules, trace segment destination) stay at 501 and will be filed as individual gap issues later.

## Done when
A client script can put segments and then see them back, by summary and by id, and can create/read/list/delete a group — with every field in those responses derived from what was stored, not from the request.

## Evidence

OPERATIONS.md for service-com-amazonaws-xray-awsxray (38 operations, protocol restJson1, sigv4 name `xray`) documents the exact routes and members cited: PutTraceSegments `POST /TraceSegments` in: TraceSegmentDocuments (required) out: UnprocessedTraceSegments; GetTraceSummaries `POST /TraceSummaries` in: StartTime/EndTime required, TimeRangeType, Sampling, SamplingStrategy, FilterExpression, NextToken out: TraceSummaries/ApproximateTime/TracesProcessedCount/NextToken @paginated(items=TraceSummaries); BatchGetTraces `POST /Traces` in: TraceIds required out: Traces/UnprocessedTraceIds/NextToken @paginated(items=Traces); CreateGroup `POST /CreateGroup` in: GroupName required len 1..32, FilterExpression, InsightsConfiguration, Tags out: Group; GetGroup `POST /GetGroup` in: GroupName|GroupARN out: Group; GetGroups `POST /Groups` out: Groups/NextToken @paginated(items=Groups); DeleteGroup `POST /DeleteGroup` in: GroupName|GroupARN, no output members. Declared errors on these ops are InvalidRequestException (400) and ThrottledException (429). Census: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-xray-awsxray/src/ops/` matches all 38 handler files (e.g. put_trace_segments.rs, get_trace_summaries.rs, batch_get_traces.rs, create_group.rs, get_group.rs, get_groups.rs, delete_group.rs), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501. No state fields and no behavioural tests exist (tests/ holds only the generated state_roundtrip.rs). Segment-document semantics (trace_id/id/start_time/end_time/http/error/fault, and Segment.Document being the verbatim submitted JSON) are well-established X-Ray behaviour.

## Steps to Reproduce

1. Start the emulator and point an X-Ray client at it (endpoint prefix `xray`, restJson1).
2. PutTraceSegments with one root segment document: {"trace_id":"1-581cf771-a006649127e371903a2de979","id":"70de5b6f19ff9a0a","name":"svc","start_time":1478293361.271,"end_time":1478293361.449,"http":{"request":{"method":"GET","url":"http://localhost/"},"response":{"status":200}}} — expect 200 with UnprocessedTraceSegments empty; today 501 NotImplemented.
3. GetTraceSummaries with StartTime/EndTime bracketing 1478293361 — expect one TraceSummary with Id=1-581cf771-a006649127e371903a2de979, Duration≈0.178, Http.HttpURL/HttpMethod/HttpStatus from the stored segment, and TracesProcessedCount>=1.
4. BatchGetTraces with TraceIds=[that id] — expect Traces[0].Segments[0].Document equal to the exact JSON string submitted in step 2, and UnprocessedTraceIds empty.
5. BatchGetTraces with TraceIds=["1-00000000-000000000000000000000000"] — expect that id in UnprocessedTraceIds and Traces empty.
6. CreateGroup GroupName=demo FilterExpression="service(\"svc\")" — expect Group.GroupARN minted and persisted.
7. GetGroup GroupName=demo and then GetGroup GroupARN=<the ARN returned> — both must return the same stored Group including FilterExpression.
8. GetGroups — expect the demo group listed in Groups.
9. DeleteGroup GroupName=demo, then GetGroup GroupName=demo — expect InvalidRequestException (400) and the group absent from GetGroups.
