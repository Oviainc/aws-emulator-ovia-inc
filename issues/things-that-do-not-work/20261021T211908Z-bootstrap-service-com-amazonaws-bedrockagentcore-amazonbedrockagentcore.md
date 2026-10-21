Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the Bedrock AgentCore emulator

_Filed: 2026-10-21T21:19:08Z_
_Operation: service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 67 operations in `services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all.

This issue defines the FIRST, SMALL slice: the AgentCore Memory *event* lifecycle, which is the primary data-plane resource of this service (`restJson1`, signing name `bedrock-agentcore`). Implement these 5 operations statefully, in this order; everything else stays 501 and is filed separately later.

1. **CreateEvent** — `POST /memories/{memoryId}/events` -> 201. Input: `memoryId` (@httpLabel, required), `actorId` (required), `sessionId`, `eventTimestamp` (timestamp, required), `payload` (list<PayloadType>, max 100, required), `branch`, `metadata` (map, max 15), `clientToken` (@idempotencyToken). Output: `event` (Event, required) with ALL required members populated from what was stored: `memoryId`, `actorId`, `sessionId`, `eventId`, `eventTimestamp`, `payload` (+ `branch`/`metadata` when supplied). `eventId` must be server-generated and match `^[0-9]+#[a-fA-F0-9]+$` (AWS uses `<epoch-millis>#<hex>`); it must NOT be echoed from input. Because the operation is `@idempotent`, a repeat with the same `clientToken` must return the SAME `eventId`, not create a second event. When `sessionId` is omitted, generate one matching `^[a-zA-Z0-9][a-zA-Z0-9-_]*$` and return it on the event.
2. **ListSessions** — `POST /memories/{memoryId}/actor/{actorId}/sessions`. Output `sessionSummaries` (list<SessionSummary>{sessionId, actorId, createdAt}) must contain exactly the sessions implied by the events created in step 1, with `createdAt` = the first event's timestamp for that session. Honour `maxResults` (1..100, default 100) and round-trip `nextToken` (@paginated inputToken=nextToken/outputToken=nextToken/items=sessionSummaries); return no `nextToken` on the last page.
3. **ListEvents** — `POST /memories/{memoryId}/actor/{actorId}/sessions/{sessionId}`. Output `events` (list<Event>, required) must list the stored events for that (memoryId, actorId, sessionId), newest-first. `includePayloads` (default true) must be honoured: when false, omit each event's `payload`. Honour `maxResults` + `nextToken` as above.
4. **GetEvent** — `GET /memories/{memoryId}/actor/{actorId}/sessions/{sessionId}/events/{eventId}`. Must return the exact `Event` stored by CreateEvent (byte-equal payload and metadata), and `ResourceNotFoundException` (HTTP 404) for an unknown `memoryId`/`actorId`/`sessionId`/`eventId`.
5. **DeleteEvent** — `DELETE /memories/{memoryId}/actor/{actorId}/sessions/{sessionId}/events/{eventId}`. Must remove the event from state and return `eventId`. A subsequent GetEvent must be `ResourceNotFoundException` (404) and the event must disappear from ListEvents. Deleting an already-deleted event is 404.

Rules for the whole slice:
- NEVER a stub and never a hollow 200: each response must be computed from persisted state held in the shared engine (`crate::state::service_state(&*self.engine)`), not canned or echoed. A create followed by a read must return exactly what was created.
- Unknown `memoryId` on any of the reads/deletes => `ResourceNotFoundException` (404). Memory resources themselves are created by the CONTROL plane (a separate service), so this data plane should treat the first CreateEvent for a syntactically valid `memoryId` as registering that memory container, and reject everything else as 404.
- Input shape violations (bad `eventId` pattern, >100 payload items, >15 metadata entries, `maxResults` outside 1..100) must produce `ValidationException` (400) / `InvalidInputException` (400) as modeled, not a 500 or a silent success.

## State Model

In `src/state.rs`, keyed off the single shared engine:

