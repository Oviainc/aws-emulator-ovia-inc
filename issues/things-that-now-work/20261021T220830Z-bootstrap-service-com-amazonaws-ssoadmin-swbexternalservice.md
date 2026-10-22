Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the SSO Admin (SWBExternalService) emulator

_Filed: 2026-10-21T22:08:30Z_
_Operation: service-com-amazonaws-ssoadmin-swbexternalservice | Class: missing_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 79 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. Implement a FIRST, SMALL slice: the permission-set lifecycle plus one account assignment, all backed by real persisted state.

Wire protocol (README/OPERATIONS.md): `awsJson1_1`, `POST /` with `X-Amz-Target: SWBExternalService.<OperationName>`, sigv4 signing name `sso`.

Happy path to implement, in order (fields from OPERATIONS.md):

0. **ListInstances** (prerequisite read; `@paginated(inputToken=NextToken, outputToken=NextToken, items=Instances)`) — Input: `MaxResults` (1..100), `NextToken`. Output: `Instances` (list<InstanceMetadata>), `NextToken`. Must return at least one seeded instance from state with `InstanceArn` (matching `^arn:aws:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$`), `IdentityStoreId`, `OwnerAccountId`, `Status=ACTIVE`, `CreatedDate`. Every other operation's required `InstanceArn` must be this exact value; an unknown `InstanceArn` must produce `ResourceNotFoundException` (404), not a 200.

1. **CreatePermissionSet** — Input: `Name` (required, 1..32, `^[\w+=,.@-]+$`), `InstanceArn` (required), `Description`, `SessionDuration` (ISO-8601 duration), `RelayState`, `Tags`. Output: `PermissionSet` structure. Must mint a NEW `PermissionSetArn` of the form `arn:aws:sso:::permissionSet/<ins-id>/ps-<16 chars>`, store the record (Name, Description, SessionDuration, RelayState, CreatedDate, Tags) under that ARN, and return the stored record — not an echo of the request alone (CreatedDate and PermissionSetArn are server-generated). Duplicate `Name` in the same instance -> `ConflictException` (409).

2. **DescribePermissionSet** — Input: `InstanceArn`, `PermissionSetArn` (both required). Output: `PermissionSet` {Name, PermissionSetArn, Description, CreatedDate, SessionDuration, RelayState}. Must be read back from the stored record created in step 1, byte-identical on every field that was set. An ARN that was never created, or one already deleted, -> `ResourceNotFoundException` (404).

3. **ListPermissionSets** (`@paginated(items=PermissionSets)`) — Input: `InstanceArn` (required), `MaxResults` (1..100), `NextToken` (0..2048, `^[-a-zA-Z0-9+=/_]*$`). Output: `PermissionSets` (list of ARN strings), `NextToken`. Must enumerate exactly the ARNs currently stored for that instance, honour `MaxResults` as a page size, and emit a `NextToken` that resumes the enumeration (and omit it on the last page).

4. **CreateAccountAssignment** — Input: `InstanceArn`, `TargetId` (12-digit account), `TargetType` (`AWS_ACCOUNT`), `PermissionSetArn`, `PrincipalType` (`USER`|`GROUP`), `PrincipalId` (GUID) — all required. Output: `AccountAssignmentCreationStatus` (AccountAssignmentOperationStatus) with `Status` (StatusValues; return `SUCCEEDED` for the emulator), `RequestId` (a real 36-char UUID), `TargetId`, `TargetType`, `PermissionSetArn`, `PrincipalType`, `PrincipalId`, `CreatedDate`. Must persist the assignment AND the operation-status record keyed by `RequestId` (so a later `DescribeAccountAssignmentCreationStatus` has something to read). Unknown `PermissionSetArn` -> `ResourceNotFoundException` (404).

5. **ListAccountAssignments** (`@paginated(items=AccountAssignments)`) — Input: `InstanceArn`, `AccountId`, `PermissionSetArn` (all required), `MaxResults`, `NextToken`. Output: `AccountAssignments` (list<AccountAssignment> {AccountId, PermissionSetArn, PrincipalType, PrincipalId}), `NextToken`. Must return exactly the assignment created in step 4, filtered by (AccountId, PermissionSetArn), and an EMPTY list (not an error) when nothing is assigned.

6. **DeletePermissionSet** — Input: `InstanceArn`, `PermissionSetArn` (both required). Output: no members. Must remove the stored record so that a following `DescribePermissionSet` returns `ResourceNotFoundException` (404) and `ListPermissionSets` no longer contains the ARN. A second delete of the same ARN -> `ResourceNotFoundException` (404).

NEVER STUBS: each of these must compute its response from the shared engine store reached via `crate::state::service_state(&*self.engine)`. Hardcoded ARNs, echoed inputs, and random IDs that are never stored are failures — a create followed by a describe must return what was created, and a delete must be observable by the next reader. Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line in each implemented handler so the census reflects progress.

