Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the Bedrock Agent Runtime emulator

_Filed: 2026-10-21T21:18:52Z_
_Operation: service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice | Class: stub_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler under `src/ops/` is an unmodified scaffold answering HTTP 501 `NotImplemented` and holding zero state, so no client workflow works. Implement a FIRST, SMALL stateful slice around the service's primary persistent resource: the **session** (sessions / invocations / invocation steps). Agent and knowledge-base invocation (InvokeAgent, Retrieve, RetrieveAndGenerate, flow executions, tagging) stay stubbed and are filed later as individual gap issues.

Happy path to implement, in order (fields per OPERATIONS.md):

1. **CreateSession** — `PUT /sessions/ -> 201`, @idempotent. Input: `sessionMetadata` (map<string,string>, 0..50), `encryptionKeyArn`, `tags`. Must mint a fresh UUID `sessionId` matching `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`, build `sessionArn` = `arn:aws:bedrock:<region>:<account>:session/<sessionId>`, persist the record, and return `sessionId`, `sessionArn`, `sessionStatus=ACTIVE`, `createdAt` (date-time).

2. **CreateInvocation** — `PUT /sessions/{sessionIdentifier}/invocations/ -> 201`. Input: `sessionIdentifier` (@httpLabel; accept BOTH the bare session UUID and the session ARN), optional `invocationId` (use it when supplied, otherwise mint a UUID), optional `description`. Persist the invocation under the session; return `sessionId`, `invocationId`, `createdAt`. Unknown session => `ResourceNotFoundException` (404).

3. **PutInvocationStep** — `PUT /sessions/{sessionIdentifier}/invocationSteps/ -> 201`. Input: `sessionIdentifier` (@httpLabel), `invocationIdentifier` (required), `invocationStepTime` (timestamp, required), `payload` (`InvocationStepPayload` union with `contentBlocks`, required), optional `invocationStepId` (overwrite that step when it already exists, else mint a UUID). Store the FULL payload verbatim; return `invocationStepId`. Unknown session or invocation => 404.

4. **GetInvocationStep** — `POST /sessions/{sessionIdentifier}/invocationSteps/{invocationStepId} -> 200`. Input: `sessionIdentifier`, `invocationStepId` (@httpLabel), `invocationIdentifier` (required). Must return the stored `invocationStep` = `InvocationStep{sessionId, invocationId, invocationStepId, invocationStepTime, payload}` — payload byte-identical to what PutInvocationStep stored, not an echo of the request. Unknown ids => 404.

5. **ListInvocationSteps** — `POST /sessions/{sessionIdentifier}/invocationSteps/ -> 200`, @paginated(inputToken=nextToken, outputToken=nextToken, items=invocationStepSummaries). Input: `sessionIdentifier`, optional `invocationIdentifier` (filter to one invocation when present, otherwise all steps in the session), `maxResults` (1..1000, default 10), `nextToken`. Return `invocationStepSummaries` of `InvocationStepSummary{sessionId, invocationId, invocationStepId, invocationStepTime}` built from state, honour `maxResults`, and emit a `nextToken` only when more remain.

6. **GetSession** — `GET /sessions/{sessionIdentifier}/ -> 200`. Returns `sessionId`, `sessionArn`, `sessionStatus`, `createdAt`, `lastUpdatedAt`, `sessionMetadata`, `encryptionKeyArn` from the stored record (metadata must be exactly what CreateSession/UpdateSession stored). Unknown session => 404.

7. **DeleteSession** — `DELETE /sessions/{sessionIdentifier}/ -> 200`, empty body. Removes the session and its invocations/steps; a following GetSession must return `ResourceNotFoundException` (404), and so must GetInvocationStep for its steps. Per AWS semantics a session must be ENDED before it can be deleted — an ACTIVE session yields `ConflictException` (409) — so **EndSession** (`PATCH /sessions/{sessionIdentifier} -> 200`, flips `sessionStatus` to `ENDED` and returns `sessionId`/`sessionArn`/`sessionStatus`) is part of this slice to make the delete reachable.

Every one of these must compute its response from persisted state: no canned values, no echoing the request back, no minting an id that is never stored. `sessionIdentifier` resolution (UUID or ARN) should be one shared helper so all session-scoped ops agree.

## State Model

In `src/state.rs`, on the shared engine state:

