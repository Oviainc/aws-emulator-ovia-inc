Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 18f3fa67e5a7c5f14368b31f341e0aff6f4ace0d

# A billing-group delete erases the group's custom line items from every EARLIER billing period, although the deleted group is still reported for those periods

_Filed: 2026-11-10T14:20:34Z_
_Operation: DeleteBillingGroup | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `18f3fa67e5a7c5f14368b31f341e0aff6f4ace0d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteBillingGroup` keeps the group's record precisely so that the billing periods it billed in go on reporting it: `remove_billing_group` records a `DeletedBillingGroupRecord`, `billing_groups_in` folds that record into every period-scoped read, and `GetBillingGroupCostReport` explicitly resolves a deleted group for the periods `reported_in` answers for. For the memberships the group held, the same function applies the crate's "a delete takes effect in the period it is made in" rule: `release_account` ENDS each membership at the current billing period so the earlier (closed) periods still report the accounts the group held then.

The group's CUSTOM LINE ITEMS get the opposite treatment in the same function: they are removed outright with `remove_custom_line_item`, and every resource association naming the group is stripped with an unconditional `retain`. Neither write is billing-period scoped, so the charges the group really carried in the periods it is still reported for disappear from all of them.

The observable result is a hollow answer on the paths that were built to keep answering for those periods:
- `GetBillingGroupCostReport` and `ListBillingGroupCostReports`, asked about a billing period BEFORE the delete, still report the deleted group — but with `ProformaCost`/`Margin` of 0, because `proforma_custom_line_item_charges` finds no items for it. Before the delete the same request reported the item's charge.
- `ListCustomLineItems` for that same earlier period reports no item at all, although the item was in effect there and the group it applies to is still reported there.

The crate already owns the bounded write this needs: `remove_custom_line_item_periods`, documented as "the bounded form of the rule a delete that names no range enforces by removing the record outright", combined with `BillingPeriodSpan::ending_now()` — the very span `release_account` and `PricingPlanRecord::end_rule_associations` use for this rule. Applying it with `ending_now()` still removes outright every item that only ever applied from the current period on (so an item created and orphaned in one period still vanishes whole), and leaves the earlier periods' charges intact.

## Evidence

The rule `remove_billing_group` states about itself — `src/state.rs:3306-3311`:

    /// What the delete does NOT erase is the group's own record of the billing periods
    /// it billed in: the removed record is kept as a
    /// [`DeletedBillingGroupRecord`](crate::state::DeletedBillingGroupRecord), so the
    /// name it occupied in those periods stays occupied for them and a read scoped to one
    /// of the periods before the delete still reports the group as it stood then
    /// ([`Self::billing_groups_in`]).

and the handler's own module doc — `src/ops/delete_billing_group.rs:7-9`:

    //! What the delete does NOT erase is the billing periods the group BILLED in: a
    //! billing group bills a consolidated family for a billing period, so the record of
    //! the deleted group is kept ([`crate::state::DeletedBillingGroupRecord`])

The SAME function applies the period-scoped rule to the memberships — `src/state.rs:3337-3348`:

        // The memberships the group held END with it, rather than being forgotten: a
        // read scoped to a billing period the group existed in still has to report
        // the accounts it held then.
        ...
        for account_id in released {
            self.release_account(&account_id, group_arn);
        }

but removes the group's custom line items, and every association naming it, with no period scoping at all — `src/state.rs:3317-3335`:

        // A custom line item "applies to" one billing group, so it cannot outlive
        // it; and an association names a resource that exists, so every association
        // of the deleted group or of one of its items goes with them.
        let orphaned: Vec<String> = self
            .custom_line_items
            .iter()
            .filter(|(_, item)| item.billing_group_arn == group_arn)
            .map(|(arn, _)| arn.clone())
            .collect();
        ...
        for arn in &orphaned {
            self.remove_custom_line_item(arn);
        }
        for item in self.custom_line_items.values_mut() {
            item.associated_resources
                .retain(|association| association.arn != group_arn);
        }

The bound the rest of the store ends a record at — `src/state.rs:336-346` (`BillingPeriodSpan::ending_now`):

    /// A call that stops a billing-period-scoped record standing, and names no range of
    /// its own, takes effect FROM the period it is made in and never earlier — the
    /// periods before it are closed bills the record really was in effect in, so a read
    /// scoped to one of them still reports it. That is the bound the rest of this store
    /// ends a record at ([`...::release_account`], [`PricingPlanRecord::end_rule_associations`])

