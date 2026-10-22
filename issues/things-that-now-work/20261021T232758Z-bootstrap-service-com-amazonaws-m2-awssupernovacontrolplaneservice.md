Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: e44f583f966e818f9abdd86a5b0ab9a8f2f88230

# Bootstrap the AWS Mainframe Modernization (m2) emulator

_Filed: 2026-10-21T23:27:58Z_
_Operation: service-com-amazonaws-m2-awssupernovacontrolplaneservice | Class: missing_

> _Note: this issue was identified against the code base at commit `e44f583f966e818f9abdd86a5b0ab9a8f2f88230`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The whole crate is a fresh scaffold: every file under `services/service-com-amazonaws-m2-awssupernovacontrolplaneservice/src/ops/` (37 operations) carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and `src/state.rs` holds `ComAmazonawsM2AwssupernovacontrolplaneserviceState { /* TODO: add service-specific state fields */ }` — zero state. Nothing works end to end.

First slice: implement the Application lifecycle (m2's primary resource) statefully. Protocol is `restJson1`, sigv4 signing name `m2`.

1. **CreateApplication** — `POST /applications`. Input: `name` (required, pattern `^[A-Za-z0-9][A-Za-z0-9_\-]{1,59}$`), `engineType` (required, `bluage|microfocus`), `definition` (required, Definition union: inline `content` or `s3Location`), optional `description`, `tags`, `kmsKeyId`, `roleArn`, `clientToken` (@idempotencyToken). Must mint and persist a new application: generate `applicationId` (matching `^\S{1,80}$`), build `applicationArn` as `arn:aws:m2:<region>:<account>:app/<applicationId>`, set `applicationVersion` = 1, store `definition`/`description`/`tags`/`engineType` and a `creationTime`. Return all three required output members from the stored record. Duplicate `name` must fail `ConflictException` (409); a repeated `clientToken` with identical input must return the SAME applicationId/version rather than creating a second application.
2. **GetApplication** — `GET /applications/{applicationId}`. Must read the stored record and return `name`, `applicationId`, `applicationArn`, `status` (`Created`/`Available`), `latestVersion` (ApplicationVersionSummary with `applicationVersion`, `status`, `creationTime`), `engineType`, `creationTime`, plus `description`/`tags`/`kmsKeyId`/`roleArn` exactly as supplied to CreateApplication. Unknown id → `ResourceNotFoundException` (404).
3. **ListApplications** — `GET /applications`. Must return one `ApplicationSummary` per persisted application (`applicationId`, `applicationArn`, `name`, `status`, `engineType`, `creationTime`, `applicationVersion`), honour the `names` query filter (1..10 names) and the `maxResults`/`nextToken` pagination contract (@paginated inputToken=nextToken, outputToken=nextToken, items=applications): truncate at `maxResults`, emit a `nextToken` only when more remain, and resume from it.
4. **UpdateApplication** — `PATCH /applications/{applicationId}`. Input `currentApplicationVersion` (required), optional `description`, `definition`. Must increment the stored version (returning the NEW `applicationVersion`), record a new version entry with the updated definition/description, and reject a stale/mismatched `currentApplicationVersion` with `ConflictException` (409). Unknown id → `ResourceNotFoundException` (404).
5. **GetApplicationVersion** — `GET /applications/{applicationId}/versions/{applicationVersion}`. Must return the stored version's `name`, `applicationVersion`, `definitionContent` (the definition text persisted by Create/Update), `status` (`Available`), `creationTime`, `description`. Unknown id or version → `ResourceNotFoundException` (404).
6. **DeleteApplication** — `DELETE /applications/{applicationId}`. Must remove the application (and its versions) from state so a subsequent GetApplication returns 404 and ListApplications no longer lists it.

NEVER STUBS: each of these six must compute its response from persisted state. Hardcoded ARNs, echoing the input back without storing it, or returning a random applicationId that no later GetApplication can resolve are all failures of this issue.

## State Model

Add to `ComAmazonawsM2AwssupernovacontrolplaneserviceState`:

- `applications: HashMap<String /* applicationId */, Application>` where
  `Application { application_id: String, application_arn: String, name: String, description: Option<String>, engine_type: String /* bluage|microfocus */, status: String /* Created|Available|... */, kms_key_id: Option<String>, role_arn: Option<String>, tags: HashMap<String,String>, creation_time: SystemTime, last_start_time: Option<SystemTime>, environment_id: Option<String>, latest_version: u32, versions: Vec<ApplicationVersion> }`
- `ApplicationVersion { application_version: u32, definition_content: String, description: Option<String>, status: String /* Creating|Available|Failed */, creation_time: SystemTime, status_reason: Option<String> }`
- `app_names: HashMap<String /* name */, String /* applicationId */>` for the ConflictException-on-duplicate-name check.
- `client_tokens: HashMap<String /* clientToken */, String /* applicationId */>` for @idempotencyToken replay.
- Reserve (used by the next slices, not this one): `environments: HashMap<String, Environment>`, `deployments: HashMap<String /* applicationId */, Vec<Deployment>>`, `batch_job_executions: HashMap<String, Vec<BatchJobExecution>>`, `tags_by_arn: HashMap<String, HashMap<String,String>>` so TagResource/UntagResource/ListTagsForResource can share one tag store keyed by ARN.

All state lives in the single shared engine, reached via `crate::state::service_state(&*self.engine)` — never a fresh `InMemoryEngine` inside a handler.

Out of scope for this first slice (stay 501, filed separately later): CreateEnvironment/GetEnvironment/UpdateEnvironment/DeleteEnvironment/ListEnvironments, CreateDeployment/GetDeployment/ListDeployments, DeleteApplicationFromEnvironment, StartApplication/StopApplication, StartBatchJob/GetBatchJobExecution/ListBatchJobExecutions/CancelBatchJobExecution/ListBatchJobDefinitions/ListBatchJobRestartPoints, the data-set import/export and ListDataSets/GetDataSetDetails family, Tag/Untag/ListTagsForResource, ListEngineVersions, GetSignedBluinsightsUrl.

## Evidence

`services/service-com-amazonaws-m2-awssupernovacontrolplaneservice/OPERATIONS.md` (generated from `smithy-model/com-amazonaws-m2-awssupernovacontrolplaneservice.json`) documents all 37 operations, including: CreateApplication `POST /applications` with required `name`/`engineType`/`definition` and required outputs `applicationArn`/`applicationId`/`applicationVersion` (lines 63-69); GetApplication `GET /applications/{applicationId}` with required `name`/`applicationId`/`applicationArn`/`status`/`latestVersion`/`engineType`/`creationTime` and `ResourceNotFoundException` 404 (lines 71-77); UpdateApplication `PATCH /applications/{applicationId}` with required `currentApplicationVersion`, required output `applicationVersion`, and `ConflictException` 409 (lines 79-84); DeleteApplication `DELETE /applications/{applicationId}` (lines 86-92); ListApplications `GET /applications` with @paginated(inputToken=nextToken, outputToken=nextToken, items=applications) and a `names` filter (lines 94-100); GetApplicationVersion `GET /applications/{applicationId}/versions/{applicationVersion}` with required `definitionContent`/`status`/`creationTime` (lines 140-146). Handler evidence: `src/ops/create_application.rs` lines 4 and 188 — `//! SCAFFOLDED, NOT IMPLEMENTED:` and `Err(crate::scaffolded::Scaffolded::not_implemented("CreateApplication"))`; every other file in `src/ops/` is the same generated stub. `src/state.rs` line 8 is `// TODO: add service-specific state fields`, so no operation can persist anything today. No tests assert any of this behaviour (`tests/` holds only the generated `state_roundtrip.rs` example).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `m2` client at it.
2. `aws m2 create-application --name BootstrapApp --engine-type bluage --definition content='{"template-version":"2.0"}' --description 'first app' --tags env=dev` → expect 200 with an `applicationId`, an `applicationArn` of the form arn:aws:m2:<region>:<account>:app/<id>, and `applicationVersion` 1 (today: HTTP 501 NotImplemented).
3. `aws m2 get-application --application-id <id>` → expect the same name/arn/engineType/description/tags that were created, a `creationTime`, and `latestVersion.applicationVersion` == 1.
4. `aws m2 list-applications` → expect exactly one ApplicationSummary for <id>; repeat with `--names BootstrapApp` (expect the same one) and `--names NoSuchApp` (expect an empty list); with 3+ applications created, `--max-results 1` must return 1 item plus a `nextToken` that resumes without repeats or omissions.
5. `aws m2 update-application --application-id <id> --current-application-version 1 --description 'second rev'` → expect `applicationVersion` 2; re-running with `--current-application-version 1` must now fail ConflictException (409).
6. `aws m2 get-application-version --application-id <id> --application-version 2` → expect the updated description and the stored `definitionContent`; version 99 → ResourceNotFoundException (404).
7. `aws m2 delete-application --application-id <id>`, then `aws m2 get-application --application-id <id>` → expect ResourceNotFoundException (404) and the application absent from list-applications.
8. `aws m2 get-application --application-id does-not-exist` → expect ResourceNotFoundException (404), not 501.
