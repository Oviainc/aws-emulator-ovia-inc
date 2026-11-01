Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e79dd1ec68971fa3f627b273166ce9e704773849

# The delete gate reads only each billing group's CURRENT generation, so a plan a frozen generation still computes with is deleted and that period keeps reporting a plan that exists nowhere

_Filed: 2026-10-29T03:46:03Z_
_Operation: DeletePricingPlan | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e79dd1ec68971fa3f627b273166ce9e704773849`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeletePricingPlan` refuses the delete when a billing group's `ComputationPreference` names the plan — the documented precondition it quotes in its own comment. But the state method that decides it, `billing_groups_referencing_pricing_plan`, looks only at the LIVE record's `group.pricing_plan_arn`. A billing group's SUPERSEDED generations each carry their own frozen `pricing_plan_arn` (`BillingGroupVersion.pricing_plan_arn`), and `ListBillingGroups` answers a period-scoped read from exactly that frozen value (`version.pricing_plan_arn`).

So a plan that a group computed with in an earlier billing period, and was repointed away from by an `UpdateBillingGroup` made in a LATER period (which is what makes `put_billing_group` freeze the old generation), is no longer seen by the gate and is deleted with 200. Afterwards:

* `ListBillingGroups` with `BillingPeriod` = the earlier period reports that group with `ComputationPreference.PricingPlanArn` = the deleted plan's ARN;
* `ListPricingPlans` reports no such plan for any period, because the record was removed from `pricing_plans`;
* `UpdateBillingGroup` / `CreateBillingGroup` refuse that very ARN with the `MISSING_PRICINGPLAN` `ValidationException` ("the pricing plan ... does not exist"), and `UpdatePricingPlan` answers it 404.

So two readers of the same association disagree and one of them hands the caller a plan ARN that resolves to nothing — the same defect class the crate already acknowledges for the sibling resource (`remove_billing_group` / `remove_custom_line_item` strip every association naming the resource they remove). The fix is to decide the gate against every generation of each group, the same generations `ListBillingGroups` reports from, rather than only the live one.

## Evidence

The documented precondition, from the model (`smithy_query(query="doc",shape="DeletePricingPlan")`):
```
Deletes a pricing plan. The pricing plan must not be associated with any billing groups to delete successfully.
```

The gate, and the comment that quotes that sentence — `src/ops/delete_pricing_plan.rs:203-211`:
```
        // "The pricing plan must not be associated with any billing groups to delete
        // successfully." A group computes its bill WITH the plan its
        // `ComputationPreference` names, so deleting the plan under it is the
        // "inconsistent state by ... deleting a resource" the modelled
        // `ConflictException` describes, and `ConflictExceptionReason` enumerates the
        // case by name. The caller points the group at another plan, or deletes the
        // group, first.
        let holders = state.billing_groups_referencing_pricing_plan(&plan_arn);
        if let Some(group_arn) = holders.first() {
```

The state method it decides from, which reads only the LIVE record — `src/state.rs:3113-3119`:
```
    pub fn billing_groups_referencing_pricing_plan(&self, plan_arn: &str) -> Vec<String> {
        self.billing_groups
            .iter()
            .filter(|(_, group)| normalize_pricing_plan_arn(&group.pricing_plan_arn) == plan_arn)
            .map(|(arn, _)| arn.clone())
            .collect()
    }
```

The frozen generations the gate never sees — `src/state.rs:1474-1475` and `:1322-1331`:
```
    /// `ComputationPreference.PricingPlanArn` as this generation carried it.
    pub pricing_plan_arn: String,
```
```
    pub fn current_version(&self) -> BillingGroupVersion {
        BillingGroupVersion {
            name: self.name.clone(),
            description: self.description.clone(),
            pricing_plan_arn: self.pricing_plan_arn.clone(),
```
frozen by `src/state.rs:3054-3064`:
```
    pub fn put_billing_group(&mut self, mut updated: BillingGroupRecord) {
        let arn = updated.arn.clone();
        let superseded = self
            .billing_groups
            .get(&arn)
            .filter(|previous| {
                previous.version_start_billing_period() != updated.version_start_billing_period()
            })
            .map(BillingGroupRecord::current_version);
        if let Some(superseded) = superseded {
            updated.superseded_versions.push(superseded);
```

The reader that then reports the dangling ARN — `src/ops/list_billing_groups.rs:222-224`:
```
        computation_preference: Some(ComputationPreference {
            pricing_plan_arn: version.pricing_plan_arn.clone(),
        }),
```

The other side of the disagreement — `src/ops/update_billing_group.rs:247-257`:
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

Test coverage: none reaches a superseded generation. `tests/all/pricing_plan_delete.rs:427` (`a_plan_a_billing_group_computes_with_cannot_be_deleted`) creates the group with the plan and never repoints it, and `tests/all/pricing_plan_delete.rs:295` (`delete_removes_the_addressed_plan_and_frees_its_name`) uses no billing group at all, so no suite exercises a group whose frozen generation still names the deleted plan.

## Steps to Reproduce

1. Designate the emulated account as the payer so the control plane is reachable.
2. In billing period M: POST /create-pricing-plan twice (keep ARNs P1 and P2), then POST /create-billing-group with {"Name":"G","AccountGrouping":{...},"ComputationPreference":{"PricingPlanArn":P1}} (keep the group ARN G).
3. Confirm POST /list-billing-groups with {"BillingPeriod":M} reports G with ComputationPreference.PricingPlanArn = P1.
4. Advance the store's clock into the NEXT billing period M+1 (so put_billing_group freezes the current generation instead of replacing it).
5. In M+1: POST /update-billing-group with {"Arn":G,"ComputationPreference":{"PricingPlanArn":P2}} — answered 200; the generation carrying P1 is now a superseded one covering period M.
6. Still in M+1: POST /delete-pricing-plan with {"Arn":P1} — answered 200, because billing_groups_referencing_pricing_plan sees only the live pricing_plan_arn, which is now P2.
7. POST /list-billing-groups with {"BillingPeriod":M} — it still reports G with ComputationPreference.PricingPlanArn = P1, while POST /list-pricing-plans with {"BillingPeriod":M} does not report P1, POST /update-pricing-plan with {"Arn":P1} answers 404, and POST /update-billing-group with {"Arn":G,"ComputationPreference":{"PricingPlanArn":P1}} answers 400 MISSING_PRICINGPLAN "the pricing plan ... does not exist".
8. Compare: DeleteBillingGroup and DeleteCustomLineItem strip every association naming the resource they remove, so no reader of theirs can report one that resolves nowhere.
