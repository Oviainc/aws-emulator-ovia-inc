Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 032f9a5bcc1a20e2d342a3403d3d533abd316530

# Fold the mirrored "which copy of a permission set an account holds" computation into one rule instead of two

_Filed: 2026-10-29T10:32:44Z_
_Operation: ListPermissionSetsProvisionedToAccount | Class: maintainability_

> _Note: this issue was identified against the code base at commit `032f9a5bcc1a20e2d342a3403d3d533abd316530`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule that decides what is provisioned where — "an account holds a permission set once it has been provisioned there, the copy in place is the one the LAST provisioning put there, and its status is the comparison of that copy's revision against the permission set's revision now" — is implemented TWICE in state.rs, once per direction of the same relation:

* `provisioned_accounts(instance_arn, permission_set_arn)` -> the accounts side, read by `ListAccountsForProvisionedPermissionSet` and by `ProvisionPermissionSet`'s ALL_PROVISIONED_ACCOUNTS target;
* `provisioned_permission_sets(instance_arn, account_id)` -> the permission-sets side, read by `ListPermissionSetsProvisionedToAccount`.

Both walk the SAME two record sets in the SAME order (`provisioned_copies` first, then `account_assignments`, so an assignment row overwrites a copy for the same pair), both call `provisioning_status_of(recorded_revision, current_revision)`, and both carry the identical justifying comment. The second function's own doc admits the duplication in as many words ("The MIRROR of [`Self::provisioned_accounts`], over the same records and with the same rule" / "The status is COMPUTED the same way"), so the only thing that keeps the two readers agreeing today is that someone wrote the same four steps twice.

The copies can drift, and the drift is not hypothetical: the already-open defect in this rule (the withdrawn-assignment rewind, `keep_withdrawn_copy`, state.rs:2870) lives in the rows half of BOTH functions, so a fix applied to one half would leave `ListAccountsForProvisionedPermissionSet` and `ListPermissionSetsProvisionedToAccount` reporting different provisioning states for the same (account, permission set) pair — one saying LATEST_PERMISSION_SET_PROVISIONED and the other LATEST_PERMISSION_SET_NOT_PROVISIONED about the same copy.

The fix is to express the relation once (one pass yielding the (permission set, account, recorded revision) pairs in copies-then-rows order, with the status computed by the one existing `provisioning_status_of`) and let the two public helpers project it by whichever axis their caller asked for. Every response, every page key and every existing test stay exactly as they are — the two functions keep their signatures, their `BTreeMap` key order and their "a permission set/account the instance no longer holds is left out" behaviour.

## Evidence

Copy 1 — `services/service-com-amazonaws-ssoadmin-swbexternalservice/src/state.rs:3494-3519`:
```
    pub fn provisioned_accounts(
        &self,
        instance_arn: &str,
        permission_set_arn: &str,
    ) -> BTreeMap<String, &'static str> {
        ...
        for copy in &self.provisioned_copies {
            ...
            let status = provisioning_status_of(&copy.revision, &revision);
            provisioned.insert(copy.account_id.clone(), status);
        }
        for row in &self.account_assignments {
            ...
            let status = provisioning_status_of(&row.provisioned_revision, &revision);
            // Later rows were provisioned later, so the copy in place is the last one's.
            provisioned.insert(row.account_id.clone(), status);
        }
```

Copy 2 — the same crate, `src/state.rs:3526-3537`, which states that it IS the same rule:
```
    /// The MIRROR of [`Self::provisioned_accounts`], over the same records and with the
    /// same rule: an account holds a permission set once that permission set has been
    /// provisioned there — which both granting an account assignment on the account and
    /// provisioning the permission set into it do — however many principals were assigned
    /// through it, so a permission set appears at most once. The copy in place is the one
    /// the LAST provisioning put there.
    ///
    /// The status is COMPUTED the same way, against the revision of EACH permission set as
    /// it stands now ...
```
and repeats the steps verbatim at `src/state.rs:3549-3595`:
```
        for copy in &self.provisioned_copies {
            ...
            let status = provisioning_status_of(&copy.revision, revision);
            provisioned.insert(copy.permission_set_arn.clone(), status);
        }
        for row in &self.account_assignments {
            ...
            let status = provisioning_status_of(&row.provisioned_revision, revision);
            // Later rows were provisioned later, so the copy in place is the last one's.
            provisioned.insert(row.permission_set_arn.clone(), status);
        }
```
(note the identical comment at `state.rs:3516` and `state.rs:3593`).

