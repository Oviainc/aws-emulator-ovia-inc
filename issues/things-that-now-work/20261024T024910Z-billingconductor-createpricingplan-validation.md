Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ceffded050eaaeab66f25b25154c54549736906e

# CreatePricingPlan accepts PricingRuleArns that name no pricing rule (and duplicate rule ARNs)

_Filed: 2026-10-24T02:49:10Z_
_Operation: CreatePricingPlan | Class: validation_

> _Note: this issue was identified against the code base at commit `ceffded050eaaeab66f25b25154c54549736906e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a CreatePricingPlan whose `PricingRuleArns` names a pricing rule that does not exist is refused — the operation is the ONLY `Create*` in this service that declares `ResourceNotFoundException` (HTTP 404), and the service's `ValidationExceptionReason` enumerates `PRICINGRULES_NOT_EXIST` for exactly this case; likewise a list that names one rule twice (modelled reason `DUPLICATE_PRICINGRULE_ARNS`) is refused.

Actual: the handler stores the submitted `PricingRuleArns` verbatim and answers 200 with a freshly minted plan ARN. The plan then holds dangling references that later reads report as real associations: `ListPricingRulesAssociatedToPricingPlan` lists rule ARNs for rules that were never created, and `UpdatePricingPlan`'s `Size` ("the pricing rules count that's currently associated with this pricing plan") counts them. Duplicates are silently collapsed on read instead of being refused on write.

## Evidence

Smithy model: `CreatePricingPlan` errors = AccessDenied, Conflict, InternalServer, **ResourceNotFoundException (404)**, ServiceLimitExceeded, Throttling, ValidationException; `ValidationExceptionReason` includes `PRICINGRULES_NOT_EXIST` and `DUPLICATE_PRICINGRULE_ARNS`. Handler: `src/ops/create_pricing_plan.rs` lines ~183-239 — it checks the ClientToken replay, the name-uniqueness index and the store cap, then writes `pricing_rule_arns: input.pricing_rule_arns.unwrap_or_default()` with no existence or duplicate check (contrast `src/ops/create_billing_group.rs`, which DOES refuse a `ComputationPreference.PricingPlanArn` that names no stored plan with `MISSING_PRICINGPLAN`). Test coverage: no test asserts a refusal; `tests/all/pricing_plan_update.rs::size_reports_the_number_of_associated_pricing_rules` (lines 440-498) deliberately creates plans referencing never-created rule ids (`rule_id(7)`, `rule_arn(&rule_id(9))`, 30 synthetic ids) and expects 200, so it currently pins the permissive behaviour and would need updating.

## Steps to Reproduce

1. Start the emulator with the billingconductor store reachable and the caller authorized as the payer account (as the test harness does).
2. POST /create-pricing-plan with {"Name":"PlanA","PricingRuleArns":["ABCDEFGHIJ"]} where no pricing rule with that id was ever created.
3. Observe HTTP 200 with a minted plan Arn instead of a refusal naming the missing rule.
4. POST /list-pricing-rules-associated-to-pricing-plan with that plan Arn and observe the phantom rule ARN reported as an association (and `Size` on a subsequent PUT /update-pricing-plan counting it).
5. Also POST /create-pricing-plan with the same rule id listed twice and observe 200 instead of a DUPLICATE_PRICINGRULE_ARNS refusal.
