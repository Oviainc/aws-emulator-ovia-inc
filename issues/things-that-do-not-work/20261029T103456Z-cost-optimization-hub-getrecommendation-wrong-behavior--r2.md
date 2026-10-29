Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9fa26429e9fb75f482c46ca7a1dcc182c128efe3

# The PurchaseSavingsPlans recommendation reports zero savings under the default savingsEstimationMode, contradicting its own recommendedResourceDetails

_Filed: 2026-10-29T10:34:56Z_
_Operation: GetRecommendation | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9fa26429e9fb75f482c46ca7a1dcc182c128efe3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The catalog row for the commitment-purchase recommendation (`actionType: PurchaseSavingsPlans`, `recommendationId` ...0005) stores the recommendation's whole value on the RECOMMENDED side as a savings-plans discount: `current_pricing: Pricing::list(1_314.0)` (no discount) and `recommended_pricing: Pricing::discounted(1_314.0, 438.0)`.

`monthly_savings(mode)` is `current.cost_for_mode(mode) - recommended.cost_for_mode(mode)`, and `cost_for_mode` returns the UNDISCOUNTED cost for every mode other than `AfterDiscounts`. The effective mode of an account that expressed no preference is `BeforeDiscounts` (`DEFAULT_SAVINGS_ESTIMATION_MODE`), which is what both recommendation readers resolve through `view_over`/`effective_preferences`. So for this row the default caller is served `estimatedMonthlySavings = 0.0`, `estimatedSavingsPercentage = 0.0` and `estimatedSavingsOverCostCalculationLookbackPeriod = 0.0` — for a recommendation whose only purpose is the commitment discount.

The same `GetRecommendation` response simultaneously reports, from the SAME stored price, `recommendedResourceDetails.computeSavingsPlans.costCalculation.pricing` = { `monthlySavingsPlansEligibleCost`: 1314.00, `estimatedMonthlyCommitment`: 876.00, `savingsPercentage`: 33.33, `estimatedOnDemandCost`: 1314.00 } — i.e. the details say adopting the recommendation turns a 1314.00/month cost into an 876.00/month commitment (a 438.00/month saving, 33.33%), while the top-level money members of the very same response say the recommendation saves nothing. The crate's own catalog doc asserts these cannot disagree. The same zero propagates to `ListRecommendations` (the row is listed with `estimatedMonthlySavings: 0.0`) and to `ListRecommendationSummaries`, whose `PurchaseSavingsPlans`/`ComputeSavingsPlans` group totals 0.00 and contributes nothing to `estimatedTotalDedupedSavings`.

Note that `AfterDiscounts` — the only mode under which this row's figures become self-consistent — is additionally unreachable over the wire today (see the open issue "UpdatePreferences refuses memberAccountDiscountVisibility and AfterDiscounts to every possible client"), so every client of this emulator sees the zero.

## Evidence

The crate's own claim that these figures agree — src/state.rs:1226-1230:
```
/// The emulator observes no real usage, so the catalog is a fixed set of
/// resources rather than something derived from traffic; every figure a
/// recommendation reports is nevertheless computed from the prices stored here
/// (so cost, saving and percentage agree with each other and with the
/// `ResourceDetails` cost calculation), and both recommendation readers read
/// these rows.
```

The row, src/state.rs:1395-1400:
```
            action_type: "PurchaseSavingsPlans".to_string(),
...
            current_pricing: Pricing::list(1_314.0),
            recommended_pricing: Pricing::discounted(1_314.0, 438.0),
```

The computation, src/state.rs:326-331 and src/state.rs:408-412:
```
    pub fn cost_for_mode(&self, mode: Option<&str>) -> f64 {
        if mode == Some(AFTER_DISCOUNTS) {
            self.cost_after_discounts()
        } else {
            round_cents(self.cost_before_discounts)
```
```
    pub fn monthly_savings(&self, mode: Option<&str>) -> f64 {
        round_cents(
            self.current_pricing.cost_for_mode(mode) - self.recommended_pricing.cost_for_mode(mode),
        )
    }
```
=> 1314.00 - 1314.00 = 0.00 under the default mode, src/state.rs:165:
```
pub const DEFAULT_SAVINGS_ESTIMATION_MODE: &str = BEFORE_DISCOUNTS;
```

The other half of the same response, src/state.rs:804-816:
```
            cost_calculation: Some(SavingsPlansCostCalculation {
                pricing: Some(SavingsPlansPricing {
                    monthly_savings_plans_eligible_cost: Some(round_cents(
                        pricing.cost_before_discounts,
                    )),
                    estimated_monthly_commitment: Some(pricing.cost_after_discounts()),
                    savings_percentage: Some(percentage_of(
                        pricing.discounts_total(),
                        pricing.cost_before_discounts,
                    )),
                    estimated_on_demand_cost: Some(round_cents(pricing.cost_before_discounts)),
```

Both halves are emitted by one handler, src/ops/get_recommendation.rs:252-268:
```
            estimated_savings_percentage: Some(row.savings_percentage(mode)),
...
            estimated_monthly_savings: Some(row.monthly_savings(mode)),
            estimated_monthly_cost: Some(row.monthly_cost(mode)),
...
            recommended_resource_details: Some(row.recommended_resource_details()),
```

Model (`smithy_query(query="shape", shape="GetRecommendationResponse")`): `estimatedMonthlySavings` — "The estimated monthly savings amount for the recommendation."; `SavingsPlansPricing$savingsPercentage` — "Estimated savings as a percentage of your overall costs after buying the Savings Plans."

Test coverage: none. The mode/money test only exercises the EC2 rightsize row, whose BOTH sides carry a discount (tests/all/cost_optimization_hub.rs:1200-1205: `(None, 280.32, 140.16), (Some("BeforeDiscounts"), 280.32, 140.16), (Some("AfterDiscounts"), 252.29, 126.15)`), and no test in the suite reads `costCalculation`/`estimatedMonthlyCommitment`/`savingsPercentage` off a recommendation's `ResourceDetails` at all (grep for `costCalculation` in tests/ matches only `costCalculationLookbackPeriodInDays`).

## Steps to Reproduce

1. Boot the enrolled router the suite uses (`enrolled_router()` in tests/all/cost_optimization_hub.rs) — an account with no `UpdatePreferences` call, so its effective `savingsEstimationMode` is the default `BeforeDiscounts`.
2. Call `GetRecommendation` with `{"recommendationId": "0ec38a3a-1a0a-4a4f-9d2a-1f1e9a7c0005"}` (the seeded `PurchaseSavingsPlans` row, src/state.rs:1372-1407).
3. Observed in the one response: `estimatedMonthlyCost: 1314.0`, `estimatedMonthlySavings: 0.0`, `estimatedSavingsPercentage: 0.0`, `estimatedSavingsOverCostCalculationLookbackPeriod: 0.0`, while `recommendedResourceDetails.computeSavingsPlans.costCalculation.pricing` reports `estimatedOnDemandCost: 1314.0`, `estimatedMonthlyCommitment: 876.0` and `savingsPercentage: 33.33`.
4. Call `ListRecommendations` with `{"includeAllRecommendations": true, "filter": {"recommendationIds": ["0ec38a3a-1a0a-4a4f-9d2a-1f1e9a7c0005"]}}`: the item carries `estimatedMonthlySavings: 0.0` for a `PurchaseSavingsPlans` action.
5. Call `ListRecommendationSummaries` with `{"groupBy": "ActionType"}`: the `PurchaseSavingsPlans` group reports `estimatedMonthlySavings: 0.0` and adds nothing to `estimatedTotalDedupedSavings`.
