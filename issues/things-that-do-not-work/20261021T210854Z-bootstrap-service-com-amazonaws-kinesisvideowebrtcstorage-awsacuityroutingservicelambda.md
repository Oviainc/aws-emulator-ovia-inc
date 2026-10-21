Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the Kinesis Video WebRTC Storage emulator

_Filed: 2026-10-21T21:08:54Z_
_Operation: service-com-amazonaws-kinesisvideowebrtcstorage-awsacuityroutingservicelambda | Class: stub_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in this crate is an unmodified scaffold: both `src/ops/join_storage_session.rs` and `src/ops/join_storage_session_as_viewer.rs` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and `src/state.rs` holds only `// TODO: add service-specific state fields`. No client can complete any call.

This service binds exactly TWO operations, so the first slice IS the whole happy path. Both must be implemented statefully, in this order:

1. **JoinStorageSession** — `POST /joinStorageSession`. Input: `channelArn` (required, must match the modeled ARN pattern). Output: empty body, HTTP 200. Semantics: joins the caller to the ongoing one-way-video / multi-way-audio WebRTC session for the channel as the *video producing device* (master). If no session exists for that channel, a new storage/streaming session must be CREATED and recorded in state. Per the modeled docs, at most ONE video producing device may be associated with a channel's session: a second `JoinStorageSession` for the same `channelArn` must REPLACE the incumbent producer ("the most recent client request takes precedence") rather than erroring or silently no-op'ing. The call is explicitly NOT idempotent — each call is a new join and bumps the session's generation/offer counter in state.
2. **JoinStorageSessionAsViewer** — `POST /joinStorageSessionAsViewer`. Input: `channelArn` (required, same ARN pattern) and `clientId` (required, length 1..256, pattern `^[a-zA-Z0-9_.-]+$`). Output: empty body, HTTP 200. Semantics: joins the caller to the SAME channel session as a playback viewer, registered under `clientId`. Must be computed against the session created by step 1 — a viewer join for a channel with no known session/channel must fail, not succeed.
3. **Read-back / verification path** — because both outputs are empty structures, "it worked" is only observable through state. The bootstrap is done when the two calls above are wired through one shared store such that the second call's outcome DEPENDS on the first: viewer join before any producer join → `ResourceNotFoundException` (404); viewer join after a successful producer join on the same `channelArn` → 200; and a repeat producer join rotates the recorded producer. A hollow `200` returned unconditionally from either handler is an explicit FAILURE of this issue, not a pass.

Error semantics that must be real, not canned (all four are modeled on BOTH operations):
- `ResourceNotFoundException` (404) — `channelArn` names a signaling channel this emulator has no session/registration for.
- `InvalidArgumentException` (400) — argument rejected on semantic grounds the schema cannot express (e.g. a syntactically valid ARN whose resource type is not a signaling channel, or a channel whose type is not `SINGLE_MASTER`). Note the modeled `@pattern`/`@length` constraints on `channelArn` and `clientId` are enforced by the framework BEFORE `handle` runs, so the handler must not re-refuse those with a 501.
- `ClientLimitExceededException` (400) and `AccessDeniedException` (403) — may stay unexercised in the first slice, but the handler must return them through `JoinStorageSessionErrorModeled::*.into()` rather than the `Scaffolded` wrapper once wired.

No hardcoded returns, no echoed inputs, no random-id-without-storage. Both handlers must reach the shared store via `crate::state::service_state(&*self.engine)` (never `engine::InMemoryEngine::new()`), and the SCAFFOLDED marker line must be deleted from each file once implemented.

## State Model

Fill in `ComAmazonawsKinesisvideowebrtcstorageAwsacuityroutingservicelambdaState` in `src/state.rs`:

- `sessions: HashMap<String, StorageSession>` — keyed by `channelArn` (the ARN string exactly as supplied).
- `StorageSession { channel_arn: String, producer: Option<ProducerSession>, viewers: HashMap<String, ViewerSession>, created_at: SystemTime, join_count: u64 }`
  - `join_count` increments on every `JoinStorageSession` call (the API is documented non-idempotent; "an offer is sent once per each call").
