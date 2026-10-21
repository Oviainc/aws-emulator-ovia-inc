Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the Wickr Admin API (com-amazonaws-wickr-wickradminapi) emulator

_Filed: 2026-10-21T21:19:05Z_
_Operation: service-com-amazonaws-wickr-wickradminapi | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 44 operations in this crate are unmodified scaffolds: every file under `services/service-com-amazonaws-wickr-wickradminapi/src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented`, holding zero state. No client workflow works at all.

Implement a FIRST, SMALL slice: the network-plus-users lifecycle, which is the primary resource of the Wickr admin API (`restJson1`, sdkId `Wickr`, signing name `wickr`, endpoint prefix `admin.wickr`). Six operations, in this order, each computing its response from persisted state — no canned bodies, no echoing the request back, no random ids that are not stored:

1. **CreateNetwork** — `POST /networks`. Input: `networkName` (required), `accessLevel` (required, PREMIUM|STANDARD), `enablePremiumFreeTrial`, `encryptionKeyArn`. Must mint a NEW `networkId` matching `^[0-9]{8}$` (8 digits, unique per store), persist a Network record (networkName, accessLevel, awsAccountId, networkArn, standing, freeTrialExpiration, migrationState, encryptionKeyArn) and return `networkId`/`networkName`/`encryptionKeyArn` from what was stored. Seed a default security group for the new network (e.g. groupId `"0"`, name `"Default"`) so user creation has a group to reference.
2. **GetNetwork** — `GET /networks/{networkId}`. Must look the stored network up by id and return all required members (`networkId`, `networkName`, `accessLevel`, `awsAccountId`, `networkArn`) exactly as created, plus the optional ones. An unknown but well-formed 8-digit id must yield `ResourceNotFoundError` (HTTP 404), not a 200 and not a 501.
3. **ListNetworks** — `GET /networks`. Must return every network created in this store in the required `networks` list (`Network` shape) — never a hard-coded or empty list after a create. Honour `maxResults` by truncating and emitting an opaque `nextToken` that resumes exactly where the previous page stopped (operation is `@paginated(inputToken=nextToken, outputToken=nextToken, items=networks)`).
4. **BatchCreateUser** — `POST /networks/{networkId}/users`. Input: `networkId` (@httpLabel), `users` (list of `BatchCreateUserRequestItem`: `username` required, `securityGroupIds` required, `firstName`, `lastName`, `inviteCode`, `inviteCodeTtl`, `codeValidation`), plus `clientToken` (@idempotencyToken via `X-Client-Token`). For each item: mint a `userId` matching `^[0-9]+$` (len 1..10), persist a User under the network, and return it in `successful` as a full `User` (userId, firstName, lastName, username, securityGroups, isAdmin=false, suspended=false, status=1 pending, inviteCode). Items that cannot be created — duplicate `username` within the network, unknown security group id, more than one group id (the model documents that a user can only be in a single security group) — must land in `failed` as `BatchUserErrorResponseItem` while the other items still succeed (partial success, HTTP 200). Unknown `networkId` => `ResourceNotFoundError` 404.
5. **ListUsers** — `GET /networks/{networkId}/users`. Must return the users actually persisted for that network in `users`, supporting `maxResults`/`nextToken` pagination and at minimum the `username` and `status` filters; filters must be applied to stored state, not ignored.
6. **DeleteNetwork** — `DELETE /networks/{networkId}`. Must remove the network AND its users/security groups from the store and return `message`. Afterwards GetNetwork/ListUsers on that id must return `ResourceNotFoundError` 404 and ListNetworks must no longer include it. Deleting an already-deleted id must be 404 (the op is `@idempotent`, so a repeat with the same `clientToken` inside a retry window may return the same success).

Validation that the slice must enforce (modeled constraints): `networkId` must be exactly 8 digits and `userId` 1..10 digits, otherwise `ValidationError` (HTTP 422); missing required members (`networkName`, `accessLevel`, `users`, `username`, `securityGroupIds`) => `ValidationError`/`BadRequestError` rather than a silently accepted 200.

Immediate follow-ons once the above pass: **GetUser**, **GetUsersCount**, **CreateSecurityGroup**/**GetSecurityGroup**/**ListSecurityGroups**, **BatchToggleUserSuspendStatus**, **BatchDeleteUser**, **UpdateNetwork**. Everything else (bots, data-retention bots, OIDC, OpenTDF, guest users, device resets) stays 501 and will be filed as individual gap issues later — that is expected.

## State Model