and the bounded write for exactly this resource, which `remove_billing_group` does not call — `src/state.rs:3441-3447`, `3455-3460`:

    /// It is the bounded form of the rule a delete that names no range enforces by removing
    /// the record outright ([`Self::remove_custom_line_item`]): a period the item applies in
    /// in no generation of it holds no association of it either
    ...
    pub fn remove_custom_line_item_periods(
        &mut self,
        item_arn: &str,
        removed: &BillingPeriodSpan,
        now: i64,
    ) -> Option<BoundedRemoval> {

The reads that still answer for those periods and therefore answer 0 — `src/state.rs:3270-3277` (`billing_groups_in` folds the deleted record in):

        for deleted in &self.deleted_billing_groups {
            if !deleted.reported_in(billing_period) {
                continue;
            }
            if let Some(version) = deleted.record.effective_in(billing_period) {
                groups.insert(deleted.record.arn.clone(), (&deleted.record, version));
            }
        }

`src/ops/get_billing_group_cost_report.rs:287-298`:

        // A group the store no longer holds is the subject of a report only for the
        // billing periods before the delete took it
        // (`crate::state::DeletedBillingGroupRecord::reported_in`)
        if let Some(deleted) = deleted {
            if !billing_periods
                .iter()
                .any(|period| deleted.reported_in(period))
            {
                return Err(no_such_billing_group(&group_arn));
            }
        }

and `src/ops/list_billing_group_cost_reports.rs:205` + `:236-239`:

        for (arn, (group, _version)) in state.billing_groups_in(&billing_period) {
        ...
                let proforma_cost: f64 = state
                    .proforma_custom_line_item_charges(&group.arn, &billing_period)
                    .values()
                    .sum();

Test coverage: none reaches the case. `tests/all/deleted_billing_group_period_reads.rs` drives all four period-scoped reads of a deleted group but creates no custom line item (grep for `create-custom-line-item`/`ProformaCost` in that file: no matches). `tests/all/delete_custom_line_item.rs:677-693` pins only that an item whose group was deleted is no longer a LIVE item ("An item whose billing group was deleted goes with it"), which a narrowing from `ending_now()` still satisfies for an item created in the current period, since the narrowing empties it and `remove_custom_line_item_periods` then removes the record (`src/state.rs:3505-3511`).

## Steps to Reproduce

1. Designate the emulated account as the payer so the control plane is reachable (set `ComAmazonawsBillingconductorAwsbillingconductorState::payer_account_id = Some(engine::emulator_account_id())`, as every suite does).
2. POST /create-pricing-plan, then POST /create-billing-group -> group G. Call the current billing period M and the previous one M-1.
3. POST /create-custom-line-item against G with ChargeDetails {"Type":"FEE","Flat":{"ChargeValue":100}} and BillingPeriodRange {"InclusiveStartBillingPeriod":"M-1"} (accepted: M-1 is the previous billing period) -> item I, applying from M-1 on.
4. POST /get-billing-group-cost-report with {"Arn": G, "BillingPeriodRange":{"InclusiveStartBillingPeriod":"M-1","ExclusiveEndBillingPeriod":"M"}} -> ProformaCost "100". POST /list-custom-line-items with {"BillingPeriod":"M-1"} -> I is reported.
5. POST /delete-billing-group with {"Arn": G}.
6. Re-run the read of step 4. Observed: HTTP 200 (G is still the subject of a report for M-1, since `reported_in` answers for the periods before the delete) with ProformaCost "0", Margin "0" — the charge the period really carried is gone. POST /list-billing-group-cost-reports with {"BillingPeriod":"M-1"} reports G with ProformaCost "0" too, and POST /list-custom-line-items with {"BillingPeriod":"M-1"} reports no item at all.
7. Expected, per `src/state.rs:3306-3311` ("a read scoped to one of the periods before the delete still reports the group as it stood then") and the bound `src/state.rs:336-346` states for every other record a delete ends: the item is narrowed out of M and every later period only — so M-1 still reports it and still prices it at 100 — exactly as the group's own memberships are ended at M by `release_account` in the same function.
8. Contrast within one call: POST /list-account-associations with {"BillingPeriod":"M-1"} still reports the accounts G held then (membership ended, not erased), while the cost report for the same period reports none of the charges it held.
