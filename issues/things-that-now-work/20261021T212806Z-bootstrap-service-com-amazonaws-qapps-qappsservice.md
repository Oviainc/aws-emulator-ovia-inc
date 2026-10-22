Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the qapps (Amazon Q Apps) emulator

_Filed: 2026-10-21T21:28:06Z_
_Operation: service-com-amazonaws-qapps-qappsservice | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 35 operations in `services/service-com-amazonaws-qapps-qappsservice/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented`, holding zero state. No client workflow works at all.

This issue defines the FIRST, minimal happy-path slice: the Q App lifecycle plus one session run. Implement exactly these 6 operations, in this order, statefully (restJson1 protocol, sigv4 signing name `qapps`, every operation takes the required `instance-id` HTTP header which must scope all state):

1. **CreateQApp** — `POST /apps.create`. Input: `title` (required, len ..100, pattern `^[^{}\\"<>]+$`), `description` (len ..500), `appDefinition` (AppDefinitionInput, required), `tags`. Must mint a fresh v4-shaped `appId` matching `^[\da-f]{8}-[\da-f]{4}-[45][\da-f]{3}-[89ABab][\da-f]{3}-[\da-f]{12}$`, derive `appArn`, set `appVersion` = 1, `status` = `DRAFT`, `createdAt`/`updatedAt` to now, `createdBy`/`updatedBy` to the caller principal, and PERSIST the whole record including the submitted `appDefinition` cards and `tags`. Return `appId`, `appArn`, `title`, `description`, `initialPrompt`, `appVersion`, `status`, `createdAt`, `createdBy`, `updatedAt`, `updatedBy`, `requiredCapabilities`.
2. **GetQApp** — `GET /apps.get` with `appId` (query, required) and optional `appVersion` (query). Must look the app up in state and return the SAME `title`, `description`, `appVersion`, `status`, timestamps and the required `appDefinition` that CreateQApp/UpdateQApp stored — not a canned body. Unknown `appId` (or one deleted) → `ResourceNotFoundException` (HTTP 404). A supplied `appVersion` that does not exist → 404.
3. **UpdateQApp** — `POST /apps.update` with `appId` (required) and optional `title`, `description`, `appDefinition`. Must apply ONLY the supplied members to the stored record (absent members unchanged), bump `appVersion` by 1, refresh `updatedAt`/`updatedBy`, and return the updated record. A following GetQApp must reflect the new title/definition and the new `appVersion`.
4. **ListQApps** — `GET /apps.list` with optional `limit` (1..100) and `nextToken` (len ..300). Must enumerate the apps persisted for this `instance-id` as `apps` (list<UserAppItem>, required) — created apps appear, deleted apps do not. Must honour `limit` as a page size and return an opaque `nextToken` when more remain; passing that token must continue from where the previous page stopped without repeating or dropping an app, and must be absent on the last page.
5. **StartQAppSession** — `POST /runtime.startQAppSession` with `appId` (required), `appVersion` (required), optional `initialValues` (list<CardValue>, len ..20), `sessionId`, `tags`. Must verify the app and version exist (else `ResourceNotFoundException` 404), mint/accept a `sessionId`, derive `sessionArn`, persist a session record whose `cardStatus` map is keyed by the card ids of the app's stored `appDefinition` and seeded from `initialValues`, and return `sessionId` + `sessionArn`.
6. **GetQAppSession** — `GET /runtime.getQAppSession` with `sessionId` (query, required). Must return the persisted session: `sessionId`, `sessionArn`, `sessionName`, `appVersion`, `latestPublishedAppVersion`, `status` (ExecutionStatus: `COMPLETED` | `ERROR` | `IN_PROGRESS` | `WAITING`, required) and the `cardStatus` map (required) built from state — the values passed to StartQAppSession must be visible here. Unknown `sessionId` → `ResourceNotFoundException` 404.
7. **DeleteQApp** — `POST /apps.delete` with `appId` (required), `@idempotent`. Must remove/tombstone the app so a subsequent GetQApp returns `ResourceNotFoundException` 404 and ListQApps omits it. Deleting an unknown `appId` → 404.

