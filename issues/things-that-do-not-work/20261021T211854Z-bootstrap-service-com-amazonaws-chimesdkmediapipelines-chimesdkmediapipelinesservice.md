Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the Chime SDK Media Pipelines emulator

_Filed: 2026-10-21T21:18:54Z_
_Operation: service-com-amazonaws-chimesdkmediapipelines-chimesdkmediapipelinesservice | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 31 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. This issue defines the FIRST minimal, stateful slice: the media capture pipeline lifecycle, which is the service's primary resource (`restJson1`, sigv4 signing name `chime`, endpoint prefix `media-pipelines-chime`).

### Happy path to implement, in order

1. **CreateMediaCapturePipeline** — `POST /sdk-media-capture-pipelines -> 201`. Input: `SourceType` (enum, only `ChimeSdkMeeting`, required), `SourceArn` (required, len 1..1024, `^arn[\/\:\-\_\.a-zA-Z0-9]+$`), `SinkType` (enum, only `S3Bucket`, required), `SinkArn` (required, same ARN pattern), plus optional `ClientRequestToken` (@idempotencyToken, len 2..64), `ChimeSdkMeetingConfiguration`, `SseAwsKeyManagementParams`, `SinkIamRoleArn`, `Tags`. Must MINT and PERSIST a new pipeline: generate `MediaPipelineId` as a fresh lowercase UUID matching `^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}$`, derive `MediaPipelineArn` (`arn:aws:chime:<region>:<account>:media-pipeline/<MediaPipelineId>`), set `Status` to a real `MediaPipelineStatus` value (`Initializing`), set `CreatedTimestamp`/`UpdatedTimestamp` to the current time serialized as ISO-8601 date-time, and echo back the stored (not merely the request's) `SourceType`/`SourceArn`/`SinkType`/`SinkArn`/`ChimeSdkMeetingConfiguration`/`SinkIamRoleArn` inside the returned `MediaCapturePipeline`. Output: `MediaCapturePipeline`.
2. **GetMediaCapturePipeline** — `GET /sdk-media-capture-pipelines/{MediaPipelineId} -> 200`. Must look the pipeline up in the store by id and return the SAME `MediaCapturePipeline` structure that Create persisted (same id, arn, source/sink, status, timestamps). An unknown but well-formed id must return `NotFoundException` (HTTP 404), not an empty 200.
3. **ListMediaCapturePipelines** — `GET /sdk-media-capture-pipelines -> 200`. Returns `MediaCapturePipelines` as a list of `MediaCapturePipelineSummary{MediaPipelineId, MediaPipelineArn}` built from the store — every created-and-not-deleted pipeline appears exactly once. Honour `max-results` (range 1..100) by truncating and emitting an opaque `NextToken` only when more remain; a supplied `next-token` resumes after the last returned item. Empty store = empty/absent list with no token.
4. **GetMediaPipeline** — `GET /sdk-media-pipelines/{MediaPipelineId} -> 200`. Same id space as above; returns the polymorphic `MediaPipeline` union-style structure with ONLY the `MediaCapturePipeline` member populated for a capture pipeline (other members left absent). 404 `NotFoundException` for unknown ids.
5. **DeleteMediaCapturePipeline** — `DELETE /sdk-media-capture-pipelines/{MediaPipelineId} -> 204`. Removes the pipeline from the store, empty body. A second delete, and any subsequent Get/GetMediaPipeline for that id, must return `NotFoundException` (404), and the id must disappear from ListMediaCapturePipelines/ListMediaPipelines.
6. **ListMediaPipelines** — `GET /sdk-media-pipelines -> 200`. Returns `MediaPipelines` as `MediaPipelineSummary{MediaPipelineId, MediaPipelineArn}` over the SAME store that backs step 1, with the same `max-results`/`next-token` paging rules, so a pipeline created in step 1 is visible here too.

### Requirements

- NEVER STUBS: every one of these six handlers must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)`. Hardcoded or echoed-input responses, and ids minted without being stored, are failures — a Create followed by a Get must return what was created, and a Delete must be observable by a later read.
- Validation on the happy path: reject a non-matching `SourceType`/`SinkType` enum value, an `SourceArn`/`SinkArn` violating the ARN pattern/length, and a `MediaPipelineId` path label not 36 chars matching the GUID pattern, with `BadRequestException` (HTTP 400) carrying the modeled error shape — not a 500 or a 501.
- Idempotency: two Creates with the same `ClientRequestToken` return the same `MediaPipelineId` rather than creating two pipelines.
- Done = a client script running steps 1→6 end-to-end passes with correct bodies, not merely 2xx: the id/ARN returned by Create is the one Get echoes, appears in both list calls, and is gone (404) after Delete.

## State Model

Store in the service's existing state struct (`src/state.rs`, `ComAmazonawsChimesdkmediapipelinesChimesdkmediapipelinesserviceState`), all under one lock:

- `capture_pipelines: IndexMap<String /* MediaPipelineId (uuid) */, CapturePipeline>` — insertion-ordered so list paging is stable.
- `CapturePipeline { media_pipeline_id: String, media_pipeline_arn: String, source_type: MediaPipelineSourceType, source_arn: String, sink_type: MediaPipelineSinkType, sink_arn: String, sink_iam_role_arn: Option<String>, status: MediaPipelineStatus, created_timestamp: DateTime, updated_timestamp: DateTime, chime_sdk_meeting_configuration: Option<ChimeSdkMeetingConfiguration>, sse_aws_key_management_params: Option<SseAwsKeyManagementParams>, tags: Vec<Tag> }`.
- `idempotency_tokens: HashMap<String /* ClientRequestToken */, String /* MediaPipelineId */>` — for Create replay.
- `next_token_cursors`: paging may be implemented as an opaque base64 of the last-returned `MediaPipelineId` (no extra state) — do not return a token when the page is the last one.

Leave room for the sibling resources (`media_insights_configurations`, `kinesis_video_stream_pools`, `insights_pipelines`, voice/speaker tasks) as later slices; they stay 501 for now.

The remaining 25 operations (concatenation/live-connector/stream/insights pipelines, insights pipeline configurations, Kinesis video stream pools, tagging, speaker-search and voice-tone-analysis tasks) intentionally remain scaffolded and will be filed as individual gap issues.

## Evidence

All 31 `src/ops/*.rs` files in `services/service-com-amazonaws-chimesdkmediapipelines-chimesdkmediapipelinesservice/src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501, confirmed by grep across the whole ops directory; there is no state wired into any handler. Operation contracts (routes, required members, status codes, error list, @paginated traits) come from the crate's generated `OPERATIONS.md` (CreateMediaCapturePipeline, GetMediaCapturePipeline, ListMediaCapturePipelines, GetMediaPipeline, DeleteMediaCapturePipeline, ListMediaPipelines sections). Response field names and constraints (`MediaCapturePipeline.MediaPipelineId`/`MediaPipelineArn`/`Status`/`CreatedTimestamp`/`UpdatedTimestamp`, `MediaCapturePipelineSummary`, `MediaPipelineSummary`, `MediaPipeline` with per-kind members, `MediaPipelineStatus` values Initializing|InProgress|Failed|Stopping|Stopped|Paused|NotStarted) come from `smithy_query` on the service model at `smithy-model/com-amazonaws-chimesdkmediapipelines-chimesdkmediapipelinesservice.json`. 404 `NotFoundException` on unknown id and 400 `BadRequestException` on malformed input are the modeled errors for exactly these operations.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI chime-sdk-media-pipelines client at it.
2. `aws chime-sdk-media-pipelines create-media-capture-pipeline --source-type ChimeSdkMeeting --source-arn arn:aws:chime:us-east-1:123456789012:meeting/abcd --sink-type S3Bucket --sink-arn arn:aws:s3:::my-bucket/captures` — expect 201 with a fresh GUID `MediaPipelineId`, a `MediaPipelineArn`, `Status: Initializing`, and ISO-8601 timestamps; today returns HTTP 501 NotImplemented.
3. `aws chime-sdk-media-pipelines get-media-capture-pipeline --media-pipeline-id <id from step 2>` — expect 200 echoing the same id/arn/source/sink that were persisted; today 501.
4. `aws chime-sdk-media-pipelines list-media-capture-pipelines --max-results 1` — expect the created pipeline's summary in `MediaCapturePipelines`, with `NextToken` present only if more pipelines exist; today 501.
5. `aws chime-sdk-media-pipelines get-media-pipeline --media-pipeline-id <id>` — expect 200 with only the `MediaCapturePipeline` member of `MediaPipeline` populated; today 501.
6. `aws chime-sdk-media-pipelines list-media-pipelines` — expect the same id to appear as a `MediaPipelineSummary`; today 501.
7. `aws chime-sdk-media-pipelines delete-media-capture-pipeline --media-pipeline-id <id>` — expect 204 with empty body; today 501.
8. Repeat the get-media-capture-pipeline and get-media-pipeline calls — expect HTTP 404 `NotFoundException`; re-run both list calls and expect the id to be gone.
9. Negative check: `get-media-capture-pipeline --media-pipeline-id not-a-guid` expects 400 `BadRequestException`, and `create-media-capture-pipeline --source-type Bogus ...` expects 400 `BadRequestException`.