- `ProducerSession { joined_at: SystemTime, generation: u64 }` — `generation` is the value of `join_count` at which this producer took over, so a later join demonstrably supersedes an earlier one.
- `ViewerSession { client_id: String, joined_at: SystemTime }` — keyed by `clientId` in `StorageSession::viewers`; a repeat join with the same `clientId` refreshes `joined_at` rather than duplicating the entry.
- Optionally `known_channels: HashSet<String>` — the set of channel ARNs treated as existing signaling channels, so `ResourceNotFoundException` is a decision made from state rather than always-200. (Pre-seeding or auto-registering on first producer join are both acceptable; it must simply be consistent and observable.)

All of this lives in the single shared engine store so the two operations see the same sessions across requests.

## Evidence

Grounding:
1. `services/service-com-amazonaws-kinesisvideowebrtcstorage-awsacuityroutingservicelambda/OPERATIONS.md` — lists exactly 2 operations, protocol `restJson1`, sigv4 name `kinesisvideo`. `JoinStorageSession`: `POST /joinStorageSession -> 200`, input `channelArn` (string, pattern `^arn:(aws[a-zA-Z-]*):kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$`, required), no output members, errors `AccessDeniedException (403)`, `ClientLimitExceededException (400)`, `InvalidArgumentException (400)`, `ResourceNotFoundException (404)`. `JoinStorageSessionAsViewer`: `POST /joinStorageSessionAsViewer -> 200`, input `channelArn` (same, required) + `clientId` (string, len 1..256, pattern `^[a-zA-Z0-9_.-]+$`, required), no output members, same four errors.
2. Stub proof — `src/ops/join_storage_session.rs` lines 4-6 carry `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented`, and line 201 is `Err(crate::scaffolded::Scaffolded::not_implemented("JoinStorageSession"))`; `src/ops/join_storage_session_as_viewer.rs` is the same generated shape. `src/ops/` contains only these two op files plus `mod.rs`.
3. No-state proof — `src/state.rs` lines 6-9: the state struct body is just `// TODO: add service-specific state fields`.
4. Modeled documentation quoted in the stub header (lines 52-73 of `join_storage_session.rs`) is the source for the stateful semantics above: "If there's no existing session for the channel, a new streaming session needs to be created", "Assume that only one video producing device client can be associated with a session for the channel. If more than one client joins the session of a specific channel as a video producing device, the most recent client request takes precedence", and "Idempotent - This API is not idempotent... An offer is sent once per each call."
5. Test coverage — the crate ships only `tests/state_roundtrip.rs` (a compiled example of the state API per README.md); there is no test asserting either operation's behaviour.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK client at it with service `kinesisvideo` (WebRTC storage endpoint), region `us-east-1`.
2. Call JoinStorageSessionAsViewer with channelArn `arn:aws:kinesisvideo:us-east-1:123456789012:channel/demo-channel/1700000000000` and clientId `viewer-1` BEFORE any producer has joined. Expected: HTTP 404 `ResourceNotFoundException` (no session for that channel). Actual today: HTTP 501 `NotImplemented`.
3. Call JoinStorageSession with channelArn `arn:aws:kinesisvideo:us-east-1:123456789012:channel/demo-channel/1700000000000`. Expected: HTTP 200 with an empty JSON body, and a StorageSession created in state with a producer recorded. Actual today: HTTP 501 `NotImplemented`.
4. Re-call JoinStorageSessionAsViewer with the same channelArn and clientId `viewer-1`. Expected: HTTP 200 and a ViewerSession for `viewer-1` registered on that channel's session — i.e. the outcome changed because of step 3, proving the two operations share persisted state.
5. Call JoinStorageSessionAsViewer again with clientId `viewer-2`. Expected: HTTP 200 and TWO distinct viewers on the session, not a replaced single entry.
6. Re-call JoinStorageSession for the same channelArn. Expected: HTTP 200, the session's join_count incremented and the producer replaced by the newer join (most-recent-wins), not an error and not a no-op.
7. Call JoinStorageSession with channelArn `not-an-arn`. Expected: HTTP 400 from framework constraint validation (the modeled @pattern), never HTTP 501.
8. Call JoinStorageSessionAsViewer with a valid channelArn and clientId `bad id!` (space and `!` violate `^[a-zA-Z0-9_.-]+$`). Expected: HTTP 400, never HTTP 501 and never 200.
