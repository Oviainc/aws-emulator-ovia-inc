Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6946203d9f2a769569cfa156ac7c42bcf6c300bc

# DeletePermissionSet forgets the provisioned copies but leaves the in-flight provisioning that re-creates them

_Filed: 2026-10-29T08:33:26Z_
_Operation: DeletePermissionSet | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6946203d9f2a769569cfa156ac7c42bcf6c300bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeletePermissionSet` drops the permission set, calls `state::forget_provisioned_copies` for it, and removes the assignments and their creation-status records — but it never touches `store.permission_set_provisionings`. A `ProvisionPermissionSet` that was still in flight when the delete landed therefore survives the delete with `applied: false`, and the next request's clock sweep (`on_request` -> `materialize_due` -> `materialize_due_provisionings` -> `apply_provisioning`) applies it unconditionally: `apply_provisioning` asks only `!record.applied && record.completion_due(now)` and never asks whether the permission set it reports on still exists, so it calls `record_provisioned_copy(...)` and INSERTS a fresh `ProvisionedCopyRecord` for a permission set this instance no longer holds — exactly the state the delete had just removed and whose absence `forget_provisioned_copies` documents as an invariant.

The instance-level sibling does it the other way round: `remove_instance` deletes the `permission_set_provisionings` rows of the instance it drops, so no copy can land for an instance that is gone. Only the permission-set-level delete leaves the owed write behind.

No read exposes the dangling copy today (both `provisioned_accounts` and `provisioned_permission_sets` filter on the permission set still existing), so this is stored state the crate's own rule forbids rather than a wrong response: hence LOW. It still breaks the invariant the delete states, and the leaked rows consume the same `provisioned_copies` vector whose oldest entries `record_provisioned_copy` evicts.

## Evidence

THE RULE, stated by the delete — services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/delete_permission_set.rs:216-219:
```
        store.permission_sets.remove(&input.permission_set_arn);
        // A permission set that no longer exists is provisioned nowhere, so the copies
        // its provisionings delivered go with it.
        store.forget_provisioned_copies(&input.instance_arn, &input.permission_set_arn);
```
(the handler goes on to remove `account_assignments` and their `assignment_operations` at lines 224-239; nothing in it mentions `permission_set_provisionings`)

and restated on the helper — src/state.rs:2841-2849:
```
    /// Forget every copy of `permission_set_arn` under `instance_arn`: a permission set
    /// that no longer exists is provisioned nowhere, and leaving its copies behind would
    /// let a later permission set minted under the same ARN inherit accounts nothing
    /// provisioned it into.
    pub fn forget_provisioned_copies(&mut self, instance_arn: &str, permission_set_arn: &str) {
        self.provisioned_copies.retain(|copy| {
            copy.instance_arn != instance_arn || copy.permission_set_arn != permission_set_arn
        });
    }
```

THE PATH THAT RE-CREATES THEM, with no permission-set guard — src/state.rs:2717-2727 and 2743-2766:
```
    fn materialize_due_provisionings(&mut self, now: SystemTime) {
        let due: Vec<usize> = self
            .permission_set_provisionings
            .iter()
            .enumerate()
            .filter(|(_, record)| !record.applied && record.completion_due(now))
            .map(|(index, _)| index)
            .collect();
        for index in due {
            self.apply_provisioning(index);
        }
    }
```
```
    fn apply_provisioning(&mut self, index: usize) {
        let Some(record) = self.permission_set_provisionings.get_mut(index) else { return; };
        if record.applied { return; }
        record.applied = true;
        ...
        for account_id in &accounts {
            self.record_provisioned_copy(&instance_arn, &permission_set_arn, account_id, &revision);
        }
    }
```
(`record_provisioned_copy`, src/state.rs:2775-2798, pushes a new `ProvisionedCopyRecord` when the account holds none — which is the case right after `forget_provisioned_copies` ran)

THE SIBLING THAT DOES DROP THE OWED WRITE — src/state.rs:2954-2955, inside `remove_instance`:
```
        self.permission_set_provisionings
            .retain(|record| record.instance_arn != instance_arn);
```

The record that survives is written with `applied: false` by src/ops/provision_permission_set.rs:314, and its completion falls due one `crate::state::PERMISSION_SET_PROVISIONING_SPAN` (500 ms, src/state.rs:602) after the call.

TEST COVERAGE: tests/all/permission_set_provisioning.rs:738-759 pins only that the STATUS stops resolving after the delete ("A provisioning status belongs to the permission set it reports on: once that permission set is gone, the instance tracks no provisioning under its id" -> 400 `ResourceNotFoundException`). No test in that file, in tests/all/permission_set_provisioning_targets.rs or in tests/all/accounts_for_provisioned_permission_set.rs inspects `store.provisioned_copies` after a delete that raced an unapplied provisioning, so nothing states a reason for the copy being re-created.

## Steps to Reproduce

1. Start the emulator, `ListInstances` to read the account's `InstanceArn`, then `UpdateInstance` with `{"InstanceArn":"<instance>","PermissionSetsEnabled":true}`.
2. `CreatePermissionSet` with `{"InstanceArn":"<instance>","Name":"Doomed"}`; keep `<ps>`.
3. `ProvisionPermissionSet` with `{"InstanceArn":"<instance>","PermissionSetArn":"<ps>","TargetType":"AWS_ACCOUNT","TargetId":"123456789012"}` — 200 with `Status: IN_PROGRESS`; the copy is owed 500 ms later and the record is stored with `applied: false`.
4. Within that 500 ms window, `DeletePermissionSet` with `{"InstanceArn":"<instance>","PermissionSetArn":"<ps>"}` — 200. At this point `provisioned_copies` holds no copy of `<ps>` (the handler called `forget_provisioned_copies`) and `permission_set_provisionings` still holds the unapplied record.
5. Wait past the 500 ms span and make any further call (for example `ListInstances`), which runs `on_request` -> `materialize_due` -> `materialize_due_provisionings`.
6. Inspect the store as the crate's own tests do (tests/all/permission_set_provisioning.rs:97-105 reaches `store.permission_set_provisionings` directly): `store.provisioned_copies` now contains a `ProvisionedCopyRecord { permission_set_arn: "<ps>", account_id: "123456789012", .. }` for a permission set the store no longer holds — the record `forget_provisioned_copies` had just removed and whose absence its doc states as the rule.
7. For contrast, repeat with `DeleteInstance` instead of `DeletePermissionSet` in step 4: `remove_instance` drops the `permission_set_provisionings` row, so no copy lands.
