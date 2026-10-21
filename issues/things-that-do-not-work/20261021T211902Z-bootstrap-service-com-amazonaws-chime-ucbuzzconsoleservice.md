Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the com-amazonaws-chime-ucbuzzconsoleservice emulator

_Filed: 2026-10-21T21:19:02Z_
_Operation: service-com-amazonaws-chime-ucbuzzconsoleservice | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler under `services/service-com-amazonaws-chime-ucbuzzconsoleservice/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and the service holds no state. Nothing works end-to-end.

This issue defines the FIRST, SMALL slice: the Amazon Chime account + user lifecycle, which is the service's primary resource and the entry point for every other call (rooms, bots, phone numbers all hang off `/accounts/{AccountId}`). Implement these 6 operations statefully, in this order (wire protocol: `restJson1`, sigv4 signing name `chime`):

1. **CreateAccount** — `POST /accounts` -> 201. Input: `Name` (required, len 1..100, pattern `\S`). Output: `Account` with `AwsAccountId` (required), `AccountId` (required, server-generated and stable), `Name` (echoing the stored name), `AccountType`, `CreatedTimestamp` (ISO-8601 date-time, set at creation), `DefaultLicense`, `SupportedLicenses`, `AccountStatus` (e.g. `Active`), `SigninDelegateGroups`. The generated `AccountId` must be stored and reusable by every later call.
2. **GetAccount** — `GET /accounts/{AccountId}` -> 200. Output: the SAME `Account` struct that CreateAccount returned, read back from the store (same `AccountId`, `Name`, `CreatedTimestamp`). Unknown `AccountId` -> `NotFoundException` (HTTP 404).
3. **ListAccounts** — `GET /accounts` -> 200. Input: `Name` (query `name`), `UserEmail` (query `user-email`), `MaxResults` (query `max-results`, range 1..200), `NextToken` (query `next-token`). Output: `Accounts` (list<Account>), `NextToken`. Must return exactly the accounts currently in the store, filter by `Name` when supplied, truncate to `MaxResults` and emit a `NextToken` that resumes where the page ended (and no `NextToken` on the last page).
4. **CreateUser** — `POST /accounts/{AccountId}/users?operation=create` -> 201. Input: `AccountId` (label, required), `Username`, `Email` (pattern `^.+@.+\..+$`), `UserType` (`PrivateUser` | `SharedDevice`). Output: `User` with server-generated `UserId`, `AccountId` echoing the path, `PrimaryEmail` from `Email`, `DisplayName` from `Username`, `UserType`, `LicenseType`, `UserRegistrationStatus`, `UserInvitationStatus`, `PersonalPIN`. The user must be stored under its account; creating a user on an unknown account -> `NotFoundException` (404).
5. **ListUsers** — `GET /accounts/{AccountId}/users` -> 200. Input: `AccountId` (required), `UserEmail` (query `user-email`), `UserType` (query `user-type`), `MaxResults` (1..200), `NextToken`. Output: `Users` (list<User>), `NextToken`. Must list the users actually created under that account, honouring the `UserEmail`/`UserType` filters and `MaxResults`/`NextToken` paging. (GetUser — `GET /accounts/{AccountId}/users/{UserId}` -> 200 — should read the same record back and is a cheap addition in the same slice.)
6. **DeleteAccount** — `DELETE /accounts/{AccountId}` -> 204 with an empty body. The account must be removed from the store so a subsequent GetAccount returns `NotFoundException` (404) and ListAccounts no longer includes it. Deleting an unknown account -> 404.

Every one of these must compute its response from persisted state. No canned bodies, no echoing the input back without storing it, and no generating an `AccountId`/`UserId` that is never written to the store — a create followed by a read must return what was created, and a delete must be observable by the next reader. Validation declared in the model must be enforced (e.g. `Name` length 1..100 and non-blank, `Email` pattern, `MaxResults` range) and rejected with `BadRequestException` (HTTP 400); missing resources with `NotFoundException` (HTTP 404), using the modeled error shapes (`Code`/`Message`) rather than generic 500s.

## State Model

All state lives in `ComAmazonawsChimeUcbuzzconsoleserviceState` in `src/state.rs`, reached via `crate::state::service_state(&*self.engine)` and mutated under its lock (never construct a fresh engine inside a handler):

- `accounts: BTreeMap<String /* AccountId */, Account>` where `Account { aws_account_id: String, account_id: String, name: String, account_type: AccountType, created_timestamp: DateTime, default_license: License, supported_licenses: Vec<License>, account_status: AccountStatus, signin_delegate_groups: Vec<SigninDelegateGroup> }`. `BTreeMap` gives the stable ordering that pagination tokens need.
- `users: BTreeMap<(String /* AccountId */, String /* UserId */), User>` where `User { user_id: String, account_id: String, primary_email: Option<String>, primary_provisioned_number: Option<String>, display_name: Option<String>, license_type: Option<License>, user_type: Option<UserType>, user_registration_status: Option<RegistrationStatus>, user_invitation_status: Option<InviteStatus>, registered_on: Option<DateTime>, invited_on: Option<DateTime>, personal_pin: Option<String> }`.
- Counters/ID generation: a monotonic or UUID-based `next_account_id` / `next_user_id` helper, with the generated id written into the map in the same lock scope as the response it is reported in.
- Pagination: `NextToken` encodes the last-returned key of the `BTreeMap` (account id, or `(account_id, user_id)`), so the next page resumes with a range scan — no per-request snapshot stored.

Deleting an account removes its `accounts` entry and all `users` entries whose key starts with that `AccountId`.

Everything else (rooms, room memberships, bots, events configuration, phone numbers and phone-number orders, retention/global/user settings, the Batch* and Redact* operations) stays scaffolded at 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-chime-ucbuzzconsoleservice/OPERATIONS.md` documents the routes, inputs, outputs and errors cited above: CreateAccount (`POST /accounts` -> 201, Input `Name`, Output `Account`), GetAccount (`GET /accounts/{AccountId}` -> 200, Output `Account`, `NotFoundException` 404), ListAccounts (`GET /accounts` -> 200, @paginated inputToken=NextToken/outputToken=NextToken, `MaxResults` 1..200), CreateUser (`POST /accounts/{AccountId}/users?operation=create` -> 201, Output `User`), ListUsers (`GET /accounts/{AccountId}/users` -> 200, @paginated, Output `Users`+`NextToken`), DeleteAccount (`DELETE /accounts/{AccountId}` -> 204). The `Account`, `User` and `Room` member lists come from the Smithy model (`smithy_query` shape lookup: `com.amazonaws.chime#Account` requires `AwsAccountId`, `AccountId`, `Name`; `com.amazonaws.chime#User` requires `UserId`). Stub proof: `src/ops/create_account.rs` line 4 is `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 169 is `Err(crate::scaffolded::Scaffolded::not_implemented("CreateAccount"))`; all 61 files in `src/ops/` are the same generated shape. No tests exist beyond the generated `tests/state_roundtrip.rs` example.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI chime client at it.
2. `CreateAccount` with `Name="BootstrapAccount"` — expect HTTP 201 and an `Account` carrying a generated `AccountId`, `AwsAccountId`, `Name="BootstrapAccount"` and a `CreatedTimestamp`. Today: HTTP 501 NotImplemented.
3. `GetAccount` with the returned `AccountId` — expect HTTP 200 with the identical `Account` (same `AccountId`, `Name`, `CreatedTimestamp`).
4. `ListAccounts` — expect HTTP 200 and `Accounts` containing exactly that account; repeat with `max-results=1` after creating a second account and follow `NextToken` to see each account exactly once.
5. `CreateUser` on that account with `Username="alice"`, `Email="alice@example.com"`, `UserType=PrivateUser` — expect HTTP 201 with a generated `UserId`, `AccountId` equal to the path account, and `PrimaryEmail="alice@example.com"`.
6. `ListUsers` on that account — expect HTTP 200 and `Users` containing the created user with the same `UserId` and `PrimaryEmail`; `GetUser` with that `UserId` returns the same record.
7. `GetAccount`/`CreateUser` with an unknown `AccountId` — expect HTTP 404 `NotFoundException`, not 501 and not 200.
8. `DeleteAccount` with the `AccountId` — expect HTTP 204 and an empty body; a following `GetAccount` must return HTTP 404 `NotFoundException` and `ListAccounts` must no longer include it.
