Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the qbusiness-expertq emulator

_Filed: 2026-10-21T21:28:19Z_
_Operation: service-com-amazonaws-qbusiness-expertq | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 83 operations in `services/service-com-amazonaws-qbusiness-expertq/src/ops/` is an untouched scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, and the crate holds zero state. No client workflow works at all.

Implement a FIRST, SMALL happy-path slice: the lifecycle of the service's primary resource, the Amazon Q Business **application**, plus its child **index** (restJson1, sdkId `QBusiness`, signing name `qbusiness`). Six operations, in this order:

1. **CreateApplication** — `POST /applications`. Input: `displayName` (required, len 1..1000, pattern `^[a-zA-Z0-9][a-zA-Z0-9_-]*$`), `description`, `roleArn`, `identityType`, `identityCenterInstanceArn`, `tags`, `clientToken` (@idempotencyToken), plus the config structs (`attachmentsConfiguration`, `qAppsConfiguration`, `personalizationConfiguration`). Must MINT and STORE a new `applicationId` matching `^[a-zA-Z0-9][a-zA-Z0-9-]{35}$` (36 chars) and an `applicationArn` of the form `arn:aws:qbusiness:<region>:<account>:application/<applicationId>`, record `createdAt`/`updatedAt` and `status = ACTIVE`, and return `{applicationId, applicationArn}` read back from what it stored.
2. **GetApplication** — `GET /applications/{applicationId}`. Must return the stored record: `applicationId`, `applicationArn`, `displayName`, `description`, `identityType`, `roleArn`, `status`, `createdAt`, `updatedAt`, `attachmentsConfiguration`, `qAppsConfiguration`, `personalizationConfiguration`. Unknown id ⇒ `ResourceNotFoundException` (HTTP 404), NOT 501 and not a zeroed 200.
3. **ListApplications** — `GET /applications`, @paginated(inputToken=`nextToken`, outputToken=`nextToken`, items=`applications`). Must return one `Application` entry per stored application (`applicationId`, `displayName`, `createdAt`, `updatedAt`, `status`), honour `maxResults` (1..100) by truncating and emitting an opaque `nextToken` that resumes exactly where the page ended.
4. **CreateIndex** — `POST /applications/{applicationId}/indices`. Input: `displayName` (required), `description`, `type` (`ENTERPRISE`|`STARTER`), `capacityConfiguration`, `tags`, `clientToken`. Must 404 (`ResourceNotFoundException`) when `applicationId` is unknown; otherwise mint/store `indexId` (36-char pattern) under that application and return `{indexId, indexArn}`.
5. **ListIndices** — `GET /applications/{applicationId}/indices`, @paginated(items=`indices`). Must list only the indices stored under that application (and 404 for an unknown application), honouring `maxResults` (1..100) and `nextToken`. **GetIndex** (`GET /applications/{applicationId}/indices/{indexId}`) should return the stored index with `status = ACTIVE`, `type`, `createdAt`, `updatedAt`.
6. **DeleteApplication** — `DELETE /applications/{applicationId}`. Must remove the application AND its child indices from the store, return an empty 200 body, and make the subsequent `GetApplication`/`ListApplications` observe it gone (404 / absent from the list). Deleting an unknown id ⇒ `ResourceNotFoundException` (404).

Every one of these must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)` — no canned bodies, no echoing the input back, no minting an id that is never stored. A create followed by a get/list must return exactly what was created, and ids must be server-generated and pattern-valid so the SDK's own output constraints are satisfied.

## State Model

In `src/state.rs` (`ComAmazonawsQbusinessExpertqState`):

- `applications: HashMap<String /*applicationId*/, Application>` where
  `Application { application_id: String, application_arn: String, display_name: String, description: Option<String>, identity_type: Option<IdentityType>, role_arn: Option<String>, identity_center_instance_arn: Option<String>, status: ApplicationStatus /*ACTIVE*/, created_at: SystemTime, updated_at: SystemTime, attachments_configuration: Option<...>, q_apps_configuration: Option<...>, personalization_configuration: Option<...>, tags: Vec<Tag>, indices: HashMap<String /*indexId*/, Index> }`
- `Index { index_id: String, index_arn: String, application_id: String, display_name: String, description: Option<String>, index_type: Option<IndexType>, status: IndexStatus /*ACTIVE*/, capacity_configuration: Option<...>, document_attribute_configurations: Vec<...>, created_at: SystemTime, updated_at: SystemTime, tags: Vec<Tag> }`
- `creation_order: Vec<String>` (or an insertion-ordered map) so ListApplications/ListIndices paginate deterministically and `nextToken` is a stable opaque cursor (e.g. base64 of the last returned id / offset).
- optional `client_tokens: HashMap<String /*clientToken*/, String /*resource id*/>` so the @idempotent creates replay the first result instead of minting a duplicate.

Everything else in the service (Chat/ChatSync, retrievers, plugins, web experiences, data sources/accessors, documents, groups, users, subscriptions, tagging, chat controls) stays 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-qbusiness-expertq/OPERATIONS.md` (generated from `smithy-model/com-amazonaws-qbusiness-expertq.json`) documents: protocol `restJson1`, sdkId `QBusiness`; CreateApplication `POST /applications` → `applicationId`/`applicationArn`; GetApplication `GET /applications/{applicationId}` @readonly with the full record incl. `status` (ApplicationStatus ACTIVE|CREATING|DELETING|FAILED|UPDATING), `createdAt`, `updatedAt`, errors incl. `ResourceNotFoundException (HTTP 404)`; ListApplications `GET /applications` @paginated(inputToken=nextToken, outputToken=nextToken, items=applications) with `maxResults` range 1..100; CreateIndex `POST /applications/{applicationId}/indices`; GetIndex/ListIndices under `/applications/{applicationId}/indices`; DeleteApplication `DELETE /applications/{applicationId}` with no output members. Stub proof: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-qbusiness-expertq/src/ops/` matches every op file, and `src/ops/create_application.rs:224` is `Err(crate::scaffolded::Scaffolded::not_implemented("CreateApplication"))`. No tests assert any behaviour (only the generated `tests/state_roundtrip.rs`).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `qbusiness` client at it.
2. `CreateApplication(displayName="demo-app")` → expect 200 with a 36-char `applicationId` matching ^[a-zA-Z0-9][a-zA-Z0-9-]{35}$ and an `applicationArn`; today: HTTP 501 NotImplemented.
3. `GetApplication(applicationId=<from step 2>)` → expect 200 with `displayName == "demo-app"`, `status == ACTIVE`, populated `createdAt`.
4. `ListApplications()` → expect the new application present in `applications`; call again with `maxResults=1` after creating a second app and verify `nextToken` returns the remaining app and no duplicates.
5. `CreateIndex(applicationId=<id>, displayName="demo-index", type="STARTER")` → expect 200 with a 36-char `indexId`; then `ListIndices(applicationId=<id>)` and `GetIndex(applicationId, indexId)` must return that index with `status == ACTIVE`.
6. `CreateIndex(applicationId="000000000000000000000000000000000000", displayName="x")` → expect `ResourceNotFoundException` 404.
7. `DeleteApplication(applicationId=<id>)` → expect 200; then `GetApplication(<id>)` → 404 `ResourceNotFoundException` and `ListApplications()` no longer contains it.
