Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the IVS RealTime emulator

_Filed: 2026-10-21T21:08:39Z_
_Operation: service-com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 39 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow can run. Implement a FIRST SLICE covering the lifecycle of the service's primary resource, the **Stage**, plus the token issuance that makes a stage usable. Scope (restJson1, all routes POST to an operation-named path):

1. **CreateStage** (`POST /CreateStage`) — input `name` (0..128, `^[a-zA-Z0-9-_]*$`), `participantTokenConfigurations` (0..12 of `ParticipantTokenConfiguration{duration, userId, attributes, capabilities}`), `tags`, `autoParticipantRecordingConfiguration`. Must mint a new stage ARN of the modeled form `arn:aws:ivs:<region>:<account>:stage/<id>`, persist the record, and return `stage` (`Stage{arn (required), name, activeSessionId, tags, autoParticipantRecordingConfiguration, endpoints}`) plus one `ParticipantToken` per entry in `participantTokenConfigurations`, each token also persisted so it can be looked up later.
2. **GetStage** (`POST /GetStage`) — input `arn` (required). Must look the stage up in state and return the SAME `stage` object that CreateStage returned (same arn, name, tags, recording configuration). Unknown ARN => `ResourceNotFoundException` (HTTP 404); malformed ARN (fails the modeled pattern/length) => `ValidationException` (HTTP 400).
3. **ListStages** (`POST /ListStages`) — input `maxResults` (1..100), `nextToken` (0..1024, `^[a-zA-Z0-9+/=_-]*$`). Must return `stages` (required) as `StageSummary{arn, name, activeSessionId, tags}` built from the stored stages — every created stage appears, deleted stages do not. Honour `maxResults` as a page bound and return an opaque `nextToken` that resumes where the page ended; omit `nextToken` on the last page.
4. **CreateParticipantToken** (`POST /CreateParticipantToken`) — input `stageArn` (required), `duration` (1..20160, default 720), `userId`, `attributes`, `capabilities` (0..2). Must verify the stage exists (else `ResourceNotFoundException` 404), then persist and return `participantToken` (`ParticipantToken{participantId, token, userId, attributes, duration, capabilities, expirationTime}`) where `participantId` is newly generated and unique, `userId`/`attributes`/`capabilities`/`duration` reflect what was requested (with the 720-minute default applied when `duration` is absent), and `expirationTime` is now + duration minutes serialized as an ISO-8601 date-time.
5. **UpdateStage** (`POST /UpdateStage`) — input `arn` (required), `name`, `autoParticipantRecordingConfiguration`. Must mutate the stored stage in place (only the members supplied) and return the UPDATED `stage`; a following GetStage/ListStages must show the new `name`. Unknown ARN => `ResourceNotFoundException` 404.
6. **DeleteStage** (`POST /DeleteStage`) — input `arn` (required), empty output. Must remove the stage (and its tokens) from state; a following GetStage must return `ResourceNotFoundException` 404 and ListStages must no longer include it. Deleting an unknown ARN => `ResourceNotFoundException` 404.

No operation in this slice may return a canned, zeroed or echoed-back response: each must compute its response from the persisted store reached through `crate::state::service_state(&*self.engine)` (never a freshly constructed engine), and each must delete its `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line. Validation of the modeled `@required` members, length ranges, integer ranges and ARN patterns must produce `ValidationException` (HTTP 400), not a 500 or a silent success.

## State Model

In `src/state.rs`, on the service state struct:

- `stages: HashMap<String /* stage ARN */, Stage>` where
  `Stage { arn: String, name: Option<String>, active_session_id: Option<String>, tags: HashMap<String,String>, auto_participant_recording_configuration: Option<AutoParticipantRecordingConfiguration>, endpoints: Option<StageEndpoints>, created_at: SystemTime }`
- `participant_tokens: HashMap<String /* stage ARN */, Vec<ParticipantTokenRecord>>` where
  `ParticipantTokenRecord { participant_id: String, token: String, user_id: Option<String>, attributes: HashMap<String,String>, duration_minutes: i32 /* default 720 */, capabilities: Vec<String>, expiration_time: SystemTime }`
- `next_id: u64` (or equivalent) used to generate unique stage ids and participant ids so ARNs/participantIds are unique per creation.

Stage ARNs are the single key for lookup/update/delete; token records are removed with their stage. Pagination tokens for ListStages are derived from a stable ordering of `stages` (e.g. insertion order by `created_at`) so a `nextToken` resumes deterministically.

All other operations (compositions, encoder/storage/ingest configurations, public keys, participants/sessions/replication, tagging) remain 501 for now and will be filed as individual gap issues.

## Evidence

OPERATIONS.md at the crate root lists all 39 operations with protocol `restJson1`, sdkId `IVS RealTime`, signing name `ivs`: CreateStage (`POST /CreateStage`, output `stage` + `participantTokens`), GetStage (`POST /GetStage`, `@readonly`, input `arn` required, errors ResourceNotFoundException 404 / ValidationException 400), ListStages (`@paginated(inputToken=nextToken, outputToken=nextToken)`, output `stages` required), CreateParticipantToken (input `stageArn` required, `duration` range 1..20160, output `participantToken`), UpdateStage (output `stage`), DeleteStage (no output members, ResourceNotFoundException 404). Shape definitions from the Smithy model: `Stage{arn required, name, activeSessionId, tags, autoParticipantRecordingConfiguration, endpoints}`, `StageSummary{arn required, name, activeSessionId, tags}`, `ParticipantToken{participantId, token (sensitive), userId, attributes, duration, capabilities, expirationTime (date-time)}` whose doc states "Default: 720 (12 hours)" for duration, and `ParticipantTokenConfiguration{duration, userId, attributes, capabilities}`. A grep for `^//! SCAFFOLDED, NOT IMPLEMENTED:` under `src/ops/` matches all 39 handler files, including `create_stage.rs`, `get_stage.rs`, `list_stages.rs`, `create_participant_token.rs`, `update_stage.rs` and `delete_stage.rs` — every one still returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501). No tests exist beyond the generated `tests/state_roundtrip.rs` state-API example.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `ivs-realtime` client at it.
2. CreateStage with name="demo-stage" and one participantTokenConfigurations entry {userId: "alice", duration: 60, capabilities: ["PUBLISH"]}; expect 200 with stage.arn matching ^arn:aws:ivs:[a-z0-9-]+:[0-9]+:stage/[a-zA-Z0-9-]+$ and exactly one participantTokens entry whose userId is "alice" (today: HTTP 501).
3. GetStage with the arn from step 2; expect 200 and stage.name == "demo-stage" with the same arn and tags.
4. ListStages with maxResults=1; expect 200, the created stage present in `stages` as a StageSummary, and a nextToken only when more stages remain.
5. CreateParticipantToken with stageArn=<arn>, userId="bob", no duration; expect 200 with a fresh unique participantId, a non-empty token, userId "bob", duration 720 and an ISO-8601 expirationTime ~720 minutes in the future.
6. CreateParticipantToken with a well-formed but nonexistent stage ARN; expect ResourceNotFoundException (HTTP 404), not 501/500.
7. UpdateStage with the arn and name="demo-stage-renamed"; expect 200 with the updated name, then GetStage returns "demo-stage-renamed".
8. DeleteStage with the arn; expect 200 and an empty body, then GetStage returns ResourceNotFoundException (HTTP 404) and ListStages no longer includes the ARN.
9. GetStage with arn="not-an-arn"; expect ValidationException (HTTP 400).
