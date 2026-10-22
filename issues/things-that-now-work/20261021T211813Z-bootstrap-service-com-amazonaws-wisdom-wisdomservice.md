Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the Wisdom (Amazon Connect Wisdom) emulator

_Filed: 2026-10-21T21:18:13Z_
_Operation: service-com-amazonaws-wisdom-wisdomservice | Class: stub_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 41 operations in `services/service-com-amazonaws-wisdom-wisdomservice/src/ops/` are unmodified scaffolds: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented`, and the crate holds no state. Implement a FIRST, SMALL, stateful slice: the assistant lifecycle plus a session, which is the core Wisdom loop a Connect client drives.

Happy path to implement (restJson1, sigv4 signing name `wisdom`), in order:

1. **CreateAssistant** — `POST /assistants`. Input: `name` (required, 1..255, `^[a-zA-Z0-9\s_.,-]+`), `type` (required, enum AGENT), `description`, `tags`, `serverSideEncryptionConfiguration`, `clientToken` (@idempotencyToken). Must mint a fresh UUID `assistantId` matching `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$` and an `assistantArn` of the form `arn:aws:wisdom:<region>:<account>:assistant/<assistantId>`, persist the record, and return `assistant` (AssistantData) with all @required members populated: `assistantId`, `assistantArn`, `name`, `type`, `status` (ACTIVE), plus the echoed-and-STORED `description`/`tags`/`serverSideEncryptionConfiguration`.
2. **GetAssistant** — `GET /assistants/{assistantId}`. Must look the assistant up in state by id OR by ARN (the label pattern accepts both) and return exactly what CreateAssistant stored; unknown id -> `ResourceNotFoundException` (HTTP 404), malformed id -> `ValidationException` (HTTP 400).
3. **ListAssistants** — `GET /assistants`. Must enumerate the persisted assistants as `assistantSummaries` (AssistantSummary: `assistantId`, `assistantArn`, `name`, `type`, `status`, `description`, `tags`), honour `maxResults` (1..100) and return/accept `nextToken` (1..2048) so a second page continues where the first stopped. An assistant created in step 1 must appear; one deleted in step 6 must not.
4. **CreateSession** — `POST /assistants/{assistantId}/sessions`. Input: `name` (required), `description`, `tags`, `clientToken`. Must 404 (`ResourceNotFoundException`) when the parent assistant does not exist, otherwise mint a UUID `sessionId` and `sessionArn` (`.../assistant/<assistantId>/session/<sessionId>`), store the session under its parent assistant, and return `session` (SessionData: `sessionId`, `sessionArn`, `name`, `description`, `tags`).
5. **GetSession** — `GET /assistants/{assistantId}/sessions/{sessionId}`. Must return the stored SessionData for that (assistantId, sessionId) pair, 404 if either is unknown or if the session belongs to a different assistant.
6. **DeleteAssistant** — `DELETE /assistants/{assistantId}` -> HTTP 204, empty body. Must remove the assistant (and its sessions) from state so a subsequent GetAssistant/GetSession returns `ResourceNotFoundException` and ListAssistants no longer lists it. It is @idempotent in the model but the real service still 404s a second delete of an id it never knew; at minimum the first delete must actually mutate state.

Every one of these six must compute its response from persisted state. No canned bodies, no echoing the input back without storing it, no random id that is never written to the store. `clientToken` replay (same token + same body) should return the originally created resource rather than a duplicate.

## State Model

In `src/state.rs`, on the service's lockable state struct:

- `assistants: HashMap<String /* assistantId (uuid) */, Assistant>` where
  `Assistant { assistant_id: String, assistant_arn: String, name: String, assistant_type: AssistantType /* AGENT */, status: AssistantStatus /* ACTIVE */, description: Option<String>, tags: BTreeMap<String,String>, server_side_encryption_configuration: Option<ServerSideEncryptionConfiguration>, sessions: HashMap<String /* sessionId */, Session>, created_at: SystemTime }`
- `Session { session_id: String, session_arn: String, assistant_id: String, name: String, description: Option<String>, tags: BTreeMap<String,String> }`
- `assistant_name_index: HashMap<String /* name */, String /* assistantId */>` so a duplicate `name` can raise `ConflictException` (409).
- `assistant_arn_index: HashMap<String /* arn */, String /* assistantId */>` so ARN-form path labels resolve to the same record as id-form labels.
- `idempotency: HashMap<String /* clientToken */, String /* created resource id */>` for CreateAssistant / CreateSession replay.
- `assistant_order: Vec<String>` (insertion order) to give ListAssistants a stable page order for `nextToken` paging.

Out of scope for this first slice (stay 501, filed later as individual gap issues): knowledge bases, contents and the content-upload/import-job flow, quick responses, SearchContent / SearchSessions / SearchQuickResponses, QueryAssistant, GetRecommendations, NotifyRecommendationsReceived, assistant associations, template URIs, and the tagging operations.

## Evidence

`services/service-com-amazonaws-wisdom-wisdomservice/OPERATIONS.md` documents all 41 operations with the routes, required members and errors cited above (CreateAssistant lines 49-55, GetAssistant 57-63, DeleteAssistant 65-71, ListAssistants 73-79, CreateSession 145-151, GetSession 153-159), and the README states every generated stub answers HTTP 501 until written and that state must be reached through `crate::state::service_state(&*self.engine)`. `grep -l '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-wisdom-wisdomservice/src/ops/` matches all 41 handler files, including `create_assistant.rs`, `get_assistant.rs`, `list_assistants.rs`, `create_session.rs`, `get_session.rs` and `delete_assistant.rs`. The @required output members (`assistantId`, `assistantArn`, `name`, `type`, `status`; `sessionId`, `sessionArn`, `name`) come from the Smithy shapes `AssistantData`, `AssistantSummary` and `SessionData` in `smithy-model/com-amazonaws-wisdom-wisdomservice.json`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `wisdom` client at it.
2. CreateAssistant(name="demo-assistant", type="AGENT", description="bootstrap demo", tags={"env":"test"}) — expect 200 with assistant.assistantId (uuid), assistant.assistantArn, status=ACTIVE; today: HTTP 501 NotImplemented.
3. GetAssistant(assistantId=<id from step 2>) — expect the same name, type, description and tags that were created; then repeat with the returned assistantArn as the label and expect the identical record.
4. ListAssistants(maxResults=1) — expect the new assistant in assistantSummaries with assistantArn and status populated, and a nextToken only when more assistants exist.
5. CreateSession(assistantId=<id>, name="demo-session") — expect 200 with session.sessionId and session.sessionArn containing the assistant id.
6. GetSession(assistantId=<id>, sessionId=<sid>) — expect the stored name/description/tags; call it with a random uuid sessionId and expect ResourceNotFoundException (HTTP 404).
7. CreateSession against a random non-existent assistantId — expect ResourceNotFoundException (HTTP 404), not 501 and not a success.
8. DeleteAssistant(assistantId=<id>) — expect HTTP 204 with an empty body.
9. GetAssistant(assistantId=<id>) again — expect ResourceNotFoundException (HTTP 404); ListAssistants must no longer include it, and GetSession for its session must also 404.
