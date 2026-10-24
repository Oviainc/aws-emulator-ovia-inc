Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ceffded050eaaeab66f25b25154c54549736906e

# CreatePricingRule accepts a rule carrying BOTH ModifierPercentage and Tiering (UpdatePricingRule refuses the same combination)

_Filed: 2026-10-24T02:49:10Z_
_Operation: CreatePricingRule | Class: validation_

> _Note: this issue was identified against the code base at commit `ceffded050eaaeab66f25b25154c54549736906e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a pricing rule prices EITHER by percentage (`MARKUP`/`DISCOUNT`) or by tiers (`TIERING`), never both, and the model enumerates the refusal for each direction: `ILLEGAL_MODIFIER_PERCENTAGE` for a `TIERING` rule that carries a `ModifierPercentage`, `ILLEGAL_TIERING_INPUT` for a `MARKUP`/`DISCOUNT` rule that carries a `Tiering` configuration. So `CreatePricingRule` should refuse a request that names the rate member its `Type` does not price by.

Actual: the create handler only checks that the REQUIRED member for the type is PRESENT (Tiering for TIERING, ModifierPercentage for MARKUP/DISCOUNT). A request with `Type=TIERING` plus a `ModifierPercentage`, or `Type=MARKUP` plus a `Tiering` block, is accepted with 200 and BOTH rate members are persisted on the record — a state the crate's own `PricingRuleRate` doc says cannot exist ("of which a rule ever holds exactly one"). The contradiction is observable: `UpdatePricingRule` refuses exactly that combination through `crate::state::reconcile_pricing_rule_rate`, so the same illegal rule is rejected on update but minted on create, and `ListPricingRules`/`UpdatePricingRule` then report a rule with both a `ModifierPercentage` and a `Tiering`.

## Evidence

Smithy model: `ValidationExceptionReason` includes `ILLEGAL_MODIFIER_PERCENTAGE` and `ILLEGAL_TIERING_INPUT`; `PricingRuleType` = DISCOUNT | MARKUP | TIERING. Handler: `src/ops/create_pricing_rule.rs` lines 281-300 (the `match input.r#type` only guards the ABSENT member) and lines 326-350 (stores `modifier_percentage` and `tiering` independently). Contrast `src/state.rs::reconcile_pricing_rule_rate` (lines ~221-261) used by `src/ops/update_pricing_rule.rs::merge`, which returns `ILLEGAL_MODIFIER_PERCENTAGE` / `ILLEGAL_TIERING_INPUT` for the same request shapes. Test coverage: `tests/all/pricing_rule_core_loop.rs` (lines 342-424) covers only the missing-member direction on create; `tests/all/pricing_rule_update.rs` (lines 762-787) covers the both-members direction on UPDATE only.

## Steps to Reproduce

1. Authorize as the payer account.
2. POST /create-pricing-rule with {"Name":"R1","Scope":"GLOBAL","Type":"TIERING","Tiering":{"FreeTier":{"Activated":true}},"ModifierPercentage":10}.
3. Observe HTTP 200 and a minted rule ARN, where the modelled refusal is 400 ValidationException with reason ILLEGAL_MODIFIER_PERCENTAGE.
4. PUT /update-pricing-rule with {"Arn":"<rule>","Type":"TIERING","ModifierPercentage":10} and observe that the SAME combination is refused 400 — the inconsistency between the two paths.
5. Repeat the create with {"Type":"MARKUP","ModifierPercentage":10,"Tiering":{"FreeTier":{"Activated":true}}} and observe 200 instead of an ILLEGAL_TIERING_INPUT refusal.