## State Model

In `src/state.rs` (`ComAmazonawsSsoadminSwbexternalserviceState`, reached through the one shared engine):

- `instances: Map<InstanceArn -> Instance { instance_arn, identity_store_id, identity_store_arn, owner_account_id, name, created_date, status: InstanceStatus, primary_region, regions: Vec<RegionMetadata> }>` — seeded with one ACTIVE instance at startup so `InstanceArn`-requiring calls have a valid target.
- `permission_sets: Map<PermissionSetArn -> PermissionSet { name, permission_set_arn, instance_arn, description, created_date, session_duration, relay_state, tags: Vec<Tag>, managed_policies: Vec<String>, inline_policy: Option<String> }>` — keyed by generated ARN; `name` must be unique per instance (enforce ConflictException). `managed_policies`/`inline_policy` reserved for the later slices.
- `account_assignments: Set/Vec<AccountAssignment { instance_arn, account_id (TargetId), target_type, permission_set_arn, principal_type, principal_id, created_date }>` — the tuple (instance, account, permission set, principal type, principal id) is the identity; re-creating an existing tuple must be idempotent rather than duplicating a row.
- `assignment_operations: Map<RequestId(UUID) -> AccountAssignmentOperationStatus { status, request_id, failure_reason, target_id, target_type, permission_set_arn, principal_type, principal_id, created_date }>` — written by CreateAccountAssignment (and later DeleteAccountAssignment) so the Describe*/List*Status reads are backed by real records.
- An ARN/ID minting helper producing spec-shaped identifiers: `ins-` (16 chars), `ps-<16>` under the instance, and 36-char UUIDs for `RequestId`, each matching the model's pattern constraints.

Everything outside this list (applications, trusted token issuers, inline/managed policy attachment, instance access-control attributes, regions, tagging, provisioning status) stays stubbed at 501 for now and will be filed as individual gap issues.

## Evidence

All 79 handlers under `services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/` match `^//! SCAFFOLDED, NOT IMPLEMENTED:` (grep over the ops directory returns every file, including `create_permission_set.rs`, `describe_permission_set.rs`, `list_permission_sets.rs`, `create_account_assignment.rs`, `list_account_assignments.rs`, `delete_permission_set.rs`, `list_instances.rs`), so every call answers HTTP 501 `NotImplemented` and nothing is persisted. Operation contracts cited above come from `services/service-com-amazonaws-ssoadmin-swbexternalservice/OPERATIONS.md` (input/output members, required flags, length/pattern constraints, `@paginated` traits, declared errors incl. ResourceNotFoundException 404 / ConflictException 409 / ValidationException 400); the `PermissionSet`, `InstanceMetadata`, `AccountAssignment` and `AccountAssignmentOperationStatus` member lists come from the Smithy model (`smithy_query query=shape`). Wire protocol (`awsJson1_1`, `X-Amz-Target: SWBExternalService.<Op>`) from OPERATIONS.md's "Wire protocol" section; state access pattern from the crate README's "State" section. No tests exist beyond the generated `tests/state_roundtrip.rs` example.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `sso-admin` client at it.
2. `ListInstances` -> expect one seeded instance with an ACTIVE status and a valid `InstanceArn`; capture that ARN. Today: HTTP 501 NotImplemented.
3. `CreatePermissionSet --instance-arn <arn> --name DemoSet --description 'demo' --session-duration PT1H` -> expect a `PermissionSet` with a freshly minted `PermissionSetArn` and a server-set `CreatedDate`; capture the ARN.
4. `DescribePermissionSet --instance-arn <arn> --permission-set-arn <psArn>` -> expect Name=DemoSet, Description='demo', SessionDuration=PT1H and the same CreatedDate returned by the create call.
5. `ListPermissionSets --instance-arn <arn> --max-results 1` -> expect the created ARN present and pagination honoured (NextToken only when more pages remain).
6. `CreateAccountAssignment --instance-arn <arn> --target-id 123456789012 --target-type AWS_ACCOUNT --permission-set-arn <psArn> --principal-type USER --principal-id f81d4fae-7dec-11d0-a765-00a0c91e6bf6` -> expect `AccountAssignmentCreationStatus` echoing those fields with a real UUID `RequestId` and a terminal status.
7. `ListAccountAssignments --instance-arn <arn> --account-id 123456789012 --permission-set-arn <psArn>` -> expect exactly one AccountAssignment with PrincipalType=USER and that PrincipalId.
8. `DeletePermissionSet --instance-arn <arn> --permission-set-arn <psArn>` -> expect success, then `DescribePermissionSet` on the same ARN -> expect `ResourceNotFoundException` (HTTP 404) and `ListPermissionSets` no longer containing the ARN.
9. Repeat the whole script against a restarted-but-same-process client to confirm state is shared across requests (one engine), not per-request.
