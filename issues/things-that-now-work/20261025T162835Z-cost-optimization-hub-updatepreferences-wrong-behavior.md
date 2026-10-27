Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: b3c7a1d3b5e40780898598751dac20435d474802

# Turning memberAccountDiscountVisibility to None leaves AfterDiscounts in effect, the state the same handler refuses to create in one request

_Filed: 2026-10-25T16:28:35Z_
_Operation: UpdatePreferences | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b3c7a1d3b5e40780898598751dac20435d474802`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdatePreferences` enforces its discount-visibility rule only when the REQUEST carries `savingsEstimationMode`. A request that carries only `memberAccountDiscountVisibility: "None"` is accepted without looking at the mode already stored, so the pair (`savingsEstimationMode = AfterDiscounts`, `memberAccountDiscountVisibility = None`) becomes reachable by sending the two members in two requests — while the very same handler refuses that pair when the two members arrive in ONE request. The handler's own doc states the rule as a property of the STATE ("the mode is unavailable until the preference that shows them is set"), and the crate's test states the one-request acceptance is justified "because the state the request leaves satisfies the rule" — so a request whose resulting state does NOT satisfy the rule must be refused too, whichever member it carries.

The consequence is not cosmetic: once the store holds that pair, every reader resolves the effective mode through `effective_preferences(...).savings_estimation_mode` and `Pricing::cost_for_mode` then SUBTRACTS the discounts, so `GetRecommendation`, `ListRecommendations` and `ListRecommendationSummaries` report discount-adjusted money (and `ListRecommendationSummaries` reports a numeric `metrics.savingsPercentage` instead of the "not supported ... before discounts" description) for an organization whose member-account discounts are explicitly not visible. `GetPreferences` likewise reports the forbidden pair back.

## Evidence

The gate fires only on the request's own member — services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/src/ops/update_preferences.rs:204-212:
```
            let discounts_visible = match input.member_account_discount_visibility.as_deref() {
                Some(requested) => requested == crate::state::DISCOUNT_VISIBILITY_ALL,
                None => store.member_account_discounts_visible(),
            };
            if input.savings_estimation_mode.as_deref() == Some(crate::state::AFTER_DISCOUNTS)
                && !discounts_visible
            {
                return Err(after_discounts_needs_visible_discounts());
            }
```
The rule the code under it is meant to keep — update_preferences.rs:255-260:
```
/// Refuse the discount-adjusted savings estimation mode while the organization
/// has not made its member accounts' discounts visible: the mode's figures are
/// net of those discounts, so the mode is unavailable until the preference that
/// shows them is set
```
The crate's own test says the rule is about the state the request LEAVES, and pins the one-request refusal of exactly this pair — tests/all/preference_gates.rs:367-396:
```
    // And in one request: enabling the visibility and asking for the discounted
    // mode together is accepted, because the state the request leaves satisfies
    // the rule.
...
    // The other way round: asking for the discounted mode while turning the
    // visibility off in the same request is refused.
    refused(
        &fresh,
        "both members at once, visibility off",
        &json!({
            "memberAccountDiscountVisibility": DISCOUNT_VISIBILITY_NONE,
            "savingsEstimationMode": AFTER_DISCOUNTS,
        }),
```
The same test's "back to None" row (preference_gates.rs:317-340) performs the accepted `memberAccountDiscountVisibility: None` write while the stored mode is still `AfterDiscounts`, but immediately rewrites the mode to `BeforeDiscounts` before reading, so the resulting state is never asserted — nothing pins it.

What the stale mode then does to the figures — src/state.rs:301-303:
```
    pub fn cost_for_mode(&self, mode: Option<&str>) -> f64 {
        if mode == Some(AFTER_DISCOUNTS) {
            self.cost_after_discounts()
```
read through the stored preference in src/ops/list_recommendations.rs:219-221:
```
            let mode = store
                .effective_preferences(crate::state::caller_account_id())
                .savings_estimation_mode;
```

## Steps to Reproduce

1. Boot the service with the caller recorded as the organization's management account (the `management_account_router()` helper in tests/all/cost_optimization_hub.rs does this).
2. POST X-Amz-Target: CostOptimizationHubService.UpdatePreferences with {"memberAccountDiscountVisibility":"All"} — 200.
3. POST UpdatePreferences {"savingsEstimationMode":"AfterDiscounts"} — 200 (allowed, discounts are visible).
4. POST UpdatePreferences {"memberAccountDiscountVisibility":"None"} — accepted with 200, although the state it leaves is the pair the handler refuses when both members arrive together.
5. POST GetPreferences {} — observe savingsEstimationMode "AfterDiscounts" together with memberAccountDiscountVisibility "None".
6. Enroll (UpdateEnrollmentStatus {"status":"Active"}), wait for the generation delay, then POST ListRecommendations {"includeAllRecommendations":true} — estimatedMonthlyCost/estimatedMonthlySavings are computed net of discounts, and ListRecommendationSummaries {"groupBy":"ActionType","metrics":["SavingsPercentage"]} returns a numeric savingsPercentage, although member-account discount visibility is None.
