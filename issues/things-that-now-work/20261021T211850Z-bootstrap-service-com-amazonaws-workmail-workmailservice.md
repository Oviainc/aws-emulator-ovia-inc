Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the WorkMail emulator

_Filed: 2026-10-21T21:18:50Z_
_Operation: service-com-amazonaws-workmail-workmailservice | Class: stub_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in `service-com-amazonaws-workmail-workmailservice` is an unmodified scaffold: each `src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and `src/state.rs` holds no WorkMail state. Nothing works end to end.

Implement a FIRST, SMALL, stateful slice: the organization + user lifecycle, which is the core WorkMail workflow (an organization owns users; users are the primary entity every other feature hangs off).

### Happy path to implement (in order)

1. **CreateOrganization** — input `Alias` (required, 1..62, pattern `^(?!d-)([\da-zA-Z]+)([-][\da-zA-Z]+)*$`), `DirectoryId`, `Domains` (0..5), `KmsKeyArn`, `EnableInteroperability`, `ClientToken` (@idempotencyToken). Output `OrganizationId` — must be a NEWLY generated id matching `^m-[0-9a-f]{32}$` and stored. Reject a second organization with the same alias with `NameAvailabilityException`.
2. **DescribeOrganization** — input `OrganizationId` (required). Output must be read back from stored state: `OrganizationId`, `Alias`, `State` ("Active"), `DirectoryId`, `DirectoryType`, `DefaultMailDomain` (first entry of `Domains` when supplied), `CompletedDate`, `ARN` (`arn:aws:workmail:<region>:<account>:organization/<OrganizationId>`), `InteroperabilityEnabled` (as created). Unknown id -> `OrganizationNotFoundException` (HTTP 400), NOT 501 and not an empty 200.
3. **CreateUser** — input `OrganizationId` (required), `Name` (required), `DisplayName` (required), `Password`, `Role`, `FirstName`, `LastName`, `HiddenFromGlobalAddressList`. Output `UserId` — newly generated, 12..256 chars, persisted under the organization. Unknown organization -> `OrganizationNotFoundException`; duplicate `Name` within the organization -> `NameAvailabilityException`.
4. **DescribeUser** — input `OrganizationId`, `UserId` (both required). Output computed from the stored user: `UserId`, `Name`, `DisplayName`, `FirstName`, `LastName`, `Email` (empty until registered), `State` (`DISABLED` after create, `ENABLED` after RegisterToWorkMail), `UserRole` (defaulting to `USER`), `EnabledDate`/`DisabledDate`, `HiddenFromGlobalAddressList`. Unknown user -> `EntityNotFoundException` (HTTP 400).
5. **ListUsers** — input `OrganizationId` (required), `MaxResults` (1..100), `NextToken`, `Filters`. Output `Users` (list<User>: `Id`, `Email`, `Name`, `DisplayName`, `State`, `UserRole`, `EnabledDate`, `DisabledDate`) built from stored users of that organization only, honoring `MaxResults` and returning/consuming an opaque `NextToken` when more remain.
6. **DeleteUser** — input `OrganizationId`, `UserId` (both required), empty output. Removes (or marks `DELETED`) the stored user so a following `DescribeUser` fails with `EntityNotFoundException` and `ListUsers` no longer returns it. Idempotent per the model's `@idempotent` trait.

Optional 7th if cheap: **RegisterToWorkMail** (`OrganizationId`, `EntityId`, `Email`) flipping the user's `State` to `ENABLED`, setting `Email` and `EnabledDate`, with `EmailAddressInUseException` on a duplicate address — this is what makes `DescribeUser`'s state transition observable.

**No canned or echoed responses.** Every one of these must compute its output from the shared engine store reached via `crate::state::service_state(&*self.engine)`. A generated id that is not stored, an echoed input, or a hardcoded `State` field is a failure of this issue.

## State Model

In `src/state.rs`, on the service state struct:

- `organizations: HashMap<String /*OrganizationId m-<32 hex>*/, Organization>`
- `alias_index: HashMap<String /*Alias*/, String /*OrganizationId*/>` (uniqueness for `NameAvailabilityException`)
- `Organization { organization_id: String, alias: String, state: String /*"Active"*/, directory_id: Option<String>, directory_type: Option<String>, domains: Vec<Domain>, default_mail_domain: Option<String>, kms_key_arn: Option<String>, interoperability_enabled: bool, arn: String, completed_date: SystemTime, error_message: Option<String>, users: HashMap<String /*UserId*/, UserRecord>, user_name_index: HashMap<String /*Name*/, String /*UserId*/>, email_index: HashMap<String /*Email lowercased*/, String /*UserId*/> }`
- `UserRecord { user_id: String, name: String, display_name: String, email: Option<String>, first_name: Option<String>, last_name: Option<String>, role: UserRole /*default USER*/, state: EntityState /*DISABLED on create, ENABLED after RegisterToWorkMail, DELETED after DeleteUser*/, hidden_from_global_address_list: bool, enabled_date: Option<SystemTime>, disabled_date: Option<SystemTime>, password: Option<String> /*never echoed back*/ }`
- `client_tokens: HashMap<String /*ClientToken*/, String /*resulting id*/>` so the `@idempotencyToken` inputs on CreateOrganization/CreateUser replay the same id instead of creating a duplicate.

List pagination tokens should encode a stable position (e.g. index into the sorted user-id list) so `NextToken` round-trips.

## Out of scope

Everything else (groups, resources, aliases, mailbox permissions, impersonation roles, access-control rules, mail domains, export jobs, retention policies, tagging, mobile device access, identity provider / Identity Center, personal access tokens) stays 501 and will be filed as individual gap issues later.

## Done when

An integration test can run CreateOrganization -> DescribeOrganization -> CreateUser -> DescribeUser -> ListUsers -> DeleteUser -> DescribeUser(expect `EntityNotFoundException`) against a single running emulator and every response body reflects what was actually created — not just 2xx status codes.

## Evidence

Spec: `services/service-com-amazonaws-workmail-workmailservice/smithy-model/com-amazonaws-workmail-workmailservice.json`, denormalized in the crate's `OPERATIONS.md`: CreateOrganization (line 94: required `Alias`, output `OrganizationId` pattern `^m-[0-9a-f]{32}$`, errors incl. `NameAvailabilityException`), CreateUser (line 108: required `OrganizationId`/`Name`/`DisplayName`, output `UserId`), DescribeOrganization (line 279: full output member list incl. `State`, `ARN`, `DefaultMailDomain`, `InteroperabilityEnabled`), DescribeUser (line 293: output incl. `State` (DELETED|DISABLED|ENABLED), `UserRole`, `Email`, timestamps), ListUsers (line 488: `@paginated(inputToken=NextToken, outputToken=NextToken)`, `MaxResults` 1..100, output `Users` list<User>), DeleteUser (line 216: `@idempotent`, errors incl. `EntityNotFoundException` path via DescribeUser), RegisterToWorkMail (line 549: `EntityId`+`Email`, `EmailAddressInUseException`). `User` and `OrganizationSummary` member lists confirmed via smithy_query. Implementation: all 94 files under `src/ops/` carry `//! SCAFFOLDED, NOT IMPLEMENTED:` — e.g. `src/ops/create_organization.rs:4` with `Err(crate::scaffolded::Scaffolded::not_implemented("CreateOrganization"))` at line 185. No tests assert any WorkMail behavior (only the generated `tests/state_roundtrip.rs`).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `workmail` at it.
2. `aws workmail create-organization --alias bootstrap-org --no-enable-interoperability` — currently HTTP 501 NotImplemented; expected HTTP 200 with a newly minted `OrganizationId` matching ^m-[0-9a-f]{32}$.
3. `aws workmail describe-organization --organization-id <id>` — expected the stored alias, State=Active and the organization ARN; an unknown id must give OrganizationNotFoundException (400).
4. `aws workmail create-user --organization-id <id> --name alice --display-name 'Alice A' --password 'Passw0rd!'` — expected HTTP 200 with a persisted `UserId`.
5. `aws workmail describe-user --organization-id <id> --user-id <uid>` — expected Name=alice, DisplayName='Alice A', State=DISABLED (ENABLED once RegisterToWorkMail runs).
6. `aws workmail list-users --organization-id <id> --max-results 1` — expected only that organization's users, at most 1 per page plus a usable NextToken.
7. `aws workmail delete-user --organization-id <id> --user-id <uid>`, then repeat describe-user — expected EntityNotFoundException (400) and the user absent from list-users.
