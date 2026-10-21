Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the AWS Account Access emulator

_Filed: 2026-10-21T21:18:54Z_
_Operation: service-com-amazonaws-accountaccess-awsaccountaccess | Class: stub_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 11 operations in `services/service-com-amazonaws-accountaccess-awsaccountaccess/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented` while holding zero state. Nothing works end to end.

Implement a FIRST, SMALL stateful slice: the create -> use -> read -> list -> delete loop of the primary resource (an account-access *application*) and its child resource (*entitlements*). Protocol is `restJson1`, sigv4 signing name `account-access`.

Happy path, in order (fields from OPERATIONS.md):

1. **CreateApplication** — `POST /applications` -> 200. Input: `identitySource` (union `IdentitySource`, required; member `identityCenter.instanceArn` matching `^arn:[a-z0-9-]+:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$`), `tags` (map<string,string>). Must MINT and persist a new application: generate `applicationArn` of the form `arn:aws:account-access:<region>:<account-id>:application/<id>` (the output member is required and must match the modeled pattern, len 49..2048), store identitySource, tags, `createdAt`/`updatedAt` (now), `status = ACTIVE`, and a `tenantId`. Return the minted ARN.
2. **GetApplication** — `GET /applications/{applicationArn}` -> 200. Must look the stored application up by ARN and return the PERSISTED values: `identitySource` (`IdentitySourceDetails`, required — `identityCenter.instanceArn` as supplied at create, plus an emulator-minted `identityCenter.applicationArn`), `status` (required, `ACTIVE` for a successfully created app), `tenantId`, `createdAt`, `updatedAt` (both required), `tags`. Unknown-but-well-formed ARN -> `ResourceNotFoundException` (404); malformed ARN -> `ValidationException` (400).
3. **CreateEntitlement** — `POST /entitlements` -> 200. Input: `applicationArn` (required), `entitlement` (union `Entitlement`, required; `principalRole{ principal (union Principal -> identityCenter), roleArn matching ^arn:[a-z0-9-]+:iam::[0-9]{12}:role/...$ }`). Must verify the parent application exists (else `ResourceNotFoundException` 404), mint a stable `entitlementId` (required output), and persist the entitlement under that application with `createdAt`.
4. **GetEntitlement** — `GET /entitlements/{entitlementId}?applicationArn=...` -> 200. Must return the persisted record: `applicationArn`, `entitlementId`, `entitlement` (`EntitlementDetails.principalRole` = stored `principal` + `roleArn` + required `account`, which is the 12-digit account id PARSED from the stored `roleArn`), `createdAt`. An id that does not exist, or that belongs to a different `applicationArn`, -> `ResourceNotFoundException` (404).
5. **ListEntitlements** — `POST /entitlements-list` -> 200. Input: `applicationArn` (required), `filter` (`EntitlementFilter`, required), `maxResults`, `nextToken`. Must enumerate the entitlements actually stored for that application as `EntitlementsListMember{ entitlementId, entitlement: EntitlementSummary.principalRole{principal, roleArn, account}, createdAt }`, apply the filter (`principalRole.principal` / `roleArn` / `account` — all specified criteria must match, per the shape doc), honour `maxResults` by truncating and returning an opaque `nextToken` that resumes exactly where the page stopped, and return an empty list (not an error) when the application has no entitlements. Unknown `applicationArn` -> `ResourceNotFoundException` (404).
6. **DeleteApplication** — `DELETE /applications/{applicationArn}` -> 204 with no body. Must remove the application AND its entitlements from the store, so a subsequent `GetApplication` / `GetEntitlement` / `ListEntitlements` for it returns `ResourceNotFoundException` (404).

Every one of these must compute its response from persisted state held in the shared engine (`crate::state::service_state(&*self.engine)`), never from canned constants or echoed input. An echoed `GetApplication`, a random `entitlementId` that is not stored, or a `ListEntitlements` that returns a fabricated list all count as failures of this issue. `CreateApplication` and `CreateEntitlement` are `@idempotent` in the model, so a repeated identical create must not silently create a duplicate resource.

## State Model

In `src/state.rs`, on `ComAmazonawsAccountaccessAwsaccountaccessState`:

- `applications: BTreeMap<String /* applicationArn */, Application>` where
  `Application { application_arn: String, tenant_id: String, identity_center_instance_arn: String, identity_center_application_arn: String, status: Status /* ACTIVE | CREATE_IN_PROGRESS | CREATE_FAILED | DELETE_IN_PROGRESS | DELETE_FAILED */, created_at: DateTime, updated_at: DateTime, tags: BTreeMap<String,String>, error: Option<ErrorDetails> }`
