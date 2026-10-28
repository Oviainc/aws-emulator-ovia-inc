Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2a79338fc5a5c4ae4fe5ea21e80da32dd32c19f2

# A CustomTier whose EndRangeExclusive is at or below BeginRangeInclusive is stored, although the crate refuses every other half-open Inclusive/Exclusive pair that covers nothing

_Filed: 2026-10-28T17:22:14Z_
_Operation: CreatePricingRule | Class: validation_

> _Note: this issue was identified against the code base at commit `2a79338fc5a5c4ae4fe5ea21e80da32dd32c19f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreatePricingRule` (and `UpdatePricingRule`) copy `Tiering.CustomTiers` straight onto the stored record without ever comparing a tier's `BeginRangeInclusive` against its `EndRangeExclusive`. A request such as `{"Type":"TIERING","Tiering":{"CustomTiers":[{"BeginRangeInclusive":100.0,"EndRangeExclusive":50.0,"RateValue":1.0}]}}` is accepted 200 and persisted, so the store holds a TIERING pricing rule whose only tier prices no usage at all — the rule reads back from `ListPricingRules` / `UpdatePricingRule` exactly as sent, and nothing in the crate can ever apply it.

This crate states and enforces the opposite rule for every OTHER half-open `Inclusive…`/`Exclusive…` pair it stores: `state::billing_period_range_is_ordered` spells the rule out from the member names, and `check_billing_period_range_is_ordered` is called by `CreateCustomLineItem`, `UpdateCustomLineItem`, `DeleteCustomLineItem`, `BatchAssociateResourcesToCustomLineItem` and `BatchDisassociateResourcesFromCustomLineItem`, while `GetBillingGroupCostReport` and `ListCustomLineItemVersions` hand-roll the same comparison. `CustomTier.BeginRangeInclusive` / `CustomTier.EndRangeExclusive` is the one half-open pair, named by the same convention, that goes in unchecked.

The refusal to answer with already exists in the crate: `state::reconcile_pricing_rule_rate` refuses the other two tiering faults with `ILLEGAL_TIERING_INPUT`, the `ValidationExceptionReason` the model enumerates for an illegal tiering input. Nothing distinguishes an inverted tier range from those.

The same unchecked copy also lets one request store two tiers covering the same usage range (`[0,100)` twice), so which rate applies to a unit of usage is undecidable from the stored rule.

## Evidence

The crate's own rule for a half-open `Inclusive…`/`Exclusive…` pair — `src/state.rs:3966-3972`:
```
/// Is a `CustomLineItemBillingPeriodRange` ordered, i.e. does it describe a non-empty
/// span of billing periods?
///
/// The member names carry the rule: `InclusiveStartBillingPeriod` is in the range and
/// `ExclusiveEndBillingPeriod` is not, so an end equal to or earlier than the start
/// would cover no billing period at all.
```
and the shared refusal it is applied through — `src/state.rs:3993-3996`:
```
/// `Err` carries the `(reason, member, message)` of the modelled `ValidationException`
/// the calling operation refuses the request with, so every operation taking that shape
/// answers an empty range the same way.
pub fn check_billing_period_range_is_ordered(
```

The model's own naming of the tier pair (`smithy_query(query="shape",shape="CustomTier")`):
```
"BeginRangeInclusive": {"required": true, "range": {"min": 0}, "doc": "<p> The inclusive start of the usage range that this tier applies to. </p>"},
"EndRangeExclusive": {"range": {"min": 0}, "doc": "<p> The exclusive end of the usage range that this tier applies to. If you don't specify a value, this tier applies to all usage that is greater than or equal to <code>BeginRangeInclusive</code>. </p>"}
```

The handler that stores it unchecked — `src/ops/create_pricing_rule.rs:286-292`:
```
            input
                .tiering
                .as_ref()
                .map(|tiering| crate::state::TieringRecord {
                    free_tier_activated: tiering.free_tier.as_ref().map(|free| free.activated),
                    custom_tiers: tiering.custom_tiers.clone(),
                }),
```
and the same on the update path — `src/ops/update_pricing_rule.rs:300-302`:
```
        if let Some(custom_tiers) = tiering.custom_tiers.clone() {
            merged.custom_tiers = Some(custom_tiers);
        }
```
Stored verbatim — `src/state.rs:1148-1149`:
```
    /// `Tiering.CustomTiers`, in the order the request listed them.
    pub custom_tiers: Option<Vec<crate::ops::types::CustomTier>>,
```

The refusal the crate already words for an illegal tiering input — `src/state.rs:616-622`:
```
        if tiering.is_none() {
            return Err((
                "ILLEGAL_TIERING_INPUT",
                "Tiering",
                format!("a {TIERING_PRICING_RULE_TYPE} pricing rule needs a tiering configuration"),
            ));
        }
```
(`ILLEGAL_TIERING_INPUT` is a `ValidationExceptionReason` value — `smithy_query(query="shape",shape="ValidationExceptionReason")`.)

Test coverage: every `CustomTiers` case in the suite is well ordered — `tests/all/pricing_rule_lifecycle.rs:364-366` (`{"BeginRangeInclusive": 0.0, "EndRangeExclusive": 100.0, ...}`, `{"BeginRangeInclusive": 100.0, ...}`), `tests/all/pricing_rule_update.rs:341-352`, `tests/all/pricing_rule_rate_exclusivity.rs:404-409`, `tests/all/pricing_rule_core_loop.rs:258-263`. `tests/all/pricing_rule_lifecycle.rs:264-278` only asserts the tiers round-trip as sent, so no test pins acceptance of an inverted or overlapping tier.

## Steps to Reproduce

1. Designate the emulated account as the payer so the control plane is reachable (set `ComAmazonawsBillingconductorAwsbillingconductorState::payer_account_id`).
2. POST /create-pricing-rule with {"Name":"Inverted","Scope":"GLOBAL","Type":"TIERING","Tiering":{"CustomTiers":[{"BeginRangeInclusive":100.0,"EndRangeExclusive":50.0,"RateValue":1.0}]}}.
3. Observe HTTP 200 and an `Arn`, although the tier's exclusive end is below its inclusive start so the tier covers no usage.
4. POST /list-pricing-rules and observe the rule reported with that same tier persisted.
5. Compare: POST /create-custom-line-item with a `BillingPeriodRange` whose `ExclusiveEndBillingPeriod` equals its `InclusiveStartBillingPeriod` is refused 400 `ValidationException` with reason `ILLEGAL_BILLING_PERIOD_RANGE` (`state::check_billing_period_range_is_ordered`).
6. Also: POST /create-pricing-rule with CustomTiers [{Begin 0, End 100, Rate 1},{Begin 0, End 100, Rate 9}] — two tiers over the identical usage range — is accepted and stored.
