Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: fb2d49e8bd3d09cec7ea7562ca3d7e59e4fee21f

# A grant made while a ProvisionPermissionSet is in flight has its recorded copy rewound to the older definition when that workflow lands

_Filed: 2026-10-29T04:08:33Z_
_Operation: CreateAccountAssignment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `fb2d49e8bd3d09cec7ea7562ca3d7e59e4fee21f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Granting an account assignment PROVISIONS the permission set into the target account: `CreateAccountAssignment` reads the permission set's revision at the instant it is accepted and records it on the new row (`provisioned_revision`), which is the value every "where is this permission set provisioned" read compares against the permission set's current definition. The crate states the ordering rule for that field in one sentence: the copy in place is the one the LAST provisioning put there, "whether that provisioning was a grant or a `ProvisionPermissionSet` call".

`apply_provisioning` breaks that rule. When the clock makes a stored provisioning due it stamps its OWN revision — the one captured when that call was accepted — onto EVERY assignment row of the (instance, permission set) whose account is in the record's `accounts` set, with no regard for when the revision already on the row was recorded. So a grant accepted AFTER the provisioning was accepted, but BEFORE it fell due, is silently undone: the row that recorded the permission set's CURRENT definition is rewritten with the older definition the in-flight workflow carries.

The guard the function does have (`if record.applied { return; }`) addresses only re-application on a later sweep, and its own doc gives the reason — "re-copying it would undo a provisioning that followed it". The FIRST application does exactly that undoing whenever the provisioning that followed it was a grant, because a grant provisions immediately while a `ProvisionPermissionSet` provisions on a delay.

Observable effect: an account that `ListAccountsForProvisionedPermissionSet` reports as `LATEST_PERMISSION_SET_PROVISIONED` right after a grant flips back to `LATEST_PERMISSION_SET_NOT_PROVISIONED` when an older, still-in-flight provisioning completes — and `ListPermissionSetsProvisionedToAccount` for that account mirrors it. Stored state the grant wrote is lost; no later call can restore it except another provisioning.

This is distinct from the two issues already open on this surface: 20261028T191050Z-sso-admin-provisionpermissionset-wrong-behavior is about a target account that holds NO row (nothing is written at all), while this is about a row that EXISTS and carries a NEWER revision being overwritten with an older one.

## Evidence

THE ORDERING RULE THE CRATE STATES — services/service-com-amazonaws-ssoadmin-swbexternalservice/src/state.rs:3212-3217 (`provisioned_accounts`):
```
    /// An account holds a permission set once it has been provisioned there — which is
    /// what granting an account assignment on that account does — however many principals
    /// were assigned through it, so an account appears at most once. The copy in place is
    /// the one the LAST provisioning put there, so a provisioning that followed a change
    /// to the permission set brings the account back up to date, whether that provisioning
    /// was a grant or a `ProvisionPermissionSet` call.
```

THE GRANT IS SUCH A PROVISIONING — src/ops/create_account_assignment.rs:244-248 and :263:
```
        // Granting an assignment PROVISIONS the permission set into the target account, so
        // the account is given the definition that is current at this instant. That
        // revision is recorded on the row below, which is what lets a later read tell an
        // account holding the permission set's latest definition from one left behind by a
        // change to it.
...
        let provisioned_revision = permission_set.definition_revision();
```

THE SAME INVARIANT RESTATED ON THE WRITER, AND THE CODE THAT BREAKS IT — src/state.rs:2694-2725:
```
    /// Copy the definition one stored provisioning carries into the accounts it targeted,
    /// by recording that revision as the one each of those accounts now holds. A
    /// provisioning that has already landed is left alone, so it lands exactly once
    /// however often the store is swept: re-copying it would undo a provisioning that
    /// followed it.
...
    fn apply_provisioning(&mut self, index: usize) {
...
        if record.applied {
            return;
        }
        record.applied = true;
...
        for row in &mut self.account_assignments {
            if row.instance_arn != instance_arn || row.permission_set_arn != permission_set_arn {
                continue;
            }
            if !accounts.contains(&row.account_id) {
                continue;
            }
            row.provisioned_revision = revision.clone();
        }
    }
