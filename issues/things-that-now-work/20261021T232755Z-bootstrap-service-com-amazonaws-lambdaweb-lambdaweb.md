Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: e44f583f966e818f9abdd86a5b0ab9a8f2f88230

# Bootstrap the Lambda Web (lambdaweb) emulator

_Filed: 2026-10-21T23:27:55Z_
_Operation: service-com-amazonaws-lambdaweb-lambdaweb | Class: missing_

> _Note: this issue was identified against the code base at commit `e44f583f966e818f9abdd86a5b0ab9a8f2f88230`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 20 operations in `services/service-com-amazonaws-lambdaweb-lambdaweb/src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501. No state is held, so no client workflow works.

Implement a FIRST, SMALL slice: the web-function lifecycle (restJson1, sigv4 name `lambda`, path prefix `/2025-03-07`). These 6 operations, in order, all computing their responses from persisted state:

1. **CreateWebFunction** — `PUT /2025-03-07/web-functions` -> 202. Input: `functionName` (required, 1..256), optional `revisionConfig` (RevisionConfig: `description`, `kmsKeyArn`, required `buildConfig`, required `serviceConfig`), optional `endpointConfig`, optional `tags`. Store a new function; generate `functionArn` = `arn:aws:lambda:<region>:<account>:web-function/<functionName>`; return `functionName`, `functionArn`, `state` (FunctionState: Pending or Active), `stateReason`, `createdAt`, `updatedAt`, plus `revision` (FunctionRevisionSummary) when `revisionConfig` was supplied, and the stored `tags`. Re-creating an existing name must fail with `ConflictException` (409); `maxResults`-style and length/pattern violations with `ValidationException` (400).
2. **CreateWebFunctionRevision** — `POST /2025-03-07/web-functions/{functionName}/revisions` -> 202. Input: `functionName` (@httpLabel), `description`, `kmsKeyArn`, required `buildConfig` (codeConfig + runtimeConfig), required `serviceConfig` (`executionRoleArn` required, `timeoutSeconds` default 30, `maxConcurrencyPerEnvironment` default 64, `environmentVariables`, `telemetryConfig`). Persist the revision under the function, mint `revisionId` + `revisionArn`, return `state` (RevisionState), `stateReason`, `createdAt`, and the stored configs verbatim (including applied defaults). Unknown `functionName` -> `ResourceNotFoundException` (404).
3. **GetWebFunctionRevision** — `GET /2025-03-07/web-functions/{functionName}/revisions/{revisionId}`. Must return exactly the revision persisted in step 2 (same `revisionArn`, `revisionId`, `buildConfig`, `serviceConfig` with defaults applied, `createdAt`); unknown function or revision -> 404.
4. **GetWebFunction** — `GET /2025-03-07/web-functions/{functionName}`. Returns the stored `functionName`, `functionArn`, `state`, `stateReason`, `createdAt`, `updatedAt` from step 1 (not echoed input); unknown name -> `ResourceNotFoundException` (404).
5. **ListWebFunctions** — `POST /2025-03-07/web-functions`, @paginated(inputToken=nextToken, outputToken=nextToken, items=functions). Input: `maxResults` (1..50, default 50), `nextToken` (uuid-shaped), `filters`. Output `functions` as a list of FunctionSummary built from stored functions (`functionName`, `functionArn`, `state`, `stateReason`, `createdAt`, `updatedAt`) with a stable order; emit `nextToken` only when more pages remain and resume correctly from it.
6. **DeleteWebFunction** — `DELETE /2025-03-07/web-functions/{functionName}` -> 204. Removes the function and its revisions/endpoints/tags from state; a subsequent GetWebFunction/GetWebFunctionRevision must return 404 and ListWebFunctions must no longer include it. Unknown name -> 404 (`@idempotent` is about retry safety, not about inventing success for a name that never existed).

No canned, hardcoded or echoed responses: every field above must be read back out of the store, and a create followed by a get/list must show what was created. Timestamps must be real and stable per resource (`createdAt` fixed at creation, `updatedAt` bumped on mutation), and ARNs must be derived from the request's region/account.

## State Model

In `src/state.rs` (`ComAmazonawsLambdawebLambdawebState`, reached via `crate::state::service_state(&*self.engine)`):

- `functions: BTreeMap<String /* functionName */, WebFunction>` — insertion/name-ordered for deterministic listing.
- `WebFunction { function_name: String, function_arn: String, state: FunctionState /* Pending|Active|Failed|Deleting */, state_reason: String, created_at: SystemTime, updated_at: SystemTime, tags: BTreeMap<String,String>, revisions: BTreeMap<String /* revisionId */, Revision>, latest_revision_id: Option<String>, endpoints: BTreeMap<String /* endpointName */, Endpoint> /* reserved for the later endpoint slice */, resource_policy: Option<(String /* policy */, String /* revisionId uuid */)> /* reserved */ }`
- `Revision { revision_id: String, revision_arn: String, description: Option<String>, kms_key_arn: Option<String>, build_config: BuildConfig /* codeConfig + runtimeConfig as modeled */, service_config: ServiceConfig /* execution_role_arn, timeout_seconds (default 30), max_concurrency_per_environment (default 64), environment_variables, telemetry_config, egress_network_connector_arn */, state: RevisionState /* Pending|Active|Failed */, state_reason: String, errors: Vec<RevisionError>, created_at: SystemTime }`
- `list_tokens: HashMap<String /* uuid nextToken */, ListCursor { kind, function_name: Option<String>, offset: usize }>` — backs the uuid-shaped `nextToken` for the paginated list ops.

All other operations (CreateWebFunctionEndpoint / Get / Update / Delete / ListWebFunctionEndpoints, ListWebFunctionRevisions, DeleteWebFunctionRevision, Put/Get/DeleteResourcePolicy, Tag/Untag/ListTags, GetWebAccountSettings) stay at 501 in this slice and are expected to be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-lambdaweb-lambdaweb/OPERATIONS.md` lists all 20 operations with their routes, required members and errors (CreateWebFunction lines 82-88, GetWebFunction 90-96, DeleteWebFunction 98-104, ListWebFunctions 106-112, CreateWebFunctionRevision 154-159, GetWebFunctionRevision 161-167) and the wire protocol (restJson1, sigv4 `lambda`). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-lambdaweb-lambdaweb/src/ops/` matches all 20 handler files, i.e. the whole service answers 501 today. Shape members for RevisionConfig/BuildConfig/ServiceConfig/FunctionSummary/FunctionRevisionSummary (including `timeoutSeconds` default 30 and `maxConcurrencyPerEnvironment` default 64) come from the Smithy model via smithy_query. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. PUT /2025-03-07/web-functions with {"functionName":"demo-fn","tags":{"env":"dev"}} — expect 202 with functionArn arn:aws:lambda:<region>:<account>:web-function/demo-fn, state Pending|Active, createdAt/updatedAt set (today: 501).
2. POST /2025-03-07/web-functions/demo-fn/revisions with a buildConfig (codeConfig+runtimeConfig) and serviceConfig {executionRoleArn: arn:aws:iam::123456789012:role/demo} — expect 202 with a new revisionId/revisionArn and serviceConfig echoing timeoutSeconds 30 and maxConcurrencyPerEnvironment 64 defaults.
3. GET /2025-03-07/web-functions/demo-fn/revisions/<revisionId> — expect 200 returning the exact revision stored in the previous step.
4. GET /2025-03-07/web-functions/demo-fn — expect 200 with the same functionArn/createdAt as the create response.
5. POST /2025-03-07/web-functions with {"maxResults":1} — expect 200 with functions[] containing demo-fn's FunctionSummary, and nextToken present only if a second function exists; follow the token and verify no duplicates/omissions.
6. PUT /2025-03-07/web-functions with the same functionName again — expect 409 ConflictException.
7. DELETE /2025-03-07/web-functions/demo-fn — expect 204; then GET /2025-03-07/web-functions/demo-fn -> 404 ResourceNotFoundException and POST /2025-03-07/web-functions no longer lists it.
8. GET /2025-03-07/web-functions/never-created — expect 404 ResourceNotFoundException, not 501.
