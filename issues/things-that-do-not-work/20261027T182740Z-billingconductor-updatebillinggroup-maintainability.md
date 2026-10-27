Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f2ead5306d42c2c14ef865f57d355d9f40696e5a

# Billing group `Size` is derived twice (membership index vs. the record's linked_account_ids), contrary to the comments on both copies

_Filed: 2026-10-27T18:27:40Z_
_Operation: UpdateBillingGroup | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f2ead5306d42c2c14ef865f57d355d9f40696e5a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The modelled `Size` ("The number of accounts in the particular billing group") is computed by two independent rules in this crate:

1. `ComAmazonawsBillingconductorAwsbillingconductorState::billing_group_sizes_in` counts the per-period account memberships through `membership_in` — this is what `ListBillingGroups` reports.
2. `BillingGroupRecord::size()` returns `linked_account_ids.len()` off the live record — its ONE caller is `UpdateBillingGroup`'s `render`.

Both copies carry a comment asserting there is only one rule: `billing_group_sizes_in`'s doc says "ONE rule answers every period ... so the count cannot disagree with the listing that reports the same memberships", `BillingGroupVersion`'s doc says "The membership a period reports is not here either: it is kept per period on the account memberships themselves ([...billing_group_sizes_in])", and `UpdateBillingGroup::render`'s doc claims "`Size`, `Status` and `StatusReason` are derived from the record by the same methods every other reader of a group uses (`ListBillingGroups`), so the two views cannot drift" — while it in fact calls the second rule. The two counts agree today only because every writer of `linked_account_ids` also writes the membership index; nothing holds them together, so any future change to one path silently makes `UpdateBillingGroup` and `ListBillingGroups` disagree about the same group.

The fix is to have `UpdateBillingGroup::render` take its `Size` from `billing_group_sizes_in(&[group_arn], &current_billing_period())`, the method the doc already names, and drop the second derivation. Responses and existing tests are unchanged: the tests below assert the two numbers are equal.

## Evidence

Copy 1 — `src/state.rs:2533-2555` (doc then body):
```
    /// ONE rule answers every period, the membership index through [`membership_in`],
    /// so the count cannot disagree with the listing that reports the same
    /// memberships. ...
    pub fn billing_group_sizes_in(
        &self,
        group_arns: &[String],
        billing_period: &str,
    ) -> BTreeMap<String, i64> {
```
used by `src/ops/list_billing_groups.rs:323`: `let sizes = state.billing_group_sizes_in(&page_arns, &billing_period);`

Copy 2 — `src/state.rs:1182-1185`:
```
    /// `Size`: "The number of accounts in the particular billing group."
    pub fn size(&self) -> i64 {
        i64::try_from(self.linked_account_ids.len()).unwrap_or(i64::MAX)
    }
```
its only caller, `src/ops/update_billing_group.rs:410`: `size: group.size(),` — under the comment at `src/ops/update_billing_group.rs:400-402`:
```
/// The stored group as this operation reports it. `Size`, `Status` and `StatusReason`
/// are derived from the record by the same methods every other reader of a group uses
/// (`ListBillingGroups`), so the two views cannot drift.
```
and contradicting `src/state.rs:1352-1354`:
```
/// so there is one copy of them. The membership a period reports is not here either: it
/// is kept per period on the account memberships themselves
/// ([`ComAmazonawsBillingconductorAwsbillingconductorState::billing_group_sizes_in`]).
```
`grep` confirms there are exactly two call sites in the crate: `update_billing_group.rs:410` (`.size()`) and `list_billing_groups.rs:323` (`billing_group_sizes_in`).

Test coverage that must stay green and already pins the equality: `tests/all/update_billing_group.rs` `update_replaces_the_members_it_names_and_keeps_the_rest` asserts the update's `response["Size"] == Some(2)` (`:495-500`) and the listing's `listed["Size"] == Some(2)` (`:548-553`); `tests/all/billing_group_billing_periods.rs` and `tests/all/membership_writes_keep_superseded_generations.rs` pin the per-period membership counts the first rule produces.

## Steps to Reproduce

1. grep -n 'size()' services/service-com-amazonaws-billingconductor-awsbillingconductor/src — one caller, src/ops/update_billing_group.rs:410.
2. grep -n 'billing_group_sizes_in' same tree — defined at src/state.rs:2538, called only from src/ops/list_billing_groups.rs:323.
3. Read src/ops/update_billing_group.rs:400-410: the comment names ListBillingGroups' method, the code calls BillingGroupRecord::size() instead.
4. Read src/state.rs:1183-1185 and src/state.rs:2538-2556: two different computations of the one modelled `Size`.
5. Replace `group.size()` in update_billing_group.rs's render with the state method for the current billing period, delete BillingGroupRecord::size(), and run tests/all/update_billing_group.rs — the Size assertions at :495 and :548 still pass.
