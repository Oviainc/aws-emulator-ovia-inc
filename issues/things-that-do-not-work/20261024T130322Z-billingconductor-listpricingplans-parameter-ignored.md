Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: f04518a3ee65f2e7815d98c430f437f66966ee67

# ListPricingPlans ignores BillingPeriod and reports today's pricing plans (and today's Size) for every period, including periods before the plan existed

_Filed: 2026-10-24T13:03:22Z_
_Operation: ListPricingPlans | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `f04518a3ee65f2e7815d98c430f437f66966ee67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `BillingPeriod` selects the month the listing is read for — "The billing period for which the described pricing plans are applicable". Billing Conductor state is billing-period scoped, so a request naming a period BEFORE a plan was created must not report that plan, and `PricingPlanListElement.Size` (the count of rules associated with the plan) must be the count that held in the named period, not the current one.

Actual: the handler never uses `input.billing_period` for selection — it reports every stored plan for any period and echoes the requested period back in the response (`src/ops/list_pricing_plans.rs` lines 196-222). `Size` is computed from the plan's CURRENT rule associations. A client reading a closed billing period sees plans that did not exist then and association counts that were different then. (The same per-period state model is missing in `ListPricingRulesAssociatedToPricingPlan` / `ListPricingPlansAssociatedWithPricingRule`, which likewise echo the period and report current associations, so one state-model fix covers them.)

## Evidence

Smithy model: `ListPricingPlansInput$BillingPeriod` ("The billing period for which the described pricing plans are applicable") and `ListPricingPlansOutput$BillingPeriod`; the service models billing-period-scoped reads throughout (`ListBillingGroups`, `ListCustomLineItems`, `GetBillingGroupCostReport`, `ListCustomLineItemVersions`). Handler: `src/ops/list_pricing_plans.rs` lines 196-222 — selection depends only on `Filters.Arns`; the inline comment states "This emulator keeps no per-billing-period history of a plan, so every period reports the plans as they stand now". Test coverage: `tests/all/pricing_plan_list.rs::the_reported_period_is_the_one_the_request_named_and_otherwise_absent` (lines 550-600) asserts the divergent behaviour — a plan created now is expected in the 2020-01 page. Precedent: the same gap is already filed for `ListBillingGroups` and `ListAccountAssociations` (open issues `...-listbillinggroups-parameter-ignored`, `...-listaccountassociations-parameter-ignored`); this is the pricing-plan surface, which those do not cover.

## Steps to Reproduce

1. Authorize as the payer account; POST /create-pricing-plan with {"Name":"Plan1"}
2. POST /list-pricing-plans with {"BillingPeriod":"2020-01"}
3. Observe HTTP 200 whose PricingPlans contains Plan1 (created in the current period) and whose BillingPeriod is echoed as 2020-01
4. POST /create-pricing-rule then PUT /associate-pricing-rules to attach a rule to Plan1
5. POST /list-pricing-plans with {"BillingPeriod":"<a period before the association>"} -> Size already reflects the new association instead of the count that held in that period