NEVER STUBS: each of these must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)` (the shared engine injected into the handler — never a fresh `InMemoryEngine`). Hardcoded bodies, echoing the input back without storing it, or minting an id without persisting a record are failures of this issue. Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line only on the operations actually implemented.

## State Model

In `src/state.rs`, on `ComAmazonawsQappsQappsserviceState`:

- `apps: HashMap<(String /*instanceId*/, String /*appId*/), QApp>` where
  `QApp { app_id: String, app_arn: String, title: String, description: Option<String>, initial_prompt: Option<String>, app_version: i32, status: AppStatus /*DRAFT|PUBLISHED|DELETED*/, created_at: DateTime, created_by: String, updated_at: DateTime, updated_by: String, required_capabilities: Vec<AppRequiredCapability>, app_definition: AppDefinition /*cards as submitted*/, versions: BTreeMap<i32, AppDefinition>, tags: HashMap<String,String> }`
- `app_order: Vec<(String /*instanceId*/, String /*appId*/)>` — stable creation order so ListQApps pagination is deterministic.
- `sessions: HashMap<(String /*instanceId*/, String /*sessionId*/), QAppSession>` where
  `QAppSession { session_id: String, session_arn: String, session_name: Option<String>, app_id: String, app_version: i32, status: ExecutionStatus, card_status: HashMap<String /*cardId*/, CardStatus>, card_values: Vec<CardValue>, sharing_configuration: SessionSharingConfiguration, owner: String, tags: HashMap<String,String>, created_at: DateTime }`
- `sessions_by_app: HashMap<(String, String /*appId*/), Vec<String /*sessionId*/>>` — so an app delete can clean up its sessions.

Pagination tokens for ListQApps should be an opaque encoding of a position in `app_order` (e.g. index or last-seen appId), validated on input (malformed token → `ValidationException` 400).

All other 28 operations (library/catalog items, categories, permissions, tagging, PredictQApp, ImportDocument, CreatePresignedUrl, session metadata/data/export, StopQAppSession, Associate/Disassociate*) stay scaffolded at 501 for now and will be filed as individual gap issues later.

## Evidence

Spec: `services/service-com-amazonaws-qapps-qappsservice/smithy-model/com-amazonaws-qapps-qappsservice.json`, denormalized in the crate's `OPERATIONS.md` — CreateQApp (`POST /apps.create`, OPERATIONS.md lines 75-80) requires `title`/`appDefinition` and returns required `appId`/`appArn`/`appVersion`/`status`/`createdAt`/`createdBy`/`updatedAt`/`updatedBy`; GetQApp (`GET /apps.get`, lines 135-141, `@readonly`) returns a REQUIRED `appDefinition` plus the same metadata and declares `ResourceNotFoundException (HTTP 404)`; UpdateQApp (lines 258-263); ListQApps (`GET /apps.list`, lines 182-188) is `@paginated(inputToken=nextToken, outputToken=nextToken, items=apps)` with `limit` range 1..100; StartQAppSession (lines 213-218) requires `appId`+`appVersion` and returns `sessionId`/`sessionArn`; GetQAppSession (lines 143-149) returns a required `cardStatus` map and required `status` of enum ExecutionStatus; DeleteQApp (lines 90-96) is `@idempotent` and declares `ResourceNotFoundException`. Protocol/identity from OPERATIONS.md lines 8-14 (restJson1, sdk id QApps, sigv4 name qapps) and every operation's required `instance-id` `@httpHeader`.

Implementation: `grep -rn '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-qapps-qappsservice/src/ops/` matches all 35 handler files, including `create_q_app.rs`, `get_q_app.rs`, `update_q_app.rs`, `list_q_apps.rs`, `start_q_app_session.rs`, `get_q_app_session.rs` and `delete_q_app.rs` — each returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501. The crate README (lines 36-53) documents that this marker is exactly the still-stubbed census. No test asserts any stateful behavior; `tests/` holds only the generated `state_roundtrip.rs` state-API example.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `qapps` client at it with `instance-id` header `11111111-2222-4333-8444-555555555555`.
2. CreateQApp(title="Trip Planner", description="demo", appDefinition={cards:[...]}) → expect 200 with a fresh v4-shaped appId, appArn, appVersion=1, status=DRAFT, createdAt/createdBy/updatedAt/updatedBy populated. Today: HTTP 501 NotImplemented.
3. GetQApp(appId=<from step 2>) → expect 200 echoing the SAME title "Trip Planner", description, appVersion=1, status=DRAFT and the appDefinition cards submitted in step 2 (not a canned/empty definition).
4. UpdateQApp(appId=<id>, title="Trip Planner v2") → expect 200 with appVersion=2 and refreshed updatedAt; then GetQApp(appId=<id>) → title is "Trip Planner v2", appVersion=2, description unchanged.
5. ListQApps(limit=1) after creating a second app → expect exactly 1 entry in `apps` plus a nextToken; ListQApps(limit=1, nextToken=<token>) → the OTHER app and no nextToken. No app repeated or missing.
6. StartQAppSession(appId=<id>, appVersion=2, initialValues=[{cardId:<card from the definition>, value:"Paris"}]) → expect 200 with sessionId + sessionArn.
7. GetQAppSession(sessionId=<from step 6>) → expect 200 with that sessionId, matching sessionArn, appVersion=2, a status from ExecutionStatus, and a cardStatus map keyed by the app's card ids reflecting the "Paris" initial value.
8. StartQAppSession(appId="00000000-0000-4000-8000-000000000000", appVersion=1) → expect ResourceNotFoundException with HTTP 404, not 501 and not 200.
9. DeleteQApp(appId=<id>) → expect 200/204; then GetQApp(appId=<id>) → ResourceNotFoundException HTTP 404, and ListQApps no longer lists it.
10. Repeat DeleteQApp(appId=<id>) → expect ResourceNotFoundException HTTP 404 (operation is @idempotent but the app is gone).