- `entitlements: BTreeMap<String /* entitlementId */, EntitlementRecord>` where
  `EntitlementRecord { entitlement_id: String, application_arn: String, principal: Principal /* identityCenter */, role_arn: String, account: String /* parsed from role_arn */, account_name: Option<String>, created_at: DateTime }`
- `entitlements_by_application: BTreeMap<String /* applicationArn */, Vec<String /* entitlementId */>>` preserving creation order, so `ListEntitlements` pages deterministically and `DeleteApplication` can cascade.
- `next_id: u64` (or equivalent counter) used to mint unique application ids and entitlement ids.

All of it reached through the ONE shared engine via `state.lock()`; never construct a fresh engine inside a handler.

Out of scope for this first slice (they stay 501 and are filed as individual gap issues later): `ListApplications`, `DeleteEntitlement`, `TagResource`, `UntagResource`, `ListTagsForResource`.

Done = a client script can create an application, read it back with the values it supplied, add an entitlement, read and list it, then delete the application and observe 404s — with correct bodies, not merely 2xx.

## Evidence

1) `services/service-com-amazonaws-accountaccess-awsaccountaccess/OPERATIONS.md` lists all 11 operations with routes, required input/output members, patterns and error sets — including `CreateApplication` (`POST /applications`, required output `applicationArn`), `GetApplication` (`GET /applications/{applicationArn}`, required `identitySource`/`status`/`createdAt`/`updatedAt`), `CreateEntitlement` (`POST /entitlements`, required output `entitlementId`), `GetEntitlement` (`GET /entitlements/{entitlementId}` + `@httpQuery(applicationArn)`), `ListEntitlements` (`POST /entitlements-list`, `@paginated(inputToken=nextToken, outputToken=nextToken, items=entitlements)`), and `DeleteApplication` (`DELETE /applications/{applicationArn} -> 204`).
2) Smithy model `smithy-model/com-amazonaws-accountaccess-awsaccountaccess.json`: `IdentitySource`/`IdentitySourceDetails` unions over `IdentityCenter{instanceArn}` / `IdentityCenterDetails{instanceArn, applicationArn}`; `Entitlement`/`EntitlementDetails` unions over `PrincipalRoleEntitlement{principal, roleArn}` / `PrincipalRoleEntitlementDetails{principal, roleArn, account (required), accountName}`; `EntitlementsListMember{entitlementId, entitlement: EntitlementSummary, createdAt}`; `EntitlementFilter.principalRole{principal, roleArn, account}` documented as "All specified criteria must match for an entitlement to be returned."; `ResourceNotFoundException` (404) and `ValidationException` (400) are declared on these operations.
3) `grep -rn 'SCAFFOLDED, NOT IMPLEMENTED' src/ops/` matches all 11 handler files (create_application.rs, get_application.rs, create_entitlement.rs, get_entitlement.rs, list_entitlements.rs, delete_application.rs, list_applications.rs, delete_entitlement.rs, tag_resource.rs, untag_resource.rs, list_tags_for_resource.rs) — each returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501. No test asserts any behavior beyond the generated `tests/state_roundtrip.rs` state-API example.

## Steps to Reproduce

1. Start the emulator and point an AWS Account Access client at it (restJson1, signing name `account-access`).
2. CreateApplication with identitySource={identityCenter:{instanceArn:"arn:aws:sso:::instance/ssoins-1234567890abcdef"}} and tags={"env":"dev"} — expect 200 with a well-formed `applicationArn`; today: HTTP 501 NotImplemented.
3. GetApplication with that applicationArn — expect 200 with status=ACTIVE, the same identityCenter.instanceArn, tags={"env":"dev"}, and createdAt/updatedAt timestamps; today: HTTP 501.
4. CreateEntitlement with that applicationArn and entitlement={principalRole:{principal:{identityCenter:{...}}, roleArn:"arn:aws:iam::123456789012:role/Readonly"}} — expect 200 with an `entitlementId`; today: HTTP 501.
5. GetEntitlement with that entitlementId and applicationArn — expect 200 echoing the stored principal, roleArn and account="123456789012" plus createdAt; today: HTTP 501.
6. ListEntitlements for that applicationArn with an empty-ish filter and maxResults=1 — expect the created entitlement in `entitlements`, and a `nextToken` only when more remain; today: HTTP 501.
7. DeleteApplication for that applicationArn — expect 204 with empty body; then GetApplication and GetEntitlement for the same ids must return ResourceNotFoundException (404); today: HTTP 501 for all three.
8. GetApplication on a well-formed but never-created ARN must return 404 ResourceNotFoundException, and on a malformed ARN 400 ValidationException.
