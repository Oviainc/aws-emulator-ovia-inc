Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 992ff7064f71a6c13a404188e850cf82f701572d

# Withdrawing one of several assignments on an account rewinds its reported provisioning status to an older definition, against the "last copy delivered" rule the crate states and tests

_Filed: 2026-10-29T09:47:32Z_
_Operation: ListAccountsForProvisionedPermissionSet | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `992ff7064f71a6c13a404188e850cf82f701572d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListAccountsForProvisionedPermissionSet` (and its mirror `ListPermissionSetsProvisionedToAccount`) computes each account's `ProvisioningStatus` in `ComAmazonawsSsoadminSwbexternalserviceState::provisioned_accounts`, which walks the stored `provisioned_copies` first and then the `account_assignments`, letting the assignment row's `provisioned_revision` OVERWRITE whatever the copy record said. The doc on that function justifies the precedence with an invariant — the assignment rows are always as new as the copy record, because "a provisioning of that account refreshes [them] too" — and the row loop restates it ("Later rows were provisioned later, so the copy in place is the last one's").

`ComAmazonawsSsoadminSwbexternalserviceState::keep_withdrawn_copy`, which `DeleteAccountAssignment` calls, breaks that invariant: it advances the ACCOUNT'S COPY RECORD to the revision the withdrawn row carried, and leaves the remaining assignment rows of that account untouched. So when an account carries two assignments granted at two different definitions of the permission set, withdrawing the NEWER one writes the newer revision into the copy record (where `keep_withdrawn_copy`'s own doc says "the copy the account keeps has to go on reporting the same definition") and then lets the surviving OLDER row override it.

Result: the same account that was reported `LATEST_PERMISSION_SET_PROVISIONED` before the withdrawal is reported `LATEST_PERMISSION_SET_NOT_PROVISIONED` after it — i.e. withdrawing an assignment makes the account look as if it had never received the current definition, although the crate's own copy record holds exactly the current revision. The status is wrong in both directions: a `ProvisioningStatus=LATEST_PERMISSION_SET_PROVISIONED` request omits the account, and a `LATEST_PERMISSION_SET_NOT_PROVISIONED` request returns it. `ListPermissionSetsProvisionedToAccount` answers the mirror image for the same state, because `provisioned_permission_sets` repeats the identical two-loop precedence.

## Evidence

THE DOCUMENTED PRECEDENCE AND THE INVARIANT IT RESTS ON — services/service-com-amazonaws-ssoadmin-swbexternalservice/src/state.rs:3344-3349:
```
    /// Both ways a copy reaches an account are read here: the copies a provisioning
    /// delivered to the accounts it targeted ([`Self::provisioned_copies`]) and the
    /// revision each assignment granted on an account recorded
    /// ([`AccountAssignmentRecord::provisioned_revision`]), which a provisioning of that
    /// account refreshes too — so an assigned account's row carries the later of the two
    /// and is the one that counts.
```
THE CODE — src/state.rs:3367-3381 (the row loop inserts unconditionally over the copy loop):
```
        for copy in &self.provisioned_copies {
            if copy.instance_arn != instance_arn || copy.permission_set_arn != permission_set_arn {
                continue;
            }
            let status = provisioning_status_of(&copy.revision, &revision);
            provisioned.insert(copy.account_id.clone(), status);
        }
        for row in &self.account_assignments {
            if row.instance_arn != instance_arn || row.permission_set_arn != permission_set_arn {
                continue;
            }
            let status = provisioning_status_of(&row.provisioned_revision, &revision);
            // Later rows were provisioned later, so the copy in place is the last one's.
            provisioned.insert(row.account_id.clone(), status);
        }
```
THE WRITE THAT BREAKS THE INVARIANT — src/state.rs:2820-2839:
```
    /// An account holds the LAST copy delivered to it, and a grant delivers one too, so an
    /// account that was provisioned before the assignment was granted holds what the grant
    /// put there rather than the older definition the provisioning carried: withdrawing
    /// that assignment removes the row recording it, and the copy the account keeps has to
    /// go on reporting the same definition.
...
    pub fn keep_withdrawn_copy(
        &mut self,
        instance_arn: &str,
        permission_set_arn: &str,
        account_id: &str,
        revision: &str,
    ) {
        if let Some(copy) =
            self.provisioned_copy_mut(instance_arn, permission_set_arn, account_id)
        {
            copy.revision = revision.to_string();
        }
    }
```
It advances only the copy — src/ops/delete_account_assignment.rs:286-297:
```
        let removed = store.account_assignments.remove(position);
        ...
        store.keep_withdrawn_copy(
            &input.instance_arn,
            &removed.permission_set_arn,
            &removed.account_id,
            &removed.provisioned_revision,
        );
```
THE READER — src/ops/list_accounts_for_provisioned_permission_set.rs:289-294:
```
            store
                .provisioned_accounts(&input.instance_arn, &input.permission_set_arn)
                .into_iter()
                .filter(|(_, status)| filter.is_none_or(|wanted| wanted == *status))
                .map(|(account_id, _)| (account_id.clone(), account_id))
                .collect()
```
THE MIRROR, same precedence — src/state.rs:3412-3434 then 3435+ inside `provisioned_permission_sets`, read by src/ops/list_permission_sets_provisioned_to_account.rs:273.

