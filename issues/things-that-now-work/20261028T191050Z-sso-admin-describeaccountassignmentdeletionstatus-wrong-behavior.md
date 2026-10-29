Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3da2fe3b5486a725ab91e82448a29b4c21c390bb

# An assignment DELETION status keeps resolving after its permission set is deleted, although both sibling status readers of this crate withdraw a status whose permission set no reader can see

_Filed: 2026-10-28T19:10:50Z_
_Operation: DescribeAccountAssignmentDeletionStatus | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3da2fe3b5486a725ab91e82448a29b4c21c390bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate holds three status readers over assignment/provisioning history, and states one rule for all of them: a status exists to report work on one permission set, so once that permission set is gone the status stops resolving. Two of the three implement it; `DescribeAccountAssignmentDeletionStatus` (and its enumeration `ListAccountAssignmentDeletionStatus`) does not.

* Creation statuses: `DeletePermissionSet` physically removes the `assignment_operations` records of the assignments it drops, with the reason stated in the handler — "keeping it after that assignment is gone would both report work on a resource no reader can see and hold store capacity for it forever" — and `assignment_operation`'s doc restates it: "A record whose permission set has gone is absent too". A test pins exactly this (`a_status_goes_with_the_permission_set_its_operation_named`).
* Provisioning statuses: `permission_set_provisioning` filters every lookup on the permission set still existing — "absent once the permission set it reports on has gone: a status exists to report the provisioning of one permission set, so an id whose permission set no reader can see is as absent as one that was never minted" — and `tracked_permission_set_provisionings` delegates to it so the describe and the list cannot disagree.
* Deletion statuses: `assignment_deletion` applies no such filter, `DeletePermissionSet` never touches `assignment_deletions`, and `ListAccountAssignmentDeletionStatus` filters only on `instance_arn`. So after `DeletePermissionSet`, `DescribeAccountAssignmentDeletionStatus` still answers 200 with `PermissionSetArn` naming a permission set every other read of this crate reports as absent, and the enumeration still lists its `RequestId`.

Consequence: one `RequestId` family splits in two. Delete an assignment, then delete its permission set: the CREATION status of that assignment is 404, the PROVISIONING statuses of that permission set are 404, and the DELETION status is 200 — reporting on a `PermissionSetArn` that `DescribePermissionSet`, `ListPermissionSets`, `ListAccountAssignments` and `ListAccountsForProvisionedPermissionSet` all say does not exist. Nothing in the crate states a reason for the split: the `assignment_deletions` field doc explains only why the store is SEPARATE from the creation statuses ("by the time it is stored the assignment it reports on is gone, so it cannot be held beside that assignment"), and the provisioning history — also documented as "history" — is nonetheless withdrawn with its permission set.

## Evidence

THE RULE, stated for the creation statuses — services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/delete_permission_set.rs:216-236:
```
        store.permission_sets.remove(&input.permission_set_arn);
        // The assignments that referenced the deleted permission set go with it, and so
        // do their creation-status records: a status record exists to report on one
        // assignment, so keeping it after that assignment is gone would both report
        // work on a resource no reader can see and hold store capacity for it forever.
        ...
        for request_id in removed {
            store.assignment_operations.remove(&request_id);
        }
```
(nothing in this handler touches `store.assignment_deletions`)

and restated on the reader — src/state.rs:3313-3330:
```
    /// The status record of the assignment operation `request_id` names, WITHIN
    /// `instance_arn`.
    ///
    /// ... A record whose permission set
    /// has gone is absent too, because the delete removes it with the assignments it
    /// reported on ([`Self::remove_instance`], `DeletePermissionSet`).
    pub fn assignment_operation(
```

THE SAME RULE, implemented as a FILTER for the provisioning statuses — src/state.rs:2772-2791:
```
    /// The status record of the permission-set provisioning `request_id` names, WITHIN
    /// `instance_arn`.
    ///
    /// Scoped to the instance exactly as [`Self::assignment_operation`] is, and absent
    /// once the permission set it reports on has gone: a status exists to report the
    /// provisioning of one permission set, so an id whose permission set no reader can
    /// see is as absent as one that was never minted.
    pub fn permission_set_provisioning(
        &self,
        instance_arn: &str,
        request_id: &str,
    ) -> Option<&PermissionSetProvisioningRecord> {
        self.permission_set_provisionings
            .iter()
            .find(|record| record.instance_arn == instance_arn && record.request_id == request_id)
            .filter(|record| {
                self.permission_set(instance_arn, &record.permission_set_arn)
                    .is_some()
            })
    }
```

