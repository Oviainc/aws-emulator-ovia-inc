Issue-Severity: CRITICAL
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# Seeded instance is an ACCOUNT instance, so the whole permission-set / ABAC surface answers 400 over the wire

_Filed: 2026-10-24T15:58:00Z_
_Operation: CreatePermissionSet | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The instance every client discovers through `ListInstances` (the account's lazily seeded "ambient" instance) is created as `InstanceType::Account` with `PermissionSetsEnabled = false`. Because every operation of the permission-set / account-assignment / ABAC surface first asks `store.permission_set_surface_withdrawn(&input.instance_arn)` (or `abac_surface_withdrawn`) and refuses with `ValidationException` (HTTP 400, message "This operation is not supported for account instances of IAM Identity Center."), the emulator's DEFAULT state makes the service's flagship workflow unreachable over the wire.

Expected (real IAM Identity Center): the instance returned by `ListInstances` for an account that has Identity Center enabled is an organization instance; `DescribeInstance` reports `PermissionSetsEnabled: true` and `CreatePermissionSet` / `DescribePermissionSet` / `ListPermissionSets` / `DeletePermissionSet` / `CreateAccountAssignment` / `ListAccountAssignments` / the customer-managed-policy, managed-policy and permissions-boundary operations / the four `InstanceAccessControlAttributeConfiguration` operations all answer 200 against that ARN. A recorded AWS trace of the canonical flow is `ListInstances` -> `CreatePermissionSet(InstanceArn=<listed arn>)` -> 200 with a minted `PermissionSetArn` (this is also exactly what Terraform's `aws_ssoadmin_permission_set` and the AWS CLI do).

Actual: `CreatePermissionSet` (and ~15 sibling operations) answer HTTP 400 `ValidationException` against the only `InstanceArn` the API ever hands out. The only way to get a permission-set-capable instance is either `UpdateInstance` with `PermissionSetsEnabled=true` (a call no real client makes before creating a permission set) or direct in-process state mutation — there is NO API path that produces an organization instance, which is why the crate's own permission-set tests build one by writing `state::Instance::active(.., InstanceType::Organization, ..)` straight into the store.

Fix direction: seed the account's ambient instance as an ORGANIZATION instance (permission sets enabled), so the surface is served by default; keep the account-instance restriction for instances actually created through `CreateInstance`, which AWS documents as creating the instance of a standalone account.

## Evidence

Smithy model: `DescribeInstance$PermissionSetsEnabled` is the per-instance flag; `CreatePermissionSet` models `ResourceNotFoundException`/`ConflictException` for its failure modes, not a blanket 400 for the account's own instance. Handler/state: `src/state.rs` — `pub const SEED_INSTANCE_TYPE: InstanceType = InstanceType::Account;`, `InstanceType::permission_sets_enabled()` returns true only for `Organization`, `enable_ambient_instance()` inserts the seeded instance with `SEED_INSTANCE_TYPE`, and `permission_set_surface_withdrawn()` is `!instance.permission_sets_enabled`. Refusal sites: `src/ops/create_permission_set.rs` (~line 193), `describe_permission_set.rs`, `list_permission_sets.rs`, `delete_permission_set.rs`, `create_account_assignment.rs`, `list_account_assignments.rs`, `attach_customer_managed_policy_reference_to_permission_set.rs`, `detach_customer_managed_policy_reference_from_permission_set.rs`, `list_customer_managed_policy_references_in_permission_set.rs`, `list_managed_policies_in_permission_set.rs`, `get_permissions_boundary_for_permission_set.rs`, `put_permissions_boundary_to_permission_set.rs`, `create/describe/update/delete_instance_access_control_attribute_configuration.rs`. Test coverage: `tests/all/account_instance_restriction.rs` asserts the 400 against the seeded instance and only reaches 200 via `ssoadmin_permission_sets::organization_router()`, which bypasses the API by inserting an `InstanceType::Organization` record directly into the store (`tests/all/ssoadmin_permission_sets.rs` lines 117-151) — i.e. no test exercises the wire-level happy path the fixed bootstrap issue itself specified (ListInstances -> CreatePermissionSet -> ... -> DeletePermissionSet).

## Steps to Reproduce

1. Start the emulator with a fresh store and point an `sso-admin` client at it.
2. Call `ListInstances` with an empty body; capture `Instances[0].InstanceArn` (the seeded `arn:aws:sso:::instance/ssoins-...`).
3. Call `DescribeInstance` with that ARN: observe `PermissionSetsEnabled: false` (AWS reports `true` for the organization instance a client discovers this way).
4. Call `CreatePermissionSet` with `{InstanceArn: <that ARN>, Name: "DemoSet"}`: observe HTTP 400 `ValidationException` "This operation is not supported for account instances of IAM Identity Center." instead of 200 with a minted `PermissionSetArn`.
5. Repeat with `ListPermissionSets`, `CreateAccountAssignment`, `ListAccountAssignments`, `PutPermissionsBoundaryToPermissionSet` and `CreateInstanceAccessControlAttributeConfiguration` against the same ARN: every one answers the same 400, so no permission-set, assignment or ABAC workflow can be driven through the API.
