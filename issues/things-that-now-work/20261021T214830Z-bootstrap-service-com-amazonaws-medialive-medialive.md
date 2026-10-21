Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the MediaLive emulator

_Filed: 2026-10-21T21:48:30Z_
_Operation: service-com-amazonaws-medialive-medialive | Class: missing_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The MediaLive crate is a fresh scaffold: every one of the 123 operations in `src/ops/` still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented`, and the service holds no state. Implement a FIRST, SMALL slice: the channel lifecycle, which is MediaLive's primary resource and core workflow.

Happy path to implement, in order (wire protocol: `restJson1`, routes rooted at `/prod`):

1. **CreateChannel** — `POST /prod/channels` -> 201. Input: `Name`, `ChannelClass` (SINGLE_PIPELINE | STANDARD), `Destinations`, `EncoderSettings`, `InputAttachments`, `InputSpecification`, `LogLevel`, `RoleArn`, `Tags`, `RequestId` (@idempotencyToken), `Maintenance`. Output: `Channel`. Must allocate a numeric channel `Id`, build `Arn` as `arn:aws:medialive:<region>:<account>:channel:<Id>`, persist every supplied field verbatim, set `State` = `IDLE` (real AWS passes through `CREATING`; settling on `IDLE` so a subsequent describe/start works is acceptable for the first slice), and set `PipelinesRunningCount` = 0 with `PipelineDetails` = [].
2. **DescribeChannel** — `GET /prod/channels/{ChannelId}` -> 200. Must return the stored channel: `Id`, `Arn`, `Name`, `ChannelClass`, `Destinations`, `EgressEndpoints`, `EncoderSettings`, `InputAttachments`, `InputSpecification`, `LogLevel`, `Maintenance`, `PipelineDetails`, `PipelinesRunningCount`, `RoleArn`, `State`, `Tags`. Unknown id -> `NotFoundException` (HTTP 404).
3. **ListChannels** — `GET /prod/channels` -> 200, `@paginated(inputToken=NextToken, outputToken=NextToken, items=Channels)`. Must return a `ChannelSummary` per persisted channel (not a canned/empty list), honour `MaxResults` (@httpQuery `maxResults`) and round-trip `NextToken` (@httpQuery `nextToken`); `NextToken` must be absent on the last page.
4. **StartChannel** — `POST /prod/channels/{ChannelId}/start` -> 200. Must transition the persisted channel from `IDLE` to `RUNNING` and set `PipelinesRunningCount` to 1 for SINGLE_PIPELINE / 2 for STANDARD, then return the full channel description reflecting the new state. Unknown id -> `NotFoundException` (404); starting a channel that is not `IDLE` -> `ConflictException` (409).
5. **StopChannel** — `POST /prod/channels/{ChannelId}/stop` -> 200. Must transition `RUNNING` -> `IDLE`, reset `PipelinesRunningCount` to 0, return the channel. Unknown id -> 404; stopping a non-`RUNNING` channel -> `ConflictException` (409).
6. **DeleteChannel** — `DELETE /prod/channels/{ChannelId}` -> 200. Must remove the channel from state and return its final description with `State` = `DELETING`/`DELETED`. A subsequent `DescribeChannel` must return `NotFoundException` (404), and `ListChannels` must no longer include it. Deleting a `RUNNING` channel -> `ConflictException` (409).

Every one of these must compute its response from persisted state. No canned bodies, no echoing the request back without storing it, no random id handed out without a stored record — a create followed by a describe/list must return exactly what was created, and start/stop must be observable by later readers.

## State Model

In `src/state.rs`, on `ComAmazonawsMedialiveMedialiveState`:

- `channels: BTreeMap<String /* ChannelId */, Channel>` — insertion/id order preserved for deterministic `ListChannels` pagination.
- `next_channel_id: u64` — monotonic counter used to mint `Id` (AWS ids are numeric strings, e.g. `"1234567"`).
- `idempotency_tokens: HashMap<String /* RequestId */, String /* ChannelId */>` — so a repeated `CreateChannel` with the same `RequestId` returns the SAME channel rather than creating a second one.
- `Channel { id: String, arn: String, name: Option<String>, channel_class: ChannelClass, state: ChannelState, destinations: Vec<OutputDestination>, egress_endpoints: Vec<ChannelEgressEndpoint>, encoder_settings: Option<EncoderSettings>, input_attachments: Vec<InputAttachment>, input_specification: Option<InputSpecification>, log_level: Option<LogLevel>, maintenance: Option<MaintenanceStatus>, pipeline_details: Vec<PipelineDetail>, pipelines_running_count: i32, role_arn: Option<String>, tags: HashMap<String,String>, channel_engine_version: Option<ChannelEngineVersionResponse> }`

`ChannelSummary` for `ListChannels` is projected from the same `Channel` record (no second copy of the data).

Out of scope for this first slice (stay 501, filed later as individual gap issues): inputs, input security groups, multiplexes, clusters/nodes/networks (MediaLive Anywhere), schedules, reservations/offerings, signal maps, CloudWatch alarm and EventBridge rule templates, input devices, tagging ops, and the Batch* operations.

## Evidence

`services/service-com-amazonaws-medialive-medialive/OPERATIONS.md` lists 123 operations for `restJson1`/sdkId `MediaLive` and gives the exact contracts cited above: CreateChannel (line 75-80, `POST /prod/channels` -> 201, output `Channel`), DeleteChannel (201-206, `DELETE /prod/channels/{ChannelId}`), DescribeChannel (334-339, `GET /prod/channels/{ChannelId}`, declares `NotFoundException` 404), ListChannels (498-504, `@paginated` with `MaxResults`/`NextToken` and `Channels: list<ChannelSummary>`), StartChannel (700-705) and StopChannel (749-754, both declare `ConflictException` 409 and `NotFoundException` 404, returning the full channel incl. `State` and `PipelinesRunningCount`). The `ChannelState` enum (CREATE_FAILED | CREATING | DELETED | DELETING | IDLE | RECOVERING | RUNNING | STARTING | STOPPING | UPDATE_FAILED | UPDATING) grounds the IDLE/RUNNING lifecycle. `services/service-com-amazonaws-medialive-medialive/src/ops/create_channel.rs:4` carries `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented`, which is true of every file in `src/ops/`; no handler reads or writes state and there are no behavioral tests beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI MediaLive client at it (endpoint prefix `medialive`, paths under `/prod`).
2. `aws medialive create-channel --name demo --channel-class SINGLE_PIPELINE --input-specification Codec=AVC,Resolution=HD,MaximumBitrate=MAX_10_MBPS` -> expect 201 with a `Channel` carrying a minted numeric `Id`, an `Arn`, `State=IDLE`, `PipelinesRunningCount=0` and the submitted `Name`/`ChannelClass`; today it returns HTTP 501 NotImplemented.
3. `aws medialive describe-channel --channel-id <Id>` -> expect 200 echoing the persisted channel exactly as created.
4. `aws medialive list-channels` -> expect the new channel present as a `ChannelSummary`; repeat with `--max-results 1` and follow `NextToken` to page through all channels with no duplicates and no `NextToken` on the final page.
5. `aws medialive start-channel --channel-id <Id>` -> expect 200 with `State=RUNNING`; then `describe-channel` must also report `RUNNING` and `PipelinesRunningCount=1`.
6. `aws medialive delete-channel --channel-id <Id>` while RUNNING -> expect `ConflictException` (HTTP 409).
7. `aws medialive stop-channel --channel-id <Id>` -> expect 200 with `State=IDLE`, `PipelinesRunningCount=0`.
8. `aws medialive delete-channel --channel-id <Id>` -> expect 200; then `describe-channel --channel-id <Id>` must return `NotFoundException` (HTTP 404) and `list-channels` must no longer list it.
9. `aws medialive describe-channel --channel-id 0000000` for an id never created -> expect `NotFoundException` (HTTP 404), not 501.
