Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: f5120b87affb905e7d968cf57a1e02e568fab681

# ListPricingPlansAssociatedWithPricingRule ignores BillingPeriod and reports today's associations for every period

_Filed: 2026-10-24T15:03:06Z_
_Operation: ListPricingPlansAssociatedWithPricingRule | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `f5120b87affb905e7d968cf57a1e02e568fab681`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `BillingPeriod` is "The pricing plan billing period for which associations will be listed" — a read for a billing period before the rule was associated with a plan (or before either resource existed) must not report that plan, and a read for a later period reports the associations in force then.

Actual: the member is echoed in the response and nothing else. The plans reported come from `state.pricing_plans_referencing_rule(&pricing_rule_arn)`, which scans every plan's current `PricingRuleArns`; no association carries an effective billing period, so 2020-01 and 2030-12 both report exactly the associations that exist right now.

## Evidence

Smithy model: `ListPricingPlansAssociatedWithPricingRuleInput$BillingPeriod` documented as "The pricing plan billing period for which associations will be listed"; the output carries `BillingPeriod` beside `PricingPlanArns`. Handler: `src/ops/list_pricing_plans_associated_with_pricing_rule.rs` lines 197-224 — the handler's own comment says "This emulator keeps no per-billing-period history, so every period reports the rule's current associations"; `pricing_plans_referencing_rule` (src/state.rs lines 1042-1052) has no period parameter. Test coverage: `tests/all/pricing_plans_associated_with_pricing_rule.rs` lines 487-529 assert only that the period is echoed back and that associations are reported when no period is named.

## Steps to Reproduce

1. Authorize as the payer account; POST /create-pricing-rule to get a rule ARN and POST /create-pricing-plan to get a plan ARN
2. PUT /associate-pricing-rules with {"Arn":"<plan>","PricingRuleArns":["<rule>"]}
3. POST /list-pricing-plans-associated-with-pricing-rule with {"PricingRuleArn":"<rule>","BillingPeriod":"2020-01"}
4. Observe HTTP 200 reporting the plan for that long-past period, where the real service reports no association as of a period that precedes it
