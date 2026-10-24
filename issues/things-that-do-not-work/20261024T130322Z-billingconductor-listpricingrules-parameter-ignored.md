Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: f04518a3ee65f2e7815d98c430f437f66966ee67

# ListPricingRules ignores BillingPeriod and reports today's pricing rules (and today's AssociatedPricingPlanCount) for every period

_Filed: 2026-10-24T13:03:22Z_
_Operation: ListPricingRules | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `f04518a3ee65f2e7815d98c430f437f66966ee67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `BillingPeriod` selects the month the listing is read for — "The billing period for which the described pricing rules are applicable". A rule created after the named period must not be reported for it, and `PricingRuleListElement.AssociatedPricingPlanCount` / `ModifierPercentage` / `Tiering` must be the values effective in that period rather than the current ones.

Actual: the handler ignores `input.billing_period` entirely for selection, reports every stored rule for any requested period and echoes the period back (`src/ops/list_pricing_rules.rs` lines 196-232). `AssociatedPricingPlanCount` is derived from the plans that reference the rule RIGHT NOW (`state.associated_pricing_plan_count`), so a read of a closed billing period reports post-hoc associations and post-hoc modifier percentages.

## Evidence

Smithy model: `ListPricingRulesInput$BillingPeriod` / `ListPricingRulesOutput$BillingPeriod` ("The billing period for which the described pricing rules are applicable"); the whole service is billing-period scoped (custom line item versions, billing group cost reports). Handler: `src/ops/list_pricing_rules.rs` lines 205-232, with the inline comment "This emulator keeps no per-billing-period history of a rule, so every period reports the rules as they stand now". Test coverage: no test asserts period-scoped selection for pricing rules; `tests/all/pricing_rule_core_loop.rs` / `pricing_rule_lifecycle.rs` exercise only the current state. Precedent: the identical gap is already filed for `ListBillingGroups` and `ListAccountAssociations`.

## Steps to Reproduce

1. Authorize as the payer account; POST /create-pricing-rule with {"Name":"Rule1","Scope":"GLOBAL","Type":"MARKUP","ModifierPercentage":10}
2. POST /list-pricing-rules with {"BillingPeriod":"2020-01"}
3. Observe HTTP 200 whose PricingRules contains Rule1 (created in the current period)
4. PUT /update-pricing-rule changing ModifierPercentage to 20, then POST /list-pricing-rules for an earlier BillingPeriod -> the new percentage and the current AssociatedPricingPlanCount are reported for a period in which they did not hold
