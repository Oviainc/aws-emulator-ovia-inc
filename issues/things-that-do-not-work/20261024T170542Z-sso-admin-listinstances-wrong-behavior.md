Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6f4880dda1aa7f89588445b4a5ba9a411741294f

# Seeded instance is an ACCOUNT instance, so the permission-set surface is unreachable over the wire

_Filed: 2026-10-24T17:05:41Z_
_Operation: ListInstances | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6f4880dda1aa7f89588445b4a5ba9a411741294f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

> **Superseded, do not fix as written.** The behavioral ground truth recorded for this
> service (checks `sso-admin.abac-configuration-lifecycle` and
> `sso-admin.account-assignment-full-lifecycle`) shows the opposite: in the recorded
> account the instance `ListInstances` hands out IS an account instance, and the
> permission-set / account-assignment / ABAC surface is refused on it with HTTP 400
> `ValidationException`. The seeding this issue asks for was applied and has since been
> reverted to match that recording; `UpdateInstance {PermissionSetsEnabled: true}` is the
> documented call that makes the surface reachable over the wire on that instance, which
> `tests/all/discovered_instance_surface.rs` now pins end to end. Flipping the seed back
> would re-break the two recorded checks.

The single instance this emulator reports for the account is seeded as an IAM Identity Center *account* instance (`state::SEED_INSTANCE_TYPE = InstanceType::Account`), i.e. `DescribeInstance`/`ListInstances` report it with `PermissionSetsEnabled: false`. Because `InstanceType::permission_sets_enabled()` is only true for `Organization`, `ComAmazonawsSsoadminSwbexternalserviceState::permission_set_surface_withdrawn` / `abac_surface_withdrawn` return true for that instance, and EVERY implemented operation of the permission-set surface refuses it up front with HTTP 400 `ValidationException` ("This operation is not supported for account instances of IAM Identity Center.") before any lookup.

Expected (real AWS, and the normal client workflow): `ListInstances` -> take the returned `InstanceArn` -> `CreatePermissionSet` -> `DescribePermissionSet` -> `CreateAccountAssignment` -> `ListAccountAssignments` all succeed with 200. In recorded AWS environments for these operations the instance returned by `ListInstances` is an organization instance with `PermissionSetsEnabled: true`.

Actual: against a fresh emulator process, the very first call of that workflow after `ListInstances` answers 400 `ValidationException`. The operations made unusable out of the box are: CreatePermissionSet, DescribePermissionSet, ListPermissionSets, DeletePermissionSet, CreateAccountAssignment, ListAccountAssignments, AttachManagedPolicyToPermissionSet, DetachManagedPolicyFromPermissionSet, ListManagedPoliciesInPermissionSet, AttachCustomerManagedPolicyReferenceToPermissionSet, DetachCustomerManagedPolicyReferenceFromPermissionSet, ListCustomerManagedPolicyReferencesInPermissionSet, PutPermissionsBoundaryToPermissionSet, GetPermissionsBoundaryForPermissionSet, and all four InstanceAccessControlAttributeConfiguration operations — i.e. ~15 of the 37 implemented operations, including the whole lifecycle the (already fixed) bootstrap issue specified as the service's happy path.

The only wire path out of the refusal today is an extra, non-obvious `UpdateInstance {PermissionSetsEnabled: true}` call; the crate's own integration tests cannot reach the surface over the wire at all and instead inject an `InstanceType::Organization` instance directly into the store (`tests/all/ssoadmin_permission_sets.rs::organization_engine`).

NOTE: the account-instance restriction itself is correct and should stay — what is wrong is that the instance the service seeds and reports by default is of that kind. The seeded/ambient instance should behave like the instance a client normally finds (permission sets enabled), with account instances reachable through `CreateInstance` (which already stores `InstanceType::Account`).

## Evidence

Spec/doc grounding: `DescribeInstance$PermissionSetsEnabled` ("Indicates whether permission sets are enabled for this Identity Center instance") and the permission-set/account-assignment operations of the model (`CreatePermissionSet`, `CreateAccountAssignment`, `ListAccountAssignments`, …) are the service's core surface; AWS documents permission sets as unsupported on account instances, and the instance returned by `ListInstances` in a permission-set workflow is the organization instance. The already-fixed bootstrap issue for this crate specified the required happy path as `ListInstances` -> `CreatePermissionSet` -> `DescribePermissionSet` -> `ListPermissionSets` -> `CreateAccountAssignment` -> `ListAccountAssignments` -> `DeletePermissionSet`, all against the seeded instance.

Code: `src/state.rs` — `pub const SEED_INSTANCE_TYPE: InstanceType = InstanceType::Account;` used by `enable_ambient_instance`, plus `InstanceType::permission_sets_enabled()` (true only for `Organization`) and `permission_set_surface_withdrawn`/`abac_surface_withdrawn`. Handlers that short-circuit on it: `create_permission_set.rs` (l.193), `describe_permission_set.rs` (l.170), `list_permission_sets.rs` (l.191), `delete_permission_set.rs` (l.173), `create_account_assignment.rs` (l.205), `list_account_assignments.rs` (l.201), `attach_managed_policy_to_permission_set.rs` (l.184), `detach_managed_policy_from_permission_set.rs` (l.180), `list_managed_policies_in_permission_set.rs` (l.202), `attach_customer_managed_policy_reference_to_permission_set.rs` (l.187), `detach_customer_managed_policy_reference_from_permission_set.rs` (l.185), `list_customer_managed_policy_references_in_permission_set.rs` (l.199), `put_permissions_boundary_to_permission_set.rs` (l.194), `get_permissions_boundary_for_permission_set.rs` (l.170), and the four ABAC handlers.

Test coverage status: no test asserts the seeded instance can serve the permission-set surface — the opposite is asserted (`tests/all/account_instance_restriction.rs::the_withdrawn_surface_is_refused_on_an_account_instance`, `tests/all/instance_update.rs::enabling_permission_sets_opens_the_permission_set_surface` asserts `CreatePermissionSet` is 400 on the seeded instance until `UpdateInstance` enables it), and the permission-set tests bypass the wire by seeding `InstanceType::Organization` straight into state (`tests/all/ssoadmin_permission_sets.rs` l.117-151).

## Steps to Reproduce

1. Start the emulator with a fresh (empty) engine and point an `sso-admin` client at it.
2. Call `ListInstances` and capture the single returned `InstanceArn` (HTTP 200).
3. Call `DescribeInstance` with that ARN: observe `PermissionSetsEnabled: false` (real AWS reports `true` for the instance a permission-set workflow runs against).
4. Call `CreatePermissionSet {InstanceArn: <that ARN>, Name: "DemoSet"}`: observe HTTP 400 `ValidationException` with message "This operation is not supported for account instances of IAM Identity Center." instead of HTTP 200 with a minted `PermissionSetArn`.
5. Repeat with `ListPermissionSets`, `CreateAccountAssignment`, `ListAccountAssignments`, `AttachManagedPolicyToPermissionSet`, `PutPermissionsBoundaryToPermissionSet`, `CreateInstanceAccessControlAttributeConfiguration` on the same ARN: every one answers 400 rather than operating on state.
6. Call `UpdateInstance {InstanceArn: <that ARN>, PermissionSetsEnabled: true}` and retry step 4: it now returns 200, showing the refusal comes solely from the seeded instance's kind.