```
The loop's only tests are instance, permission set and account membership — nothing compares WHEN `row.provisioned_revision` was recorded against when this provisioning was accepted, so a newer grant's revision is overwritten unconditionally.

AND THE RECORD'S OWN FIELD DOC SAYS THE LATE ARRIVAL IS NOT THIS CALL'S WORK — src/state.rs:2403-2408:
```
    /// Every account this provisioning copies the permission set into, FIXED when the
    /// call was accepted: the one account an `AWS_ACCOUNT` target names, or the accounts
    /// holding a copy at that instant for an `ALL_PROVISIONED_ACCOUNTS` one. An account
    /// that started holding the permission set after the call is not part of the work
    /// this call accepted.
    pub accounts: BTreeSet<String>,
```

THE READ THAT REPORTS THE REGRESSION — src/ops/list_accounts_for_provisioned_permission_set.rs:288-293 calls `provisioned_accounts`, whose status comes straight from `row.provisioned_revision` (src/state.rs:3236-3243).

TEST COVERAGE: none for this ordering. tests/all/permission_set_provisioning.rs:382-462 only grants BEFORE the provisioning is accepted and never grants while one is in flight (`grep -n "in flight" tests/all/permission_set_provisioning.rs` → lines 415, 418, 429, 540, all about the status/copy not having landed yet), and tests/all/permission_set_provisioning.rs:465-499 covers the unassigned-target case the other issue owns. No test states a reason for an earlier-accepted provisioning winning over a later grant. The suite already holds the lever to reproduce it deterministically — `hold_provisionings` (tests/all/permission_set_provisioning.rs:99-105) moves only the stored completion instant.

## Steps to Reproduce

1. POST / with `X-Amz-Target: SWBExternalService.ListInstances`, read the `InstanceArn`, then `UpdateInstance` with `{"InstanceArn":"<instance>","PermissionSetsEnabled":true}`.
2. `CreatePermissionSet` with `{"InstanceArn":"<instance>","Name":"Rollout"}`; keep `<ps>`.
3. `CreateAccountAssignment` with `{"InstanceArn":"<instance>","TargetId":"111111111111","TargetType":"AWS_ACCOUNT","PermissionSetArn":"<ps>","PrincipalType":"USER","PrincipalId":"11111111-2222-3333-4444-555555555555"}` — account 111111111111 now holds revision v1.
4. `ProvisionPermissionSet` with `{"InstanceArn":"<instance>","PermissionSetArn":"<ps>","TargetType":"ALL_PROVISIONED_ACCOUNTS"}` — 200, `Status: IN_PROGRESS`; the record now carries accounts={111111111111} and revision v1 (or hold it in flight as tests/all/permission_set_provisioning.rs:420 does).
5. Inside the 500 ms `PERMISSION_SET_PROVISIONING_SPAN`, `UpdatePermissionSet` with `{"InstanceArn":"<instance>","PermissionSetArn":"<ps>","Description":"changed"}` — the permission set's revision is now v2 and `ListAccountsForProvisionedPermissionSet` with `ProvisioningStatus: LATEST_PERMISSION_SET_NOT_PROVISIONED` lists 111111111111.
6. Still inside that span, `CreateAccountAssignment` for the SAME account and permission set with a different `PrincipalId` — this grant provisions v2 into 111111111111, and `ListAccountsForProvisionedPermissionSet` with `ProvisioningStatus: LATEST_PERMISSION_SET_PROVISIONED` now lists 111111111111, exactly as src/state.rs:3214-3217 says it must.
7. Wait out the span (or set every provisioning's `succeeded_due` into the past with the suite's `hold_provisionings(&engine, completed())`) and repeat `ListAccountsForProvisionedPermissionSet` with `ProvisioningStatus: LATEST_PERMISSION_SET_PROVISIONED` — the list is now EMPTY, and the `LATEST_PERMISSION_SET_NOT_PROVISIONED` list names 111111111111 again: the provisioning accepted at step 4 stamped its older v1 over the v2 the grant at step 6 recorded.
8. `ListPermissionSetsProvisionedToAccount` with `{"InstanceArn":"<instance>","AccountId":"111111111111"}` reports `<ps>` as `LATEST_PERMISSION_SET_NOT_PROVISIONED` for the same reason.
