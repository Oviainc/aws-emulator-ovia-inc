Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 16fab4a727e8e34e48fe5cd80cb0080f7932bd9f

# Bootstrap the com-amazonaws-qconnect-wisdomservice (Amazon Q Connect / Wisdom) emulator

_Filed: 2026-10-21T23:18:33Z_
_Operation: service-com-amazonaws-qconnect-wisdomservice | Class: missing_

> _Note: this issue was identified against the code base at commit `16fab4a727e8e34e48fe5cd80cb0080f7932bd9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 94 operations in `services/service-com-amazonaws-qconnect-wisdomservice/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. This issue defines the FIRST buildable slice: the assistant lifecycle plus a session created inside it, which is the core end-to-end loop of Amazon Q in Connect (restJson1, sigv4 signing name `wisdom`).

Implement these 6 operations, in this order, each computing its response from persisted state:

1. **CreateAssistant** — `POST /assistants`. Input: `name` (required, len 1..255, pattern `^[a-zA-Z0-9\s_.,-]+`), `type` (required, enum, only `AGENT`), `description`, `tags`, `serverSideEncryptionConfiguration`, `clientToken` (@idempotencyToken). Mint a fresh lowercase-hex UUID `assistantId` matching `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$` and an `assistantArn` of the form `arn:aws:wisdom:<region>:<account12>:assistant/<assistantId>`, store the record, and return `assistant` (AssistantData) with the required members `assistantId`, `assistantArn`, `name`, `type`, `status` (use `ACTIVE`) plus the echoed-and-STORED `description`/`tags`/`serverSideEncryptionConfiguration`. The returned values must come from the stored record, not be synthesized per request.
2. **GetAssistant** — `GET /assistants/{assistantId}`. Accept either the bare UUID or the full ARN in the label (the model's pattern allows both). Return the SAME AssistantData that CreateAssistant stored (same id, arn, name, type, status, description, tags). Unknown id -> `ResourceNotFoundException` (HTTP 404).
3. **ListAssistants** — `GET /assistants`. Return `assistantSummaries` (required) built from the stored map — one AssistantSummary per live assistant with `assistantId`, `assistantArn`, `name`, `type`, `status`, `description`, `tags`. Honour `maxResults` (1..100) by truncating and emitting an opaque `nextToken` (len 1..2048) that a follow-up call resumes from; never return a token when the page is the last one.
4. **CreateSession** — `POST /assistants/{assistantId}/sessions`. Input: `assistantId` (@httpLabel, required), `name` (required), `description`, `tags`, `tagFilter`, `aiAgentConfiguration`, `contactArn`, `clientToken`. Resolve the parent assistant first — unknown parent -> `ResourceNotFoundException` (404). Mint `sessionId` (UUID) and `sessionArn` (`arn:aws:wisdom:<region>:<account12>:session/<assistantId>/<sessionId>`), store the session under its parent assistant, and return `session` (SessionData) with required `sessionArn`, `sessionId`, `name`, plus stored `description`, `tags`, `tagFilter`, `aiAgentConfiguration` and `origin` = `CUSTOMER` (the model documents `CUSTOMER` as the origin for a session created through the CreateSession API).
5. **GetSession** — `GET /assistants/{assistantId}/sessions/{sessionId}`. Look the session up under its parent assistant and return exactly the SessionData that CreateSession persisted. Unknown assistant OR unknown session -> `ResourceNotFoundException` (404). A session id that belongs to a different assistant must also 404, not be found by global id lookup.
6. **DeleteAssistant** — `DELETE /assistants/{assistantId}` -> HTTP 204 with an empty body. Remove the assistant and its sessions from the store. A subsequent GetAssistant and GetSession must then return `ResourceNotFoundException` (404), and ListAssistants must no longer include it. Deleting an id that was never created -> `ResourceNotFoundException` (404).

No canned, hardcoded or purely echoed responses, and no "mint a random id and forget it": every read above must be served out of the shared store reached via `crate::state::service_state(&*self.engine)` (per the crate README), so a create is observable by every later call in the same process.

## State Model

In `src/state.rs` (`ComAmazonawsQconnectWisdomserviceState`):

- `assistants: HashMap<String /* assistantId (UUID) */, Assistant>`
- `Assistant { assistant_id: String, assistant_arn: String, name: String, r#type: AssistantType /* AGENT */, status: AssistantStatus /* ACTIVE */, description: Option<String>, tags: HashMap<String,String>, server_side_encryption_configuration: Option<ServerSideEncryptionConfiguration>, created_at: SystemTime, sessions: HashMap<String /* sessionId */, Session> }`
- `Session { session_id: String, session_arn: String, assistant_id: String, name: String, description: Option<String>, tags: HashMap<String,String>, tag_filter: Option<TagFilter>, ai_agent_configuration: HashMap<String, AIAgentConfigurationData>, origin: Origin /* CUSTOMER */, created_at: SystemTime }`
- `assistant_name_index: HashMap<String /* name */, String /* assistantId */>` — so a repeated CreateAssistant with the same name can raise `ConflictException` (409).
- `idempotency: HashMap<String /* clientToken */, String /* created resource id */>` — a replayed `clientToken` on CreateAssistant/CreateSession returns the originally created resource instead of a second one (both ops are `@idempotent` with an `@idempotencyToken` member).
- `list_tokens: HashMap<String /* nextToken */, usize /* offset */>` (or an offset encoded in the opaque token) to back ListAssistants pagination.

Accept both UUID and ARN forms when resolving `{assistantId}`/`{sessionId}` labels by normalizing to the trailing UUID before the map lookup.

All other 88 operations (knowledge bases, contents, message templates, quick responses, AI agents/prompts/guardrails, SendMessage/GetNextMessage/ListMessages, QueryAssistant, Retrieve, tagging, search, import jobs) stay at 501 for now and are tracked as separate gap issues.

Done = a client script can run CreateAssistant -> GetAssistant -> ListAssistants -> CreateSession -> GetSession -> DeleteAssistant -> GetAssistant(404) and every response body is correct and consistent with the state the earlier calls created.

## Evidence

`services/service-com-amazonaws-qconnect-wisdomservice/OPERATIONS.md` (generated from `smithy-model/com-amazonaws-qconnect-wisdomservice.json`, 94 operations, protocol restJson1, sigv4 name `wisdom`) documents each op in the slice: CreateAssistant `POST /assistants` with required `name`/`type` returning `assistant` (AssistantData) and errors ConflictException(409)/ValidationException(400)/ServiceQuotaExceededException(402); GetAssistant `GET /assistants/{assistantId}` returning AssistantData with ResourceNotFoundException(404); ListAssistants `GET /assistants` @paginated(inputToken=nextToken, outputToken=nextToken, items=assistantSummaries) with required `assistantSummaries` and `maxResults` range 1..100; CreateSession `POST /assistants/{assistantId}/sessions` returning `session` (SessionData); GetSession `GET /assistants/{assistantId}/sessions/{sessionId}`; DeleteAssistant `DELETE /assistants/{assistantId} -> 204`. `smithy_query` on AssistantData/AssistantSummary/SessionData gives the required members cited above (assistantId, assistantArn, name, type, status; sessionArn, sessionId, name; SessionData.origin documented as CUSTOMER for sessions created via CreateSession). All 94 files under `src/ops/` match `^//! SCAFFOLDED, NOT IMPLEMENTED:` (e.g. `src/ops/create_assistant.rs:4`, `src/ops/get_assistant.rs:4`, `src/ops/list_assistants.rs:4`, `src/ops/create_session.rs:4`, `src/ops/get_session.rs:4`, `src/ops/delete_assistant.rs:4`) and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501. No test exercises any of these; `tests/state_roundtrip.rs` is the scaffold's state-API example only.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `qconnect` client at it (restJson1, signing name `wisdom`).
2. CreateAssistant(name="demo-assistant", type="AGENT", description="bootstrap demo", tags={"env":"test"}) — expect HTTP 200 with assistant.assistantId (UUID), assistant.assistantArn, name="demo-assistant", type="AGENT", status set; today: HTTP 501 NotImplemented.
3. GetAssistant(assistantId=<id from step 2>) — expect the identical AssistantData (same arn, name, description, tags); today: 501.
4. ListAssistants(maxResults=10) — expect assistantSummaries to contain exactly the assistant created in step 2; today: 501.
5. CreateSession(assistantId=<id>, name="demo-session", description="chat") — expect 200 with session.sessionId, session.sessionArn containing both the assistant and session ids, origin=CUSTOMER; today: 501.
6. GetSession(assistantId=<id>, sessionId=<session id>) — expect the same SessionData persisted in step 5; today: 501.
7. GetSession(assistantId=<id>, sessionId="00000000-0000-0000-0000-000000000000") — expect ResourceNotFoundException with HTTP 404.
8. DeleteAssistant(assistantId=<id>) — expect HTTP 204 with an empty body; today: 501.
9. GetAssistant(assistantId=<id>) again — expect ResourceNotFoundException (HTTP 404), and ListAssistants no longer returns it.
