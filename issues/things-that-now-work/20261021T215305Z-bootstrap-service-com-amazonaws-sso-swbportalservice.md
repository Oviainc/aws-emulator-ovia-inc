Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 9fd9433686ca60725b6b64638320351edab7ca3b

# Bootstrap the sso-swbportalservice emulator

_Filed: 2026-10-21T21:53:05Z_
_Operation: service-com-amazonaws-sso-swbportalservice | Class: missing_

> _Note: this issue was identified against the code base at commit `9fd9433686ca60725b6b64638320351edab7ca3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 4 operations in this crate is an unmodified scaffold answering HTTP 501 `NotImplemented` and holding zero state (`src/ops/{list_accounts,list_account_roles,get_role_credentials,logout}.rs`). Implement the full SSO portal happy path — the whole service surface is only 4 operations, so the first slice IS the core loop:

1. **ListAccounts** — `GET /assignment/accounts`. Input: `accessToken` (header `x-amz-sso_bearer_token`, required), optional `maxResults` (1..100), `nextToken`. Output: `accountList` (list of `AccountInfo{accountId, accountName, emailAddress}`) and `nextToken` when a page was truncated. The accounts returned must be exactly those bound to the session that owns the presented access token, read from persisted state — not a canned list.
2. **ListAccountRoles** — `GET /assignment/roles`. Input: `accessToken` (header, required), `accountId` (query `account_id`, required), optional `maxResults` (1..100), `nextToken`. Output: `roleList` (list of `RoleInfo{roleName, accountId}`) + `nextToken`. Must return only the roles assigned for THAT accountId for THAT token, and must raise `ResourceNotFoundException` (404) when the accountId is not one of the accounts the session can see.
3. **GetRoleCredentials** — `GET /federation/credentials`. Input: `roleName` (query `role_name`, required), `accountId` (query `account_id`, required), `accessToken` (header, required). Output: `roleCredentials` = `RoleCredentials{accessKeyId, secretAccessKey, sessionToken, expiration(long epoch millis)}`. The (accountId, roleName) pair must be one that ListAccountRoles reported for this session or the call fails with `ResourceNotFoundException` (404); the minted credential must be stored so it can be looked up/validated, and `expiration` must be a real future timestamp derived from the issue time, not a constant.
4. **Logout** — `POST /logout`. Input: `accessToken` (header, required). Output: empty body, HTTP 200. Must invalidate the session in state: every subsequent ListAccounts / ListAccountRoles / GetRoleCredentials with that same token must then fail with `UnauthorizedException` (401), proving the logout mutated persisted state.

Cross-cutting requirements for all four:
- An unknown, malformed or logged-out `accessToken` → `UnauthorizedException` (HTTP 401) on every operation; a missing/invalid required query param or `maxResults` outside 1..100 → `InvalidRequestException` (HTTP 400).
- Pagination must be real: `maxResults` bounds the page, `nextToken` is an opaque cursor whose continuation resumes exactly after the last item returned, and the last page omits `nextToken`. Ignoring these is a failure.
- Responses must be computed from persisted state. Hardcoded account/role lists, echoed inputs, and random credentials that are never stored are all failures.
- Seeding: because this service has no create API, provide a way to populate the session/account/role fixture (state-seeding helper or test constructor) so a client script has something to list; the listing operations must read it, never synthesize it.

## State Model

- `ComAmazonawsSsoSwbportalserviceState` (in `src/state.rs`) holds:
  - `sessions: HashMap<String /* accessToken */, Session>` where `Session { subject: String, accounts: Vec<String> /* accountIds visible to this session */, active: bool, expires_at: SystemTime }`. `Logout` sets `active = false` (or removes the entry); every operation resolves its token here first and returns `UnauthorizedException` if absent/inactive/expired.
  - `accounts: HashMap<String /* accountId */, Account>` where `Account { account_id: String, account_name: String, email_address: String, roles: Vec<String> /* roleNames */ }`. Backs `AccountInfo` for ListAccounts and the role set for ListAccountRoles; iteration order must be deterministic (sorted by accountId) so pagination cursors are stable.
  - `issued_credentials: HashMap<(String /* accessToken */, String /* accountId */, String /* roleName */), RoleCredentialRecord>` where `RoleCredentialRecord { access_key_id: String, secret_access_key: String, session_token: String, expiration_millis: i64 }`. GetRoleCredentials mints into this map (reusing an unexpired record for the same key) so the same session re-fetching the same role sees a consistent, stored credential.
  - Page cursors encoded from a deterministic offset/last-key over the sorted account or role listing, so `nextToken` round-trips without storing per-request server state.

Done = a client can: list accounts for a seeded session, list roles for one of those accounts, fetch role credentials for one of those roles and receive a future `expiration`, call Logout, and then see 401 on all three read operations. Nothing here may be canned.

## Evidence

OPERATIONS.md for this crate lists exactly 4 operations (GetRoleCredentials, ListAccountRoles, ListAccounts, Logout) with the routes, required `x-amz-sso_bearer_token` header, `maxResults` range 1..100, `@paginated(inputToken=nextToken, outputToken=nextToken, items=accountList|roleList)` traits, and the error set InvalidRequestException(400)/ResourceNotFoundException(404)/TooManyRequestsException(429)/UnauthorizedException(401). Smithy shapes confirm the output members: `AccountInfo{accountId, accountName, emailAddress}`, `RoleInfo{roleName, accountId}`, `RoleCredentials{accessKeyId, secretAccessKey(sensitive), sessionToken(sensitive), expiration(long)}`. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-sso-swbportalservice/src/ops/` matches all four handler files, so the entire service answers 501 today and no test asserts any behavior.

## Steps to Reproduce

1. Seed a session fixture: access token `tok-1` bound to accounts `111111111111` (roles `AdminRole`, `ReadOnly`) and `222222222222` (role `ReadOnly`).
2. GET /assignment/accounts with header `x-amz-sso_bearer_token: tok-1` — expect 200 and an accountList containing both seeded accounts with their names and email addresses; today returns 501.
3. GET /assignment/accounts?max_result=1 — expect exactly 1 account plus a nextToken; re-issue with next_token and expect the remaining account and no nextToken.
4. GET /assignment/roles?account_id=111111111111 with the same token — expect 200 with roleList = [AdminRole, ReadOnly] scoped to that account.
5. GET /assignment/roles?account_id=999999999999 — expect ResourceNotFoundException (HTTP 404).
6. GET /federation/credentials?account_id=111111111111&role_name=AdminRole with the same token — expect 200 with roleCredentials containing accessKeyId/secretAccessKey/sessionToken and an `expiration` in the future.
7. GET /federation/credentials?account_id=111111111111&role_name=NoSuchRole — expect ResourceNotFoundException (HTTP 404).
8. POST /logout with header `x-amz-sso_bearer_token: tok-1` — expect HTTP 200 and an empty body.
9. Repeat GET /assignment/accounts, GET /assignment/roles and GET /federation/credentials with `tok-1` — each must now fail with UnauthorizedException (HTTP 401), proving Logout mutated persisted state.
10. Call any operation with header `x-amz-sso_bearer_token: bogus` — expect UnauthorizedException (HTTP 401); call ListAccounts with `max_result=0` or `max_result=101` — expect InvalidRequestException (HTTP 400).
