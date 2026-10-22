Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 16fab4a727e8e34e48fe5cd80cb0080f7932bd9f

# Bootstrap the SecurityAgent emulator

_Filed: 2026-10-21T23:18:07Z_
_Operation: service-com-amazonaws-securityagent-securityagent | Class: missing_

> _Note: this issue was identified against the code base at commit `16fab4a727e8e34e48fe5cd80cb0080f7932bd9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 96 operations in `services/service-com-amazonaws-securityagent-securityagent/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. This issue defines the FIRST, SMALL slice to implement: the lifecycle of the service's root resource, the **Application** (agent spaces, code reviews, pentests, threat models and findings all hang off an `applicationId`, so this is the entry point every realistic client script starts from).

Wire protocol is `restJson1`, sigv4 signing name `securityagent`; each operation below is a `POST /<OperationName>` with a JSON body (per OPERATIONS.md "Wire protocol" and the per-op Route lines).

Implement these 5 operations, in this order, all backed by the single shared engine store reached via `crate::state::service_state(&*self.engine)`:

1. **CreateApplication** — input `idcInstanceArn` (string), `roleArn` (string), `defaultKmsKeyId` (string), `tags` (map<string,string>); output `applicationId` (string, required). Must mint a NEW unique `applicationId` per call, insert an Application record into state carrying every supplied input field, and derive and store a `domain` and an `applicationName` so later reads have something real to return. When `idcInstanceArn` is supplied, build and store an `IdCConfiguration{idcApplicationArn, idcInstanceArn}` (the `idcApplicationArn` synthesized by the emulator). Two calls must yield two distinct ids and two distinct records.
2. **GetApplication** (`@readonly`) — input `applicationId` (required); output `applicationId` (required), `domain` (required), `applicationName`, `idcConfiguration` (IdCConfiguration), `roleArn`, `defaultKmsKeyId`. Must look the id up in state and return exactly what CreateApplication/UpdateApplication persisted — not an echo of the request and not placeholder strings. Unknown id must fail (404 `ResourceNotFoundException`, the service-wide error shape used at HTTP 404 elsewhere in the model, e.g. AddArtifact), never a hollow 200.
3. **ListApplications** (`@paginated(inputToken=nextToken, outputToken=nextToken, items=applicationSummaries)`, `@readonly`) — input `nextToken`, `maxResults`; output `applicationSummaries` (list<ApplicationSummary>, required), `nextToken`. Must enumerate the stored applications, one `ApplicationSummary{applicationId, applicationName, domain, defaultKmsKeyId}` per record, in a stable order. `maxResults` must truncate the page and emit a `nextToken` that resumes exactly after the last returned item; the final page must omit `nextToken`. A deleted application must not appear.
4. **UpdateApplication** (`@idempotent`) — input `applicationId` (required), `roleArn`, `defaultKmsKeyId`; output `applicationId` (required). Must mutate the stored record in place (supplied fields overwritten, omitted fields left as-is) so a subsequent GetApplication observes the new values. Unknown id must 404.
5. **DeleteApplication** (`@idempotent`) — input `applicationId` (required); empty output. Must remove the record from state so GetApplication afterwards 404s and ListApplications no longer lists it.

Every one of these must compute its response from persisted state. Hardcoded/canned bodies, echoing the request back, or minting a random `applicationId` without storing anything are explicit failures of this issue — a create followed by a get must return what was created, and state must survive across requests for the life of the process.

## State Model

In `src/state.rs`, on `ComAmazonawsSecurityagentSecurityagentState` (the lockable per-service store):

- `applications: BTreeMap<String /* applicationId */, Application>` — BTreeMap (not HashMap) so ListApplications has the stable ordering pagination tokens require.
- `struct Application { application_id: String, application_name: String, domain: String, role_arn: Option<String>, default_kms_key_id: Option<String>, idc_configuration: Option<IdcConfig>, tags: BTreeMap<String, String>, created_at: SystemTime }`
- `struct IdcConfig { idc_application_arn: Option<String>, idc_instance_arn: Option<String> }`
- `next_application_seq: u64` — monotonic counter used to mint unique `applicationId` values (and, with `created_at`, to order list results deterministically).