THE OUTLIER — src/state.rs:3178-3191, no permission-set filter at all:
```
    /// The stored status of the assignment deletion `request_id` names WITHIN
    /// `instance_arn`.
    ///
    /// Scoped to the instance exactly as [`Self::assignment_operation`] is: a `RequestId`
    /// of another instance's deletion is as absent here as one that never ran.
    pub fn assignment_deletion(
        &self,
        instance_arn: &str,
        request_id: &str,
    ) -> Option<&AssignmentOperationRecord> {
        self.assignment_deletions
            .iter()
            .find(|record| record.instance_arn == instance_arn && record.request_id == request_id)
    }
```
consumed unchanged by the describe — src/ops/describe_account_assignment_deletion_status.rs:204-226:
```
        // Answered from the STORED record the deletion wrote: a `RequestId` no deletion
        // of this instance ran under names no operation, which is the same 404 an id
        // belonging to another instance gets.
        let stored = store
            .assignment_deletion(
                &input.instance_arn,
                &input.account_assignment_deletion_request_id,
            )
```
and by the enumeration — src/ops/list_account_assignment_deletion_status.rs:258-264, which filters on `instance_arn` and the status filter only:
```
            let mut rows: Vec<(String, AccountAssignmentOperationStatusMetadata)> = store
                .assignment_deletions
                .iter()
                .filter(|record| record.instance_arn == input.instance_arn)
                .filter(|record| filter.is_none_or(|status| record.status(now) == status))
```
The reported shape carries the dangling ARN — src/state.rs:2355-2366 (`to_shape`): `permission_set_arn: Some(self.permission_set_arn.clone())`.

TEST COVERAGE: the rule is pinned for the CREATION status only — tests/all/account_assignment_creation_status.rs:512-520:
```
/// RULE 4 — a status record belongs to the permission set its operation named: deleting
/// that permission set takes its assignments AND their status records with it, so the
/// request id stops resolving — the read reports state rather than a history of its own.
///
/// This pins the STORE's rule (`ComAmazonawsSsoadminSwbexternalserviceState::
/// assignment_operations`, `DeletePermissionSet`): the records live exactly as long as the
/// permission set they name.
```
No test in tests/all/account_assignment_deletion_status.rs or tests/all/account_assignment_deletion_status_list.rs calls `DeletePermissionSet`, so no test states a reason for the deletion status surviving it (`grep -n DeletePermissionSet tests/all/account_assignment_deletion_status*.rs` returns nothing).

## Steps to Reproduce

1. POST / with `X-Amz-Target: SWBExternalService.ListInstances`, read the `InstanceArn`, and `UpdateInstance` with `{"InstanceArn":"<instance>","PermissionSetsEnabled":true}`.
2. `CreatePermissionSet` with `{"InstanceArn":"<instance>","Name":"Doomed"}`; keep `<ps>`.
3. `CreateAccountAssignment` with `{"InstanceArn":"<instance>","TargetId":"123456789012","TargetType":"AWS_ACCOUNT","PermissionSetArn":"<ps>","PrincipalType":"USER","PrincipalId":"11111111-2222-3333-4444-555555555555"}`; keep the creation `RequestId` as `<create-id>`.
4. `DeleteAccountAssignment` with the same five members; keep the deletion `RequestId` as `<delete-id>`.
5. `DeletePermissionSet` with `{"InstanceArn":"<instance>","PermissionSetArn":"<ps>"}` — 200.
6. `DescribeAccountAssignmentCreationStatus` with `{"InstanceArn":"<instance>","AccountAssignmentCreationRequestId":"<create-id>"}` — 404 `ResourceNotFoundException` (the creation status went with the permission set).
7. `DescribePermissionSetProvisioningStatus` for any `RequestId` minted against `<ps>` — also 404, by the `permission_set_provisioning` filter.
8. `DescribeAccountAssignmentDeletionStatus` with `{"InstanceArn":"<instance>","AccountAssignmentDeletionRequestId":"<delete-id>"}` — 200, with `PermissionSetArn: "<ps>"`, a permission set `DescribePermissionSet` and `ListPermissionSets` both report as absent; `ListAccountAssignmentDeletionStatus` for the same instance still lists `<delete-id>`.
