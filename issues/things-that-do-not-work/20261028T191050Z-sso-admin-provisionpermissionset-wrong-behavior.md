Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3da2fe3b5486a725ab91e82448a29b4c21c390bb

# ProvisionPermissionSet to an AWS_ACCOUNT target that holds no assignment reports SUCCEEDED but provisions nothing — the target never appears among the accounts the permission set is provisioned to

_Filed: 2026-10-28T19:10:50Z_
_Operation: ProvisionPermissionSet | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3da2fe3b5486a725ab91e82448a29b4c21c390bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ProvisionPermissionSet` with `TargetType: AWS_ACCOUNT` and `TargetId: <account>` resolves the instance and the permission set, computes the target set as exactly that one account, stores a provisioning record carrying it, and answers a tracking status that walks `IN_PROGRESS` -> `SUCCEEDED`. But the write the record promises — `apply_provisioning` — only re-stamps `provisioned_revision` on account-assignment rows that ALREADY exist for that (instance, permission set, account) triple. A target account that holds no assignment of the permission set is therefore skipped entirely: nothing is written for it, `ListAccountsForProvisionedPermissionSet` never lists it, and `ListPermissionSetsProvisionedToAccount` for that account never names the permission set. The call is a 200 + SUCCEEDED that persists nothing a reader can see.

This contradicts the crate's own description of what the record means and does. `PermissionSetProvisioningRecord::accounts` is documented as "Every account this provisioning copies the permission set into ... the one account an `AWS_ACCOUNT` target names", and the record's own doc says the copy "reaches the accounts in `accounts`" when the workflow completes. For an unassigned target account, the account IS in `accounts` and the copy reaches nothing. The handler's own comment (`provision_permission_set.rs`) likewise calls the resolved value "the accounts this provisioning copies into".

It also contradicts the suite's own statement of the rule: `tests/all/permission_set_provisioning.rs` opens with "provisioning a permission set COPIES the permission set's definition ... into the accounts the request's target names — the one account an `AWS_ACCOUNT` target names ... so each of those accounts then holds the permission set's current definition", while a test further down in the same file asserts the opposite for an unassigned target.

Consequence over the wire: a caller that provisions a permission set into a new account (the normal way to roll a permission set out before granting assignments there) is told the workflow SUCCEEDED, yet every read of "where is this permission set provisioned" answers as if the call had never been made.

## Evidence

MODEL (`smithy_query(query="doc",shape="ProvisionPermissionSet")`):
> "The process by which a specified permission set is provisioned to the specified target."

MODEL (`smithy_query(query="doc",shape="ListAccountsForProvisionedPermissionSet")`):
> "Lists all the Amazon Web Services accounts where the specified permission set is provisioned."

THE TARGET IS RESOLVED AND STORED — services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/provision_permission_set.rs:263-284:
```
        let (account_id, accounts) =
            if input.target_type == crate::state::PROVISION_TARGET_TYPE_AWS_ACCOUNT {
                let Some(target_id) = input.target_id.clone() else {
                    ...
                };
                let accounts = ::std::collections::BTreeSet::from([target_id.clone()]);
                (Some(target_id), accounts)
```
and handed to the record at provision_permission_set.rs:298-317 (`accounts,`).

WHAT THE CRATE SAYS THAT SET MEANS — src/state.rs:2416-2421:
```
    /// Every account this provisioning copies the permission set into, FIXED when the
    /// call was accepted: the one account an `AWS_ACCOUNT` target names, or the accounts
    /// holding a copy at that instant for an `ALL_PROVISIONED_ACCOUNTS` one. An account
    /// that started holding the permission set after the call is not part of the work
    /// this call accepted.
    pub accounts: BTreeSet<String>,
```
and src/state.rs:2398-2402:
```
/// the status on. The COPY the provisioning carries — [`revision`](Self::revision), the
/// permission set's definition as it stood when the call was accepted — reaches the
/// accounts in [`accounts`](Self::accounts) at that SAME instant, applied by
/// [`ComAmazonawsSsoadminSwbexternalserviceState::materialize_due`], so an account's copy
/// changes when the workflow completes rather than when it was asked for.
```

WHAT THE CODE ACTUALLY DOES — src/state.rs:2713-2738:
```
    /// An account holds a permission set through the assignments granted on it, and the
    /// revision recorded on those rows is what every reader compares against the
    /// permission set's current definition, so this provisioning updates exactly those
    /// rows. A target account that holds no assignment of this permission set has no copy
    /// to refresh, so there is nothing to write for it.
    fn apply_provisioning(&mut self, index: usize) {
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
and the reader it feeds — src/state.rs:3249-3257:
```
        for row in &self.account_assignments {
            if row.instance_arn != instance_arn || row.permission_set_arn != permission_set_arn {
                continue;
            }
            let status = provisioning_status_of(&row.provisioned_revision, &revision);
            // Later rows were provisioned later, so the copy in place is the last one's.
            provisioned.insert(row.account_id.clone(), status);
        }
```
so an account with no assignment row can never be reported by `ListAccountsForProvisionedPermissionSet` (src/ops/list_accounts_for_provisioned_permission_set.rs:288-293 calls exactly this helper).

THE SUITE CONTRADICTS ITSELF — tests/all/permission_set_provisioning.rs:6-10:
```
//! THE RULE under test: provisioning a permission set COPIES the permission set's
//! definition, as it stands when the call is accepted, into the accounts the request's
//! target names — the one account an `AWS_ACCOUNT` target names, or every account that
//! already holds the permission set for an `ALL_PROVISIONED_ACCOUNTS` one — so each of
//! those accounts then holds the permission set's current definition
```
versus tests/all/permission_set_provisioning.rs:465-499, which pins the opposite for the same target type:
```
/// A provisioning of a permission set the target account does not hold has no copy to
/// refresh there: an account holds a permission set through the assignments granted on it,
/// so the call is accepted and tracked, and the accounts reported as holding the permission
/// set are unchanged.
#[tokio::test]
async fn provisioning_an_account_that_holds_no_assignment_adds_no_account() {
...
    let all = list_accounts(&router, &instance_arn, &permission_set_arn, None, case).await;
    assert_accounts(&all, &[ACCOUNT_MID], &format!("{case}: unfiltered"));
```
The reason that test states ("an account holds a permission set through the assignments granted on it") is a modelling choice of the emulator, not anything the model says; the two model sentences quoted above say the permission set is provisioned to the specified target and that the list reports all accounts where it is provisioned.

## Steps to Reproduce

1. POST / with `X-Amz-Target: SWBExternalService.ListInstances`, read the account's `InstanceArn`, then `UpdateInstance` with `{"InstanceArn":"<instance>","PermissionSetsEnabled":true}` so the permission-set surface is available.
2. POST / with `X-Amz-Target: SWBExternalService.CreatePermissionSet` and `{"InstanceArn":"<instance>","Name":"Rollout"}`; keep the minted `PermissionSetArn`.
3. POST / with `X-Amz-Target: SWBExternalService.ProvisionPermissionSet` and `{"InstanceArn":"<instance>","PermissionSetArn":"<ps>","TargetType":"AWS_ACCOUNT","TargetId":"555555555555"}` — 200, with a minted `RequestId` and `Status: IN_PROGRESS`.
4. Wait out `crate::state::PERMISSION_SET_PROVISIONING_SPAN` (500 ms) and POST `DescribePermissionSetProvisioningStatus` with that `RequestId` — `Status: SUCCEEDED`, `AccountId: 555555555555`.
5. POST / with `X-Amz-Target: SWBExternalService.ListAccountsForProvisionedPermissionSet` and `{"InstanceArn":"<instance>","PermissionSetArn":"<ps>"}` — `AccountIds` is EMPTY; `555555555555` is absent, although the provisioning the emulator reported as SUCCEEDED named it as its target.
6. POST / with `X-Amz-Target: SWBExternalService.ListPermissionSetsProvisionedToAccount` and `{"InstanceArn":"<instance>","AccountId":"555555555555"}` — also empty.
7. Repeat steps 3-5 after first granting `CreateAccountAssignment` on `555555555555` for the same permission set: the account is now reported, showing the provisioning is only ever a refresh of an existing assignment row and never a provisioning of the target the request named.