In `src/state.rs` (`ComAmazonawsWickrWickradminapiState`, reached via `crate::state::service_state(&*self.engine)`):

- `networks: BTreeMap<String /* networkId, 8 digits */, Network>` where `Network { network_id, network_name, access_level: AccessLevel, aws_account_id, network_arn /* arn:aws:wickr:<region>:<acct>:network/<networkId> */, standing: i32, free_trial_expiration: Option<String>, migration_state: i32, encryption_key_arn: Option<String>, created_at }`
- `users: BTreeMap<String /* networkId */, BTreeMap<String /* userId */, UserRecord>>` where `UserRecord { user_id, username, uname, first_name, last_name, security_group_ids: Vec<String>, is_admin: bool, suspended: bool, status: i32 /* 1 pending, 2 active */, invite_code: Option<String>, invite_code_ttl: Option<i32>, code_validation: Option<bool>, last_activity: Option<i64>, last_login: Option<i64> }`
- `security_groups: BTreeMap<String /* networkId */, BTreeMap<String /* groupId */, SecurityGroupRecord { group_id, name, settings, user_ids: Vec<String> }>>` (bootstrap only needs the auto-seeded default group)
- `next_network_id: u32` / `next_user_id: u64` counters so minted ids are unique and stored, plus `username_index: BTreeMap<(networkId, username), userId>` for duplicate detection
- `idempotency: BTreeMap<(operation, clientToken), serialized response>` for the `@idempotencyToken` header on BatchCreateUser/BatchDeleteUser/DeleteNetwork/CreateSecurityGroup

Done = an integration test drives CreateNetwork -> GetNetwork -> ListNetworks -> BatchCreateUser -> ListUsers -> DeleteNetwork end to end against one running emulator and asserts the response BODIES (minted ids, echoed-from-storage names, user list contents, 404 after delete), not just 2xx.

## Evidence

`services/service-com-amazonaws-wickr-wickradminapi/OPERATIONS.md` lists all 44 operations with their routes, members and errors; the routes/fields quoted above come from its CreateNetwork (`POST /networks`), GetNetwork (`GET /networks/{networkId}`, @readonly), ListNetworks (`GET /networks`, @paginated items=networks), BatchCreateUser (`POST /networks/{networkId}/users`), ListUsers (`GET /networks/{networkId}/users`, @paginated items=users) and DeleteNetwork (`DELETE /networks/{networkId}`, @idempotent) sections, and every one declares `ResourceNotFoundError (HTTP 404)` and `ValidationError (HTTP 422)`. Shape details for `Network`, `User` and `BatchCreateUserRequestItem` (including the model's note that a user may belong to only ONE security group, and status 1=pending/2=active) come from smithy_query on the model at `services/service-com-amazonaws-wickr-wickradminapi/smithy-model/com-amazonaws-wickr-wickradminapi.json`. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-wickr-wickradminapi/src/ops/` matches all 44 handler files (e.g. `create_network.rs:4`, `get_network.rs:4`, `list_networks.rs:4`, `batch_create_user.rs:4`, `list_users.rs:4`, `delete_network.rs:4`), so there is no behavior anywhere yet and no test asserts any of this.

## Steps to Reproduce

1. Start the emulator and point an AWS client at the Wickr admin API endpoint (restJson1, signing name `wickr`).
2. POST /networks with {"networkName":"demo-net","accessLevel":"STANDARD"} — expect 200 with a fresh 8-digit `networkId`; today it returns HTTP 501 NotImplemented.
3. GET /networks/{networkId} — expect 200 echoing the stored networkName `demo-net`, accessLevel STANDARD, awsAccountId and networkArn.
4. GET /networks — expect the new network present in `networks`; repeat with maxResults=1 and follow `nextToken` to page through all networks exactly once each.
5. POST /networks/{networkId}/users with users=[{"username":"alice@example.com","firstName":"Alice","securityGroupIds":["0"]}] — expect 200 with the user in `successful` carrying a minted numeric `userId` and status 1.
6. Repeat the same BatchCreateUser call — expect the duplicate username in `failed`, not a second user.
7. GET /networks/{networkId}/users — expect exactly one user, alice@example.com, with the stored firstName; also GET with username=alice@example.com and confirm filtering works.
8. DELETE /networks/{networkId} — expect 200 with a message.
9. GET /networks/{networkId} again — expect ResourceNotFoundError with HTTP 404, and GET /networks no longer lists it.
10. GET /networks/123 (malformed, not 8 digits) — expect ValidationError HTTP 422, not 501 and not 200.