- `sessions: BTreeMap<String /* sessionId (uuid) */, Session>` — insertion/created order preserved for listing.
- `Session { session_id: String, session_arn: String, status: SessionStatus /* ACTIVE | ENDED | EXPIRED */, created_at: DateTime, last_updated_at: DateTime, session_metadata: BTreeMap<String,String>, encryption_key_arn: Option<String>, tags: BTreeMap<String,String>, invocations: BTreeMap<String /* invocationId */, Invocation> }`
- `Invocation { invocation_id: String, session_id: String, description: Option<String>, created_at: DateTime, steps: BTreeMap<String /* invocationStepId */, InvocationStepRecord> }`
- `InvocationStepRecord { invocation_step_id: String, invocation_id: String, session_id: String, invocation_step_time: DateTime, payload: InvocationStepPayload /* the modeled union, stored whole */ }`
- Pagination: `nextToken` an opaque (e.g. base64) cursor over the ordered key list for ListInvocationSteps (and, when ListSessions/ListInvocations land, the same helper).

Done = an integration test that runs CreateSession -> CreateInvocation -> PutInvocationStep -> GetInvocationStep -> ListInvocationSteps -> GetSession -> EndSession -> DeleteSession -> GetSession(404) end-to-end, asserting response BODIES (ids consistent across calls, payload round-trips, status transitions, 404 after delete), not just 2xx.

## Evidence

OPERATIONS.md (crate root, rendered from smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json): restJson1, sdkId `Bedrock Agent Runtime`, 35 modeled operations. It documents CreateSession (`PUT /sessions/ -> 201`; out: sessionId/sessionArn/sessionStatus/createdAt), CreateInvocation (`PUT /sessions/{sessionIdentifier}/invocations/ -> 201`; out: sessionId/invocationId/createdAt), PutInvocationStep (`PUT /sessions/{sessionIdentifier}/invocationSteps/ -> 201`; in: invocationIdentifier/invocationStepTime/payload; out: invocationStepId), GetInvocationStep (`POST /sessions/{sessionIdentifier}/invocationSteps/{invocationStepId} -> 200`; out: invocationStep), ListInvocationSteps (`POST .../invocationSteps/ -> 200`, @paginated items=invocationStepSummaries, maxResults 1..1000 default 10), GetSession (`GET /sessions/{sessionIdentifier}/ -> 200`), EndSession (`PATCH /sessions/{sessionIdentifier} -> 200`) and DeleteSession (`DELETE /sessions/{sessionIdentifier}/ -> 200`), each declaring ResourceNotFoundException (404)/ConflictException (409)/ValidationException (400). smithy_query confirms the structures InvocationStep{sessionId,invocationId,invocationStepId,invocationStepTime,payload}, InvocationStepSummary{sessionId,invocationId,invocationStepId,invocationStepTime} and InvocationStepPayload (union, member contentBlocks). Scaffold proof: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 29 wired handler files, including create_session.rs, create_invocation.rs, put_invocation_step.rs, get_invocation_step.rs, list_invocation_steps.rs, get_session.rs, end_session.rs and delete_session.rs — every one returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501). No tests exist beyond the generated tests/state_roundtrip.rs.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `bedrock-agent-runtime` at it.
2. CreateSession with sessionMetadata {"topic":"demo"} — expect 201 with a UUID sessionId, a session/<sessionId> ARN, sessionStatus=ACTIVE and createdAt; actual: HTTP 501 NotImplemented.
3. CreateInvocation with sessionIdentifier=<sessionId> and description="turn-1" — expect 201 with the same sessionId and a UUID invocationId.
4. PutInvocationStep with sessionIdentifier=<sessionId>, invocationIdentifier=<invocationId>, invocationStepTime=now, payload={contentBlocks:[{text:"hello"}]} — expect 201 with invocationStepId.
5. GetInvocationStep with the same three ids — expect 200 and invocationStep whose payload equals the stored contentBlocks and whose sessionId/invocationId/invocationStepId match.
6. ListInvocationSteps with sessionIdentifier=<sessionId>, invocationIdentifier=<invocationId>, maxResults=1 — expect the step summary listed from state (and no nextToken when it is the only step).
7. GetSession with sessionIdentifier=<session ARN> — expect 200 with sessionMetadata {"topic":"demo"} and lastUpdatedAt present (ARN and bare-UUID identifiers must both resolve).
8. DeleteSession on the still-ACTIVE session — expect ConflictException (409); then EndSession — expect 200 with sessionStatus=ENDED; then DeleteSession — expect 200.
9. GetSession again — expect ResourceNotFoundException (404), and GetInvocationStep for the deleted step likewise 404.
