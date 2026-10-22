Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the IAM emulator

_Filed: 2026-10-21T21:08:33Z_
_Operation: service-com-amazonaws-iam-awsidentitymanagementv20100508 | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 180 operations in `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, and the crate holds no state. Nothing works end-to-end.

This issue defines the FIRST, SMALL slice: the IAM user lifecycle plus one inline policy, implemented statefully over the awsQuery protocol (`sdkId: IAM`, signing name `iam`). Implement these 6 operations, in this order:

1. **CreateUser** — input `UserName` (required, 1..64, `^[\w+=,.@-]+$`), `Path` (optional, default `/`), `PermissionsBoundary`, `Tags`. Output `User`. Must mint and persist a `User` with ALL required members of the `User` shape: `Path`, `UserName`, `UserId` (stable, unique, 16..128, `^[\w]+$` — e.g. `AIDA` + 17 uppercase alphanumerics), `Arn` (`arn:aws:iam::<account-id>:user<path><UserName>`), `CreateDate` (now), plus `Tags`/`PermissionsBoundary` when supplied. Creating a name that already exists (IAM names are case-insensitive for uniqueness) must return `EntityAlreadyExistsException` (HTTP 409), not a second user.
2. **GetUser** — input `UserName` (optional; when absent, resolve the caller — a stable default user is acceptable). Output `User` (required). Must return the EXACT stored record from step 1 — same `UserId`, `Arn`, `Path`, `CreateDate`, `Tags` — never a value recomputed from the request. Unknown name → `NoSuchEntityException` (HTTP 404).
3. **PutUserPolicy** — input `UserName`, `PolicyName`, `PolicyDocument` (all required). Empty output. Must store the document verbatim under (user, policy name), overwriting on repeat. Unknown user → `NoSuchEntityException` (404). A `PolicyDocument` that is not parseable JSON → `MalformedPolicyDocumentException` (400).
4. **GetUserPolicy** — input `UserName`, `PolicyName`. Output `UserName`, `PolicyName`, `PolicyDocument` (all required). Must return the byte-identical document stored in step 3; unknown user or unknown policy name → `NoSuchEntityException` (404).
5. **ListUsers** — input `PathPrefix`, `Marker` (1..320), `MaxItems` (1..1000). Output `Users` (required), `IsTruncated` (required, default false), `Marker`. Must enumerate the persisted users, filter by `PathPrefix`, honour `MaxItems` as a real page size, and set `IsTruncated=true` plus an opaque `Marker` that a follow-up call resumes from without repeating or skipping users. `@paginated(inputToken=Marker, outputToken=Marker, items=Users)`.
6. **DeleteUser** — input `UserName` (required). Empty output. Must remove the user from state so a following `GetUser` returns `NoSuchEntityException` (404) and `ListUsers` omits it. A user that still has inline policies (or attached policies/access keys) must be refused with `DeleteConflictException` (409) rather than silently deleted; unknown user → `NoSuchEntityException` (404).

NEVER STUBS: each of these six must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)`. Hardcoded bodies, echoing the input back, or generating an ID without storing it are failures of this issue — a `CreateUser` must be observable by the later `GetUser`/`ListUsers`, and a `DeleteUser` must be observable as absence.

## State Model

In `src/state.rs`, on the service state struct:

- `account_id: String` — the 12-digit account used to build every ARN (fixed, e.g. `000000000000`).
- `users: BTreeMap<String /* lowercased UserName, for case-insensitive uniqueness */, User>` where `User { user_name: String /* original case */, user_id: String, arn: String, path: String /* default "/" */, create_date: SystemTime, password_last_used: Option<SystemTime>, permissions_boundary: Option<String>, tags: Vec<Tag> }`. `BTreeMap` so `ListUsers` has a stable, resumable order for the `Marker`.
- `user_inline_policies: BTreeMap<String /* lowercased UserName */, BTreeMap<String /* PolicyName */, String /* PolicyDocument verbatim */>>` — read by `GetUserPolicy`, and consulted by `DeleteUser` for the `DeleteConflictException` check.
- `user_id_seq: u64` — monotonic counter feeding unique `UserId` generation.

`Tag { key: String, value: String }`. Pagination `Marker`: encode the last-returned map key so the next page is `range((Excluded(key), Unbounded))`.

All 174 other operations (roles, groups, managed policies, access keys, MFA, OIDC/SAML providers, instance profiles, tagging, Simulate*) stay at 501 and are tracked as individual gap issues later. Keep this first slice to the six operations above.

## Evidence

`services/service-com-amazonaws-iam-awsidentitymanagementv20100508/OPERATIONS.md` line 3 ("180 operations") and lines 8-14 (protocol `awsQuery`, SDK id `IAM`, signing name/endpoint prefix `iam`) plus the per-operation contracts cited above: CreateUser (line 164-168), DeleteUser (296-300), DeleteUserPolicy (308-312), GetUser (574-578), GetUserPolicy (580-584), ListUsers (811-816), PutUserPolicy (862-866). The `User` output shape's required members (`Path`, `UserName`, `UserId`, `Arn`, `CreateDate`) come from `smithy_query(query="shape", shape="User")`. The all-stub state is confirmed by `src/ops/create_user.rs` lines 4 and 192: the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and `Err(crate::scaffolded::Scaffolded::not_implemented("CreateUser"))`; README.md lines 36-53 document that this marker is the census of unimplemented operations. No tests exist beyond the generated `tests/state_roundtrip.rs` example.

## Steps to Reproduce

1. Point an AWS SDK/CLI client at the emulator endpoint with signing name `iam`.
2. `aws iam create-user --user-name alice --path /eng/` → expect 200 with User.UserName=alice, User.Path=/eng/, a 16-128 char UserId matching ^[\w]+$, Arn=arn:aws:iam::<account>:user/eng/alice and a CreateDate. Today: HTTP 501 NotImplemented.
3. `aws iam create-user --user-name alice` again → expect HTTP 409 EntityAlreadyExistsException.
4. `aws iam get-user --user-name alice` → expect the identical UserId/Arn/Path/CreateDate returned by step 2 (not recomputed, not echoed).
5. `aws iam put-user-policy --user-name alice --policy-name S3Read --policy-document '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}'` → expect 200.
6. `aws iam get-user-policy --user-name alice --policy-name S3Read` → expect UserName=alice, PolicyName=S3Read and the byte-identical policy document from step 5.
7. `aws iam create-user --user-name bob` then `aws iam list-users --max-items 1` → expect exactly 1 user, IsTruncated=true and a Marker; re-run with `--starting-token <Marker>` and expect the OTHER user with no repeats.
8. `aws iam list-users --path-prefix /eng/` → expect only alice.
9. `aws iam delete-user --user-name alice` → expect HTTP 409 DeleteConflictException while the S3Read inline policy exists.
10. `aws iam delete-user-policy --user-name alice --policy-name S3Read` then `aws iam delete-user --user-name alice` → expect 200, then `aws iam get-user --user-name alice` → HTTP 404 NoSuchEntityException and `aws iam list-users` no longer lists alice.
