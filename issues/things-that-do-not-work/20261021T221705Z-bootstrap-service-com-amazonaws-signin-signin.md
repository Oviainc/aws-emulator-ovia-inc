Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 7d7bdea90bc70459c4b589287302dc4eaf39117d

# Bootstrap the signin emulator

_Filed: 2026-10-21T22:17:05Z_
_Operation: service-com-amazonaws-signin-signin | Class: stub_

> _Note: this issue was identified against the code base at commit `7d7bdea90bc70459c4b589287302dc4eaf39117d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 11 operations of the `Signin` service (restJson1, sigv4 signing name `signin`) are unmodified scaffolds that answer HTTP 501 `NotImplemented` and hold no state. Implement a FIRST, SMALL stateful slice covering the lifecycle of the service's primary resource — the signin resource-permission statement — plus the console-authorization configuration read/write pair.

### Happy path to implement (in order)

1. **PutResourcePermissionStatement** — `POST /put-resource-permission-statement`, @idempotent. Input: the condition keys `sourceVpc` (`^vpc-([0-9a-f]{8}|[0-9a-f]{17})$`), `signinSourceVpce` / `consoleSourceVpce` (`^vpce-[a-z0-9]{8,20}$`), `vpcSourceIp`, `sourceIp`, `requestedRegion` (`^[a-z]{2}(-[a-z]+)+-\d+$`), `excludedPrincipal` (IAM/STS ARN, len 20..2048), and `clientToken` (@idempotencyToken, len 1..64). Output: `statementId` (required, `^[A-Za-z0-9+/]{64}=?$`). Must STORE a new statement record and return the id it stored; a repeat call with the same `clientToken` must return the SAME `statementId` and must not create a second statement.
2. **ListResourcePermissionStatements** — `POST /list-resource-permission-statements`, @readonly, @paginated(inputToken=nextToken, outputToken=nextToken, items=permissionStatements). Input: `maxResults` (1..100), `nextToken` (len 0..2048, `^[-a-zA-Z0-9+=/_]*$`). Output: `permissionStatements` (required list of `PermissionStatementSummary{sid (required), condition: map}`), `nextToken`. Must return exactly the statements created by step 1, each with the condition keys that were supplied, honour `maxResults` as a page bound, and emit `nextToken` only when more statements remain (absent on the last page).
3. **GetResourcePolicy** — `POST /get-resource-policy`, @readonly, no input. Output: `signinResourceBasedPolicy` (required) = `SigninResourceBasedPolicy{version ("Version"), statement ("Statement": list of policy statements)}`. Must RENDER the policy document from the stored statements (one policy statement per stored permission statement, its `Sid` = the stored `statementId`, its `Condition` = the stored condition keys), not a canned document. With no statements stored, return a policy with an empty statement list.
4. **DeleteResourcePermissionStatement** — `POST /delete-resource-permission-statement`. Input: `statementId` (required, `^[A-Za-z0-9+/]{64}=?$`), `clientToken`. Empty output. Must remove the stored statement so that it disappears from ListResourcePermissionStatements and from GetResourcePolicy; deleting an unknown `statementId` must return `ResourceNotFoundException` (HTTP 404), and a malformed/unmatched-pattern id `ValidationException` (HTTP 400).
5. **PutConsoleAuthorizationConfiguration** — `POST /put-console-authorization-configuration`, @idempotent. Input: `targetId` (len 12..32, `^(\d{12}|o-[a-z0-9]{10}|r-[0-9a-z]{4,32})$`; default to the caller's account id when absent). Output (all required): `targetId`, `scope`, `consoleAuthorizationEnabled`. Must PERSIST the configuration for that target and derive `scope` from the target form (account id -> account scope, `o-` -> organization, `r-` -> organizational root).
6. **GetConsoleAuthorizationConfiguration** — `POST /get-console-authorization-configuration`, @readonly. Same input/output shape as (5). Must return the configuration persisted by (5) for the same `targetId` — never a hardcoded default — and `ResourceNotFoundException` (HTTP 404) for a target that was never configured.

Every one of these six must compute its response from persisted state held in the crate's shared engine state (`crate::state::service_state`). No canned responses, no echoing the input back, no generating a `statementId` that is never stored. A create followed by a list/get in a separate request must observe what was created; a delete followed by a list must observe it gone. Validation errors (`ValidationException`, 400) must be raised for inputs violating the modeled length/pattern constraints, and `ResourceNotFoundException` (404) for reads/deletes of unknown ids.

## State Model

In `ComAmazonawsSigninSigninState` (src/state.rs), behind the existing lock:

- `permission_statements: IndexMap<String /* statementId */, PermissionStatement>` — insertion-ordered so List and GetResourcePolicy are deterministic and pagination is stable. `PermissionStatement { statement_id: String /* 64-char base64-ish, generated and stored */, source_vpc: Option<String>, signin_source_vpce: Option<String>, console_source_vpce: Option<String>, vpc_source_ip: Option<String>, source_ip: Option<String>, requested_region: Option<String>, excluded_principal: Option<String>, created_at: SystemTime }`, with a `conditions()` helper that projects the populated fields into the `ConditionBlock` map returned by List and embedded in the policy document.
- `statement_idempotency: HashMap<String /* clientToken */, String /* statementId */>` — makes the @idempotent/@idempotencyToken contract on PutResourcePermissionStatement and DeleteResourcePermissionStatement observable.
- `console_auth_configs: HashMap<String /* targetId */, ConsoleAuthorizationConfiguration>` where `ConsoleAuthorizationConfiguration { target_id: String, scope: String, console_authorization_enabled: bool, updated_at: SystemTime }`.
- `list_tokens: HashMap<String /* nextToken */, usize /* offset into permission_statements */>` (or an opaque encoded offset) so ListResourcePermissionStatements' pagination resumes exactly where the previous page ended.
- `policy_version: String` — the `Version` field of the rendered `SigninResourceBasedPolicy`.

Out of scope for this first slice (they keep answering 501 and are filed separately later): CreateOAuth2Token, CreateOAuth2TokenWithIAM, IntrospectOAuth2TokenWithIAM, RevokeOAuth2TokenWithIAM, DeleteConsoleAuthorizationConfiguration.

## Evidence

`services/service-com-amazonaws-signin-signin/OPERATIONS.md` lists all 11 operations with the routes, required members, constraints and errors cited above (PutResourcePermissionStatement lines 95-101, ListResourcePermissionStatements lines 79-85, GetResourcePolicy lines 63-69, DeleteResourcePermissionStatement lines 48-53, Put/GetConsoleAuthorizationConfiguration lines 55-61 and 87-93). `smithy_query shape=SigninResourceBasedPolicy,PermissionStatementSummary` gives `SigninResourceBasedPolicy{version("Version"), statement("Statement")}` and `PermissionStatementSummary{sid required, condition map}`. `grep '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-signin-signin/src/ops` matches all 11 handler files, including put_resource_permission_statement.rs, list_resource_permission_statements.rs, get_resource_policy.rs, delete_resource_permission_statement.rs, put_console_authorization_configuration.rs and get_console_authorization_configuration.rs — every one still returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501) and no handler touches the shared state. README.md confirms the restJson1 wire protocol and that the stub marker line is the census of unimplemented operations.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/`curl` client with sigv4 signing name `signin` at it.
2. POST /put-resource-permission-statement with {"sourceVpc":"vpc-0123456789abcdef0","requestedRegion":"us-east-1","clientToken":"tok-1"} — expect 200 with a 64-char `statementId`; today: HTTP 501 NotImplemented.
3. Repeat the identical call with clientToken "tok-1" — expect the SAME `statementId` and no duplicate statement.
4. POST /list-resource-permission-statements with {"maxResults":1} — expect one `permissionStatements` entry whose `sid` equals the id from step 2 and whose `condition` map contains the supplied sourceVpc/requestedRegion keys.
5. POST /get-resource-policy — expect a `signinResourceBasedPolicy` whose `Statement` list contains a statement with Sid = that `statementId` and the same condition keys.
6. POST /delete-resource-permission-statement with that `statementId` — expect 200; then re-list and re-get the policy and expect the statement to be absent, and a second delete of the same id to return ResourceNotFoundException (404).
7. POST /put-console-authorization-configuration with {"targetId":"o-abc1234567"} — expect 200 with targetId echoed from stored state plus `scope` and `consoleAuthorizationEnabled`.
8. POST /get-console-authorization-configuration with {"targetId":"o-abc1234567"} in a separate request — expect exactly the configuration stored in step 7; and expect ResourceNotFoundException (404) for an unconfigured targetId such as "000000000000".