- `memories: HashMap<String /* memoryId */, MemoryStore>`
- `MemoryStore { actors: HashMap<String /* actorId */, ActorStore>, idempotency: HashMap<String /* clientToken */, String /* eventId */> }`
- `ActorStore { sessions: HashMap<String /* sessionId */, SessionStore> }`
- `SessionStore { session_id: String, actor_id: String, created_at: SystemTime, events: Vec<StoredEvent> /* append-ordered; iterate reversed for newest-first */ }`
- `StoredEvent { event_id: String /* "<epoch-millis>#<hex>" */, memory_id: String, actor_id: String, session_id: String, event_timestamp: SystemTime, payload: Vec<PayloadType>, branch: Option<Branch>, metadata: Option<HashMap<String, MetadataValue>> }`
- `event_seq: u64` — monotonic counter feeding the hex suffix of generated `eventId`s so ids are unique within a millisecond.
- Pagination: `nextToken` is an opaque encoding of (list kind, offset) so ListEvents/ListSessions resume deterministically; reject a token that does not decode with `ValidationException`.

Done = a client script can CreateEvent twice into one session, see the session via ListSessions, see both events (newest-first, payload-identical) via ListEvents, fetch one via GetEvent, DeleteEvent it, and observe it gone from both GetEvent (404) and ListEvents — with no 501 and no hardcoded values anywhere in that path.

## Evidence

OPERATIONS.md (crate root) documents the wire protocol (`restJson1`, sdkId `Bedrock AgentCore`, signing name `bedrock-agentcore`) and the exact contracts quoted above: CreateEvent `POST /memories/{memoryId}/events -> 201`, @idempotent, required `actorId`/`eventTimestamp`/`payload`, output `event` (Event, required); GetEvent `GET /memories/{memoryId}/actor/{actorId}/sessions/{sessionId}/events/{eventId}` @readonly output `event`; DeleteEvent `DELETE .../events/{eventId}` output `eventId`; ListEvents `POST /memories/{memoryId}/actor/{actorId}/sessions/{sessionId}` @paginated(items=events) with `includePayloads` default true and `maxResults` 1..100 default 100; ListSessions `POST /memories/{memoryId}/actor/{actorId}/sessions` @paginated(items=sessionSummaries). smithy_query on `Event` confirms required members memoryId/actorId/sessionId/eventId/eventTimestamp/payload with `eventId` pattern `^[0-9]+#[a-fA-F0-9]+$`, and `SessionSummary` requires sessionId/actorId/createdAt. All five handlers are unmodified scaffolds: `src/ops/create_event.rs`, `list_sessions.rs`, `list_events.rs`, `get_event.rs`, `delete_event.rs` each carry `//! SCAFFOLDED, NOT IMPLEMENTED:` at line 4 and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501); a repo-wide grep shows the same marker in all 67 op files, so there are no tests asserting any behavior here.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK client at it with service `bedrock-agentcore`.
2. CreateEvent on memoryId `demo-memory-0123456789` with actorId `actor-1`, sessionId `sess1`, eventTimestamp now, payload `[{conversational:{role:USER, content:{text:'hello'}}}]` — expect 201 with event.eventId matching ^[0-9]+#[a-fA-F0-9]+$. Today: HTTP 501 NotImplemented.
3. CreateEvent again on the same memory/actor/session with an assistant-role payload — expect a second, distinct eventId.
4. Replay the first CreateEvent with the identical clientToken — expect the SAME eventId returned and still only two events stored.
5. ListSessions(memoryId, actorId=actor-1) — expect one sessionSummary {sessionId: sess1, actorId: actor-1, createdAt: first event timestamp}.
6. ListEvents(memoryId, actorId=actor-1, sessionId=sess1) — expect both events, newest first, with payloads byte-identical to what was sent; repeat with includePayloads=false and expect payloads omitted.
7. GetEvent(memoryId, actor-1, sess1, <first eventId>) — expect the stored event exactly as created.
8. DeleteEvent on that eventId — expect 200 with the eventId echoed; then GetEvent on it expects ResourceNotFoundException (404) and ListEvents returns only the remaining event.
9. GetEvent on memoryId `no-such-memory-0123456789` — expect ResourceNotFoundException (404), not 501 or 500.
