Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c1d5331cbdbc8645ce17750a515f2b0101f76c93

# A billing period a deleted billing group billed in reports that group as an account's association but no group at all: the period-scoped group listing skips the DeletedBillingGroupRecord the delete deliberately kept

_Filed: 2026-10-29T11:48:38Z_
_Operation: ListBillingGroups | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c1d5331cbdbc8645ce17750a515f2b0101f76c93`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteBillingGroup` does not erase the deleted group's record of the billing periods it billed in: `remove_billing_group` moves the whole `BillingGroupRecord` (with every generation it held) into `deleted_billing_groups`, and it ENDS the account memberships at the deletion period instead of forgetting them, precisely so that a read scoped to an earlier period still answers for the group.

One period-scoped read relies on exactly that and reports the deleted group: `ListAccountAssociations` for a period P strictly before the delete answers `BillingGroupArn = <the deleted group>` for each account it held then, and its handler comment defends that as the only possible answer.

The period-scoped reads of the GROUP do not: `ListBillingGroups` (and, from the same map, `ListBillingGroupCostReports` and `GetBillingGroupCostReport`) iterate `state.billing_groups`, which the delete emptied of that ARN, so for the SAME period P they report no such group — not its `Name`/`ComputationPreference`/`Status` as the generation in force in P had them, and not its `Size`, even though `billing_group_sizes_in` would still count the memberships that covered P for that ARN and `DeletedBillingGroupRecord::existed_in` already answers "did this group exist in P".

So a client that lists account associations for a closed month gets a billing group ARN it cannot resolve for that month through any other read of this service, and the group's membership count for a month it really billed in is reported by nobody. The data and the predicate are both already in the crate (the kept record, `existed_in`, `effective_in`, `billing_group_sizes_in`); only the group listing never consults them. This is distinct from the already-fixed `DeleteBillingGroup` occupancy issue, which is about the group's NAME staying claimed for the current period.

## Evidence

The delete keeps the period record on purpose — `src/state.rs:2812-2815`:
```
    /// What the delete does NOT erase is the group's own record of the billing periods
    /// it billed in: the removed record is kept as a
    /// [`DeletedBillingGroupRecord`](crate::state::DeletedBillingGroupRecord), so the
    /// name it occupied in those periods stays occupied for them.
```
and keeps the whole group, every generation of it — `src/state.rs:1511-1513` and `1524-1526`:
```
/// A billing group that was DELETED, with the instant the delete was made at: the
/// record of the billing periods that group billed in, which the delete does not
/// erase.
...
    /// The group as it stood when the delete took it, with every generation of it the
    /// store still held.
```
with the predicate for those periods already written — `src/state.rs:1540-1543`:
```
    /// Did this group exist in `billing_period`? From the period it was created in
    /// through the one it was deleted in, both inclusive; a value that is not a billing
    /// period at all names no period at all.
    pub fn existed_in(&self, billing_period: &str) -> bool {
```
and the memberships deliberately outliving the group — `src/state.rs:2841-2843`:
```
        // The memberships the group held END with it, rather than being forgotten: a
        // read scoped to a billing period the group existed in still has to report
        // the accounts it held then.
```
Side 1 — `ListAccountAssociations` reports the deleted group for such a period, and says so — `src/ops/list_account_associations.rs:206-210`:
```
        // `BillingGroups` filter of `ListCustomLineItems` do. That is the only answer
        // that can agree with the `BillingGroupArn` this same listing reports for the
        // period, since the memberships a group held outlive the group itself
        // (`crate::state::remove_billing_group`): a group deleted since still holds,
        // for a period it existed in, the accounts it held then.
```
(the value comes straight off the membership record, `src/ops/list_account_associations.rs:218`: `for (account_id, group_arn) in state.linked_accounts(&billing_period) {`, and `state.linked_accounts` resolves it through `membership_in`, `src/state.rs:2770-2772`, which never checks that the group is still stored.)

Side 2 — the period-scoped group listing only ever walks the LIVE map, so the same period reports no group — `src/ops/list_billing_groups.rs:304-310`:
```
        // `billing_groups` is ordered by ARN, and an ARN carries the monotonic id
        // this service mints, so the sequence is creation order and a `NextToken`
        // names the same cut point on the next call.
        for (arn, group) in &state.billing_groups {
            let Some(version) = group.effective_in(&billing_period) else {
                continue;
            };
```
while `remove_billing_group` removed that key — `src/state.rs:2816-2819`:
```
    pub fn remove_billing_group(&mut self, group_arn: &str) -> Option<BillingGroupRecord> {
        let removed = self.billing_groups.remove(group_arn)?;
        self.billing_group_names.remove(&removed.name);
        self.record_deleted_billing_group(removed.clone(), now_millis());
```
and `Size` for such a period is computable but asked for nobody — `src/state.rs:2786-2794` (`billing_group_sizes_in` answers for whatever ARNs it is handed, from the membership index), fed only from the page of live groups (`src/ops/list_billing_groups.rs:330-331`).

Test coverage: the membership side is pinned — `tests/all/account_association_billing_periods.rs:455-458` ("a disassociation and a deleted billing group both end it at the period the call was made in, so an earlier period still reports the group") and `:487-488` ("Every period the membership ran for reports the group, whether or not the group still exists"). That test deliberately does NOT call its own `group_size_in` helper (`tests/all/account_association_billing_periods.rs:287-301`, "The `Size` `ListBillingGroups` reports for one group in `period`", which panics when the group is not listed) for the deleted row, so nothing pins the group listing for a period before the delete. The only `ListBillingGroups`-after-delete assertion is scoped to the DELETION period itself — `tests/all/billing_group_core_loop.rs:1323-1327` (no `BillingPeriod`, i.e. the current month) — which this report leaves untouched.

## Steps to Reproduce

1. Designate the emulated account as the payer so the control plane is reachable (set `ComAmazonawsBillingconductorAwsbillingconductorState::payer_account_id`).
2. POST /create-pricing-plan, then POST /create-billing-group with `AccountGrouping.LinkedAccountIds = ["111122223333"]`, and arrange for the group and the membership to have begun in an earlier billing period (as `tests/all/account_association_billing_periods.rs` does with `membership_ran` / `group_created_in`).
3. POST /delete-billing-group for that group (HTTP 200).
4. POST /list-account-associations with `{"BillingPeriod": "<the earlier period>"}` — the account is reported with `BillingGroupArn` naming the deleted group, as `tests/all/account_association_billing_periods.rs:487-488` pins.
5. POST /list-billing-groups with `{"BillingPeriod": "<the same earlier period>", "Filters": {"Arns": ["<that ARN>"]}}` — `BillingGroups` is empty, so the ARN the previous call reported for that period resolves to no group, no `Name`/`Status`/`ComputationPreference` as the generation in force then had them, and no `Size` for a month the group really billed in.
6. Same for POST /list-billing-group-cost-reports and POST /get-billing-group-cost-report with that period: the cost report of a month the group billed in reports nothing (the ARN 404s for the latter), although `state.deleted_billing_groups` still holds the group and `DeletedBillingGroupRecord::existed_in` answers that it existed in that period.
