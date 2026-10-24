Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: f5120b87affb905e7d968cf57a1e02e568fab681

# ListPricingRulesAssociatedToPricingPlan ignores BillingPeriod and reports today's associations for every period

_Filed: 2026-10-24T15:03:06Z_
_Operation: ListPricingRulesAssociatedToPricingPlan | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `f5120b87affb905e7d968cf57a1e02e568fab681`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `BillingPeriod` is "the billing period for which the pricing rule associations are listed" — rule/plan associations in Billing Conductor take effect per billing period, so a read for a period BEFORE a rule was associated with the plan (or before the plan existed) must not report that association, and a read for a later period reports the associations in force then.

Actual: the input member is read only to be echoed back in the response. The selection is `state.associated_pricing_rule_arns(&pricing_plan_arn)`, which returns the plan record's current `pricing_rule_arns` with no period dimension at all — the state keeps no history of when an association was made (`PricingPlanRecord.pricing_rule_arns: Vec<String>`), so every billing period, past or future, reports exactly the same set. A caller auditing a closed billing period is told about associations that did not exist then.

## Evidence

Smithy model: `ListPricingRulesAssociatedToPricingPlanInput$BillingPeriod` (pattern ^\d{4}-(0?[1-9]|1[012])$) is documented as "The billing period for which the pricing rule associations are listed", and the output echoes `BillingPeriod` beside the rule ARNs. Handler: `src/ops/list_pricing_rules_associated_to_pricing_plan.rs` lines 186-222 — `input.billing_period` is only placed in the output ("This emulator keeps no per-billing-period history, so every period reports the plan's current associations"); state: `PricingPlanRecord` (src/state.rs lines 331-339) stores a flat ARN list with no effective period. Test coverage: `tests/all/pricing_rules_associated_to_pricing_plan.rs` lines 326-367 assert only that the requested period is echoed and that associations are still reported when none is named — nothing asserts period-sensitive membership. Same class as the already-filed ListPricingPlans / ListPricingRules BillingPeriod issues, but a different operation.

## Steps to Reproduce

1. Authorize as the payer account; POST /create-pricing-rule to get a rule ARN and POST /create-pricing-plan to get a plan ARN
2. PUT /associate-pricing-rules with {"Arn":"<plan>","PricingRuleArns":["<rule>"]}
3. POST /list-pricing-rules-associated-to-pricing-plan with {"PricingPlanArn":"<plan>","BillingPeriod":"2020-01"} — a period long before the plan or the association existed
4. Observe HTTP 200 with PricingRuleArns containing the rule, where the real service reports no association for that period
