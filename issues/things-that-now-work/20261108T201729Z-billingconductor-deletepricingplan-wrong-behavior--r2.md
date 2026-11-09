Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f392f82803b1854878505550560cb55ee4e9c61d

# The pricing-plan delete gate ignores DELETED billing groups, whose generations the period-scoped listing still reports, so a reported ComputationPreference.PricingPlanArn resolves to nothing

_Filed: 2026-11-08T20:17:29Z_
_Operation: DeletePricingPlan | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f392f82803b1854878505550560cb55ee4e9c61d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeletePricingPlan` refuses the delete while a billing group still computes with the plan, and the state method that decides it now walks EVERY generation of each group — but only of the groups the store still holds (`self.billing_groups`). It never looks at `self.deleted_billing_groups`.

The crate's one answer to "which groups does a billing period hold, and as which generation of themselves" — `billing_groups_in`, which `ListBillingGroups`, `ListBillingGroupCostReports` and `GetBillingGroupCostReport` are all answered from — reports a DELETED group for every period from its creation up to (not including) the period of the delete, projecting the generation in force then, and `ListBillingGroups` reports that generation's `pricing_plan_arn` as `ComputationPreference.PricingPlanArn`.

So after a group created in billing period M is deleted in a later period M+1, the plan it computed with is no longer seen by the gate and is deleted with 200. Afterwards `ListBillingGroups` with `BillingPeriod` = M still reports the deleted group with `ComputationPreference.PricingPlanArn` = the deleted plan, while `ListPricingPlans` reports no such plan, `UpdatePricingPlan` answers it 404 and `CreateBillingGroup`/`UpdateBillingGroup` refuse that very ARN with `MISSING_PRICINGPLAN` ("the pricing plan ... does not exist"). That is exactly the outcome the state method's own doc says the gate exists to prevent, and the invariant the test suite for this gate states.

The fix is to decide the gate against the generations of the DELETED groups too — the same records `billing_groups_in` reports from — the way `deleted_billing_group_occupying_name` already treats a deleted group's generations as still reportable for the name it occupied.

## Evidence

The gate and the comment stating the rule it is supposed to enforce — `src/ops/delete_pricing_plan.rs:203-217`:
```
        // "The pricing plan must not be associated with any billing groups to delete
        // successfully." ...
        // Every GENERATION of a stored group counts, not just the one in force now: a
        // group's computation preference is a member a write can change, so a closed
        // billing period reports the plan the generation in force THEN computed with,
        // and a plan still named by any of those generations is a plan a read of this
        // store can still report.
        let holders = state.billing_groups_referencing_pricing_plan(&plan_arn);
        if let Some(group_arn) = holders.first() {
```

The state method it decides from, which walks only the STORED groups — `src/state.rs:3598-3623`:
```
    /// The ARNs of every stored billing group that computes with this pricing plan in
    /// ANY billing period ...
    /// Every generation of a stored group is therefore an association the store can
    /// still report, and a plan one of them names is in use whether or not the LIVE
    /// generation still names it — otherwise the plan could be deleted out from under a
    /// closed period that goes on reporting it, leaving an ARN that resolves to no plan.
    pub fn billing_groups_referencing_pricing_plan(&self, plan_arn: &str) -> Vec<String> {
        self.billing_groups
            .iter()
            .filter(|(_, group)| {
                group
                    .versions()
                    .iter()
                    .any(|version| normalize_pricing_plan_arn(&version.pricing_plan_arn) == plan_arn)
            })
            .map(|(arn, _)| arn.clone())
            .collect()
    }
```
Note `self.billing_groups` only — the deleted-group store is never consulted.

The other side: the crate's ONE period-scoped group read, which DOES report deleted groups and their generations — `src/state.rs:3229-3248`:
```
    pub fn billing_groups_in(
        &self,
        billing_period: &str,
    ) -> BTreeMap<String, (&BillingGroupRecord, BillingGroupVersion)> {
        let mut groups: BTreeMap<String, (&BillingGroupRecord, BillingGroupVersion)> =
            BTreeMap::new();
        for deleted in &self.deleted_billing_groups {
            if !deleted.reported_in(billing_period) {
                continue;
            }
            if let Some(version) = deleted.record.effective_in(billing_period) {
                groups.insert(deleted.record.arn.clone(), (&deleted.record, version));
            }
        }
```
and `src/state.rs:1771-1789`:
```
    /// Does a read of the billing groups scoped to `billing_period` still REPORT this
    /// deleted group? From the period it was created in up to, but NOT including, the
    /// period the delete was made in.
    pub fn reported_in(&self, billing_period: &str) -> bool {
```

The reader that then hands the caller the dangling ARN — `src/ops/list_billing_groups.rs:224-226`:
```
        computation_preference: Some(ComputationPreference {
            pricing_plan_arn: version.pricing_plan_arn.clone(),
        }),
```

The opposite side of the disagreement — `src/ops/update_billing_group.rs:247-257`:
```
        let pricing_plan_arn = match &input.computation_preference {
            Some(preference) => {
                let plan_arn =
                    crate::state::normalize_pricing_plan_arn(&preference.pricing_plan_arn);
                if !state.pricing_plan_exists(&plan_arn) {
                    return Err(validation(
                        "MISSING_PRICINGPLAN",
                        "ComputationPreference.PricingPlanArn",
                        format!("the pricing plan {plan_arn} does not exist"),
                    ));
```

The invariant the existing suite for this gate states, which the deleted-group path breaks — `tests/all/association_delete_gate_spans_billing_periods.rs:7-21`:
```
//! RULE: a delete that is refused while the resource is still associated decides that
//! precondition over every association a period-scoped read of this store can still
//! report, not only the one in force in the current billing period. So
//!
//! * `DeletePricingPlan` is refused while ANY generation of a stored billing group
//!   computes with the plan ...
//! The invariant both serve is that the readers of one association cannot disagree: a
//! plan ARN `ListBillingGroups` reports for a period, and a rule ARN the association
//! readers report for a period, name a resource the store still holds.
```

Test coverage: no suite reaches a DELETED group here. Every row of `tests/all/association_delete_gate_spans_billing_periods.rs` keeps the group stored (it repoints it with `UpdateBillingGroup`), `tests/all/pricing_plan_delete.rs:427` (`a_plan_a_billing_group_computes_with_cannot_be_deleted`) uses a live group, and `tests/all/deleted_billing_group_period_reads.rs:484,602` asserts the deleted group's `ComputationPreference.PricingPlanArn` is still reported for earlier periods without ever deleting that plan.

## Steps to Reproduce

1. Designate the emulated account as the payer so the control plane is reachable.
2. In billing period M: POST /create-pricing-plan (keep the ARN P), then POST /create-billing-group with {"Name":"G","AccountGrouping":{"LinkedAccountIds":[]},"ComputationPreference":{"PricingPlanArn":P}} (keep the group ARN G).
3. Confirm POST /list-billing-groups with {"BillingPeriod":M} reports G with ComputationPreference.PricingPlanArn = P.
4. Advance the store's clock into the NEXT billing period M+1 (the harness in tests/all/deleted_billing_group_period_reads.rs backdates the stored instants for exactly this), so the delete lands in a later period than the creation and `reported_in` keeps M.
5. In M+1: POST /delete-billing-group with {"Arn":G} — answered 200. The deleted record is kept, so POST /list-billing-groups with {"BillingPeriod":M} still reports G with ComputationPreference.PricingPlanArn = P.
6. Still in M+1: POST /delete-pricing-plan with {"Arn":P} — answered 200, because billing_groups_referencing_pricing_plan iterates only self.billing_groups, which no longer holds G.
7. POST /list-billing-groups with {"BillingPeriod":M} — it STILL reports G with ComputationPreference.PricingPlanArn = P, while POST /list-pricing-plans with {"BillingPeriod":M} reports no P, POST /update-pricing-plan with {"Arn":P} answers 404, and POST /create-billing-group naming P answers 400 MISSING_PRICINGPLAN "the pricing plan ... does not exist".
8. Compare with the same sequence where G is NOT deleted: step 6 is refused 409 ConflictException PRICING_PLAN_ATTACHED_TO_BILLING_GROUP_DELETE_CONFLICT.