TEST COVERAGE — the rule is pinned for the ONE-assignment case and asserts the opposite of what the two-assignment case answers, tests/all/permission_set_provisioning_targets.rs:719-723 and 789-799:
```
/// A provisioning is what put the permission set into the account, so the account keeps it
/// when an assignment granted there afterwards is withdrawn again — holding the LAST copy
/// delivered to it, which is the grant's when the permission set changed in between.
...
        // The copy the account keeps is the last one delivered there, so it is the current
        // definition whether or not the permission set changed before the grant.
        let latest = provisioned_accounts(
            &router,
            &instance_arn,
            &permission_set_arn,
            Some(LATEST_PROVISIONED),
            case,
        )
        .await;
        assert_same(&latest, &expected, &format!("{case}: withdrawn, latest"));
```
No test grants TWO assignments on one account with a `UpdatePermissionSet` in between: tests/all/accounts_for_provisioned_permission_set.rs:636-694 (`an_account_is_reported_until_its_last_assignment_is_withdrawn`) grants three principals on ACCOUNT_MID and two on ACCOUNT_HIGH but never changes the permission set between the grants, and tests/all/permission_sets_provisioned_to_account.rs:681 does the same for the mirror.

## Steps to Reproduce

1. Start the emulator, read the account's instance ARN with `X-Amz-Target: SWBExternalService.ListInstances`, and enable permission sets on it (`UpdateInstance` with `{"InstanceArn":"<instance>","PermissionSetsEnabled":true}`).
2. `CreatePermissionSet` `{"InstanceArn":"<instance>","Name":"Rollout"}` — keep `<ps>`.
3. `ProvisionPermissionSet` `{"InstanceArn":"<instance>","PermissionSetArn":"<ps>","TargetType":"AWS_ACCOUNT","TargetId":"111122223333"}`, then wait out `crate::state::PERMISSION_SET_PROVISIONING_SPAN` (500 ms) so the copy lands. The store now holds one `ProvisionedCopyRecord` for 111122223333.
4. `CreateAccountAssignment` `{"InstanceArn":"<instance>","TargetId":"111122223333","TargetType":"AWS_ACCOUNT","PermissionSetArn":"<ps>","PrincipalType":"GROUP","PrincipalId":"<uuid-A>"}`.
5. `UpdatePermissionSet` `{"InstanceArn":"<instance>","PermissionSetArn":"<ps>","Description":"changed"}` — the definition revision moves on.
6. `CreateAccountAssignment` again with `"PrincipalId":"<uuid-B>"`. This grant records the CURRENT revision on its row.
7. `ListAccountsForProvisionedPermissionSet` `{"InstanceArn":"<instance>","PermissionSetArn":"<ps>","ProvisioningStatus":"LATEST_PERMISSION_SET_PROVISIONED"}` — 200 with `AccountIds: ["111122223333"]`, correct.
8. `DeleteAccountAssignment` for `<uuid-B>` (the NEWER grant). `keep_withdrawn_copy` writes the current revision into the account's copy record.
9. Repeat step 7 — `AccountIds` is now EMPTY, and `ProvisioningStatus=LATEST_PERMISSION_SET_NOT_PROVISIONED` returns `["111122223333"]`, although the stored copy record for that account holds exactly the permission set's current revision. Withdrawing an assignment regressed the account's reported provisioning status.
10. `ListPermissionSetsProvisionedToAccount` `{"InstanceArn":"<instance>","AccountId":"111122223333","ProvisioningStatus":"LATEST_PERMISSION_SET_PROVISIONED"}` answers the mirror image: `<ps>` is absent.
