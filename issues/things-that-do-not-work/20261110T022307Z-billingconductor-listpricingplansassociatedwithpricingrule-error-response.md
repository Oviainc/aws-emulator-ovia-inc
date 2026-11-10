Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 505d23cb73aeec2b6bc1a547ffda8ada5cc353f2

# The pricing-rule existence gate is not billing-period scoped, so a period that held no such rule is answered 200 instead of the modelled 404

_Filed: 2026-11-10T02:23:07Z_
_Operation: ListPricingPlansAssociatedWithPricingRule | Class: error-response_

> _Note: this issue was identified against the code base at commit `505d23cb73aeec2b6bc1a547ffda8ada5cc353f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListPricingPlansAssociatedWithPricingRule` is a period-scoped read: `BillingPeriod` selects the month the associations are listed for. The handler resolves the rule's EXISTENCE from the live `pricing_rules` map alone, before it has even computed the billing period, and then narrows only the ASSOCIATIONS to that period. So a request naming a `BillingPeriod` that precedes the period the rule was created in is answered HTTP 200, echoing `PricingRuleArn` back with no `PricingPlanArns`, as though the rule existed then and simply associated no plan.

The crate already owns the period-scoped existence predicate and the sibling listing of the same resource uses it: `ListPricingRules` filters on `rule.exists_in(&billing_period)` and its own comment states the rule — "a rule is reported only for the periods it existed in". `exists_in` is a `VersionedResource` method available on `PricingRuleRecord` ("A resource does not exist in a period that precedes the one it was created in"), so the two readers of the same stored rule disagree about whether that rule existed in a closed period: one omits it entirely, the other answers for it.

This handler's own comment states the intent that the code then fails to carry out for the period being read: "the rule is the resource the request references, so a rule this account does not hold is the modelled 404" — in the period asked about this account held no such rule, yet the 404 is not given.

The sibling cross-reference listing `ListPricingRulesAssociatedToPricingPlan` has the same shape: its existence answer comes from `state.associated_pricing_rule_arns`, which resolves `self.pricing_plans.get(plan_arn)` with no period scoping at all, so a plan is likewise answered for billing periods before it was created.

## Evidence

The crate's period-scoped rule for this very resource, and the helper that decides it — `src/ops/list_pricing_rules.rs:217-223` and `:234-242`:

        // `BillingPeriod` selects the month the listing is READ FOR — "the billing
        // period for which the described pricing rules are applicable" — and a request
        // that names no period asks about the current one. So a rule is reported only
        // for the periods it existed in, with the values that held in the period asked
        // about: the generation in force then
    ...
        let selected: Vec<(String, &crate::state::PricingRuleRecord)> = state
            .pricing_rules
            .iter()
            .filter(|(arn, rule)| {
                wanted.as_ref().is_none_or(|wanted| wanted.contains(*arn))
                    && rule.exists_in(&billing_period)
            })

The helper itself — `src/state.rs:1059-1073`:

    /// Does this resource exist in `billing_period`? A resource does not exist in a
    /// period that precedes the one it was created in, and a value that is not a billing
    /// period at all names no period a resource exists in.
    ...
    fn exists_in(&self, billing_period: &str) -> bool {
        match (
            billing_period_key(billing_period),
            billing_period_key(&self.creation_billing_period()),
        ) {
            (Some(wanted), Some(created)) => wanted >= created,
            _ => false,
        }
    }

This handler skips it, gating on the live map BEFORE the period is read — `src/ops/list_pricing_plans_associated_with_pricing_rule.rs:179-201`:

        // The associations of a rule nobody created are not an empty set: the rule is
        // the resource the request references, so a rule this account does not hold is
        // the modelled 404. There are no AWS-managed pricing rules — the
        // `PricingRuleArn` pattern allows only a 12-digit account segment — so what is
        // stored is the whole universe of rules.
        if !state.pricing_rules.contains_key(&pricing_rule_arn) {
            return Err(
                ListPricingPlansAssociatedWithPricingRuleErrorModeled::ResourceNotFoundException(
    ...
        // "The pricing plan billing period for which associations will be listed":
        // the read is scoped to ONE period, and an association is in force only from
        // the period it was made in onward. A request that names no period asks about
        // the current one.
        let billing_period = input
            .billing_period

The sibling listing of the plan side resolves existence the same unscoped way — `src/state.rs:3801-3810`:

    pub fn associated_pricing_rule_arns(
        &self,
        plan_arn: &str,
        billing_period: &str,
    ) -> Option<Vec<String>> {
        match self.pricing_plans.get(plan_arn) {
            Some(plan) => Some(plan.associated_rule_arns(billing_period)),
            None => is_aws_managed_pricing_plan(plan_arn).then(Vec::new),
        }
    }

Test coverage: `tests/all/pricing_rule_association_billing_periods.rs` and `tests/all/pricing_plans_associated_with_pricing_rule.rs` exercise this listing over billing periods, but no test drives it with a `BillingPeriod` earlier than the rule's creation period, so the current 200 is unpinned.

## Steps to Reproduce

1. Start the emulator with the emulated account designated the payer account (as the suites do: `engine.state::<BillingConductorState>().lock().payer_account_id = Some(emulator_account_id())`).
2. POST /create-pricing-rule with `{"Name":"r1","Scope":"GLOBAL","Type":"MARKUP","ModifierPercentage":10.0}` -> note the returned rule ARN R.
3. POST /list-pricing-rules with `{"BillingPeriod":"2020-01"}` -> `PricingRules` does NOT contain R (the rule did not exist then, per `rule.exists_in`).
4. POST /list-pricing-plans-associated-with-pricing-rule with `{"PricingRuleArn": R, "BillingPeriod":"2020-01"}`.
5. Observed: HTTP 200 with `PricingRuleArn` echoed and no `PricingPlanArns`. Expected (per `src/ops/list_pricing_rules.rs:217-223` plus `exists_in` at `src/state.rs:1059-1073`): the modelled 404 ResourceNotFoundException, because the billing period being read held no such pricing rule.
