Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 9821f2ab76525a0c0a9a7ed388e3f955f1962573

# Bootstrap the AWSAccountAccessRuntime emulator

_Filed: 2026-11-10T13:46:42Z_
_Operation: service-com-amazonaws-accountaccessruntime-awsaccountaccessruntime | Class: stub_

> _Note: this issue was identified against the code base at commit `9821f2ab76525a0c0a9a7ed388e3f955f1962573`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in this crate is an unmodified scaffold answering HTTP 501, and `src/state.rs` holds no fields at all (`// TODO: add service-specific state fields`). The service binds only TWO operations, so the whole happy path IS the bootstrap slice:

1. **ListRoles** — `GET /access`, input `tenantId` (required, len 3..24, pattern `^[a-z0-9-]+$`), `maxResults`, `nextToken`; output `roles` (required list of `AccessDetails{accountId (required, ^[0-9]{12}$), accountName (optional), roleArn (required)}`) and `nextToken`. Must return the entitlements recorded in state for THAT tenantId (an unknown tenant yields an empty `roles` list, not a canned role), and must honour pagination: slice the stored role list by `maxResults`, emit an opaque `nextToken` only when more roles remain, and resume exactly where the supplied token left off (`@paginated(inputToken=nextToken, outputToken=nextToken, pageSize=maxResults, items=roles)`).
2. **GetRoleCredentials** — `GET /access/credentials`, input `tenantId` (required), `roleArn` (required), `accessType` (required, enum `PROGRAMMATIC_ACCESS`), optional `sessionName` (pattern `^[a-zA-Z0-9_+=,.@-]{2,64}$`) and `sessionDuration`; output `credentialsDetails`, a union whose `programmaticAccess` member is `ProgrammaticAccessDetails{accessKeyId, secretAccessKey, sessionToken, expiresAt}` (all four required). The handler must look the `(tenantId, roleArn)` pair up in the SAME state ListRoles reads: when the pair is not an entitlement of that tenant, return the modelled `AccessDeniedException` (HTTP 403) — the model says "If you aren't entitled to the role, the request fails with an `AccessDeniedException` error." On success it must MINT a credential record into state (access key id, secret, session token, `expiresAt` = now + `sessionDuration` when supplied, otherwise the service default window) and return exactly the stored values — not a hardcoded triple.

There is no modelled create/delete operation, so the entitlement data must be seedable/bootstrappable in `state.rs` (e.g. a default tenant with a couple of roles, plus whatever the crate's test helper inserts) and both handlers must read that one shared store via `crate::state::service_state(&*self.engine)`. No canned or echoed responses: a role returned by ListRoles must be accepted by GetRoleCredentials, and a role never recorded must be refused.

## State Model

```rust
pub struct ComAmazonawsAccountaccessruntimeAwsaccountaccessruntimeState {
    /// tenantId -> that tenant's entitlements, in a stable (insertion/sorted) order
    /// so pagination tokens stay meaningful across calls.
    tenants: BTreeMap<String, Tenant>,
    /// every credential set minted by GetRoleCredentials, newest last
    sessions: Vec<IssuedSession>,
}

struct Tenant {
    /// one entry per accessible role; serialized straight into `roles`
    roles: Vec<RoleEntitlement>,
}

struct RoleEntitlement {
    account_id: String,        // 12 digits -> AccessDetails.accountId
    account_name: Option<String>, // -> AccessDetails.accountName (may be absent)
    role_arn: String,          // -> AccessDetails.roleArn; the GetRoleCredentials lookup key
}

struct IssuedSession {
    tenant_id: String,
    role_arn: String,
    session_name: Option<String>,
    access_key_id: String,
    secret_access_key: String,
    session_token: String,
    expires_at: SystemTime,    // -> ProgrammaticAccessDetails.expiresAt
}
```

Done = an integration test that lists a seeded tenant's roles (paging through with `maxResults`), feeds a listed `roleArn` into GetRoleCredentials and gets back the credential set that was persisted, and gets `AccessDeniedException` for a roleArn absent from that tenant.

## Evidence

Both handlers are untouched scaffolds:
- `services/service-com-amazonaws-accountaccessruntime-awsaccountaccessruntime/src/ops/list_roles.rs:4`: `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented` and`
- `services/service-com-amazonaws-accountaccessruntime-awsaccountaccessruntime/src/ops/get_role_credentials.rs:4`: same marker line.

No state exists to compute an answer from — `services/.../src/state.rs:7-9`:
```rust
pub struct ComAmazonawsAccountaccessruntimeAwsaccountaccessruntimeState {
    // TODO: add service-specific state fields
}
```

Contract, from `OPERATIONS.md:25-39`:
- `## GetRoleCredentials` — "**Route:** `GET /access/credentials -> 200`", "**Input:** `tenantId` (string, len 3..24, pattern ^[a-z0-9-]+$, @httpQuery(tenantId), required), `roleArn` (string, @httpQuery(roleArn), required), `accessType` (AccessType, one of: PROGRAMMATIC_ACCESS, @httpQuery(accessType), required), `sessionName` ..., `sessionDuration` ...", "**Output:** `credentialsDetails` (CredentialsDetails)", "**Errors:** `AccessDeniedException (HTTP 403)` ..."
- `## ListRoles` — "**Traits:** @paginated(inputToken=?, outputToken=?, items=roles), @readonly", "**Output:** `roles` (list<AccessDetails>, required), `nextToken` (string)"

Smithy docs (smithy_query): GetRoleCredentials — "Returns temporary Amazon Web Services credentials for a role that you are entitled to assume within a tenant. ... If you aren't entitled to the role, the request fails with an `AccessDeniedException` error." ListRoles — "Returns the roles that you are entitled to access within a tenant. The results are paginated. Use a returned role with `GetRoleCredentials` to obtain temporary credentials."

Shapes: `ProgrammaticAccessDetails` members `accessKeyId`, `secretAccessKey`, `sessionToken`, `expiresAt` are all `required: true`; `AccessDetails.accountId` is `required` with pattern `^[0-9]{12}$` and `accountName` is documented as "can be absent when the account name can't be resolved".

## Steps to Reproduce

1. Build and run the emulator with this service mounted.
2. Seed/bootstrap one tenant (e.g. tenantId `test-tenant`) with two role entitlements in the service state.
3. GET /access?tenantId=test-tenant&maxResults=1 — expect 200 with exactly one `roles` entry carrying the seeded accountId/roleArn and a non-null `nextToken`. Today: HTTP 501 NotImplemented.
4. GET /access?tenantId=test-tenant&maxResults=1&nextToken=<token from step 3> — expect the SECOND seeded role and no `nextToken`.
5. GET /access/credentials?tenantId=test-tenant&roleArn=<roleArn from step 3>&accessType=PROGRAMMATIC_ACCESS&sessionName=demo — expect 200 with `credentialsDetails.programmaticAccess` containing accessKeyId, secretAccessKey, sessionToken and expiresAt, all read back from the session the call persisted. Today: HTTP 501.
6. GET /access/credentials?tenantId=test-tenant&roleArn=arn:aws:iam::999999999999:role/not-entitled&accessType=PROGRAMMATIC_ACCESS — expect AccessDeniedException with HTTP 403.
7. GET /access?tenantId=unknown-tenant — expect 200 with an empty `roles` list (state-driven), not a canned role.