Pagination tokens should be derived from the last-returned `applicationId` (an opaque, resumable cursor into the BTreeMap), not from an offset that shifts when records are deleted.

Everything outside this slice (agent spaces, code reviews, pentests, threat models, findings, integrations, private connections, security requirement packs, target domains, artifacts, tagging) stays at 501 and will be filed as individual gap issues later. Keep this first slice to the five Application operations.

Done = a client script can create an application, read it back with the same field values, see it in a paginated list, update it and observe the change, delete it, and then get a 404 — with correct JSON bodies, not merely 2xx.

## Evidence

1) `services/service-com-amazonaws-securityagent-securityagent/OPERATIONS.md` lines 546-583 define the five Application operations with the exact input/output members and traits quoted above (CreateApplication `POST /CreateApplication` -> `applicationId` required; GetApplication `@readonly` -> `applicationId`/`domain` required plus `applicationName`/`idcConfiguration`/`roleArn`/`defaultKmsKeyId`; UpdateApplication `@idempotent`; DeleteApplication `@idempotent`, no output members; ListApplications `@paginated(inputToken=nextToken, outputToken=nextToken, items=applicationSummaries)` with required `applicationSummaries`).
2) `smithy_query shape=ApplicationSummary` gives members `applicationId` (required), `applicationName` (required), `domain` (required), `defaultKmsKeyId`; `shape=IdCConfiguration` gives `idcApplicationArn`, `idcInstanceArn` — these are the fields ListApplications/GetApplication must populate from state.
3) `smithy_query service_info`: protocol `restJson1`, sdkId `SecurityAgent`, sigv4 name `securityagent`, 96 operations.
4) All-stubs confirmed: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-securityagent-securityagent/src/ops/` matches every operation file, including `create_application.rs`, `get_application.rs`, `list_applications.rs`, `update_application.rs`, `delete_application.rs`. Each returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501, and no test asserts any Application behavior (only the generated `tests/state_roundtrip.rs` exists).
5) The @paginated / @idempotent / @readonly contract above is from the model itself, so pagination and delete-then-404 are spec-grounded, not invented.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/`curl` client for `securityagent` (restJson1, sigv4) at it.
2. POST /CreateApplication with body {"idcInstanceArn":"arn:aws:sso:::instance/ssoins-1111","roleArn":"arn:aws:iam::123456789012:role/SecAgent","defaultKmsKeyId":"alias/sec","tags":{"env":"test"}} — expect 200 with a fresh `applicationId`; today: HTTP 501 NotImplemented.
3. POST /CreateApplication a second time with different field values — expect a DIFFERENT `applicationId`.
4. POST /GetApplication with {"applicationId":"<id from step 2>"} — expect 200 whose `roleArn`, `defaultKmsKeyId` and `idcConfiguration.idcInstanceArn` equal what step 2 sent, plus a non-empty `domain` and `applicationName`.
5. POST /ListApplications with {"maxResults":1} — expect exactly 1 `ApplicationSummary` (applicationId/applicationName/domain populated from state) and a `nextToken`; POST /ListApplications again with that `nextToken` — expect the OTHER application and no `nextToken` on the final page.
6. POST /UpdateApplication with {"applicationId":"<id>","roleArn":"arn:aws:iam::123456789012:role/Changed"} then POST /GetApplication for the same id — expect `roleArn` to be the new value and `defaultKmsKeyId` unchanged.
7. POST /DeleteApplication with {"applicationId":"<id>"} — expect 200; then POST /GetApplication for that id — expect a 404 ResourceNotFoundException, and POST /ListApplications must no longer include it.
8. POST /GetApplication with {"applicationId":"does-not-exist"} — expect 404 ResourceNotFoundException, not a 200 with an empty/echoed body.