The two copies are reached from two handlers, each consuming its half the same way — `src/ops/list_accounts_for_provisioned_permission_set.rs:268-273`:
```
            store
                .provisioned_accounts(&input.instance_arn, &input.permission_set_arn)
                .into_iter()
                .filter(|(_, status)| filter.is_none_or(|wanted| wanted == *status))
```
and `src/ops/list_permission_sets_provisioned_to_account.rs:268-273`:
```
            store
                .provisioned_permission_sets(&input.instance_arn, &input.account_id)
                .into_iter()
                .filter(|(_, status)| filter.is_none_or(|wanted| wanted == *status))
```
(`src/ops/provision_permission_set.rs:261-264` reads copy 1 as well, for its non-account target).

The shared input the two copies both read is written in one place — `src/state.rs:2860-2876` (`keep_withdrawn_copy`, "Carry the copy a WITHDRAWN assignment recorded into the copy its account keeps") — which is exactly where the open behavioural defect in this rule sits, so a one-sided fix makes the two readers disagree.

Test coverage that must stay green and unchanged: `tests/all/accounts_for_provisioned_permission_set.rs` (the accounts side, including its `ProvisioningStatus` filter table) and `tests/all/permission_sets_provisioned_to_account.rs` — `every_provisioned_permission_set_is_reported_once_in_arn_order` (line 263), `the_provisioning_status_filter_selects_by_the_definition_the_account_holds` (line 548), `a_permission_set_is_reported_until_its_last_assignment_is_withdrawn` (line 681), `an_account_nothing_is_provisioned_into_answers_an_empty_page` (line 395) — plus `tests/all/permission_set_provisioning_targets.rs`, which pins the ALL_PROVISIONED_ACCOUNTS target that reads copy 1.

## Steps to Reproduce

1. Open `services/service-com-amazonaws-ssoadmin-swbexternalservice/src/state.rs` and read `provisioned_accounts` (lines 3494-3520) and `provisioned_permission_sets` (lines 3540-3597) side by side: the same two record walks in the same order, the same `provisioning_status_of` comparison and the same "Later rows were provisioned later" comment appear in both.
2. Read the doc comment at `state.rs:3526-3537`, which states that the second function is "The MIRROR of [`Self::provisioned_accounts`], over the same records and with the same rule" and that "The status is COMPUTED the same way" — the crate's own admission that one rule is written twice.
3. Follow the two callers: `src/ops/list_accounts_for_provisioned_permission_set.rs:269` and `src/ops/list_permission_sets_provisioned_to_account.rs:269`, which consume the two halves identically (filter by the request's `ProvisioningStatus`, then page by the map key).
4. Confirm the drift risk: the rows half of both copies depends on `AccountAssignmentRecord::provisioned_revision` as maintained by `keep_withdrawn_copy` (`state.rs:2870`); change that decision in one function only and `ListAccountsForProvisionedPermissionSet` and `ListPermissionSetsProvisionedToAccount` report different provisioning statuses for the same (account, permission set) pair.
5. Fix check: with the relation computed once and both helpers projecting it, `cargo test -p service-com-amazonaws-ssoadmin-swbexternalservice` must still pass unchanged, in particular `tests/all/accounts_for_provisioned_permission_set.rs`, `tests/all/permission_sets_provisioned_to_account.rs` and `tests/all/permission_set_provisioning_targets.rs`.
