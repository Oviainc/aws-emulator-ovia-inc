// SPDX-License-Identifier: CC0-1.0

//! What a recommendation's money members report, against the cost calculation
//! the SAME answer carries for the two resource configurations, driven
//! end-to-end over the wire (`awsJson1_0`) through the same multi-service router
//! the emulator boots.
//!
//! The rule under test: the estimated monthly saving of a recommendation is what
//! the resource costs today less what it would cost in the recommended
//! configuration, so a recommendation whose action is BUYING a commitment saves
//! what that commitment takes off the usage it covers. The caller's
//! `savingsEstimationMode` preference decides only whether the discounts a
//! resource already carries are netted out of those two costs — both of them, so
//! an existing discount cancels — and never whether the discount the recommended
//! action itself creates counts, because that discount is the benefit being
//! estimated rather than one the estimate is taken before or after.
//!
//! Each test is a table over every row of the catalog and every state of the
//! preference, so the rule is asserted across action types, resource types,
//! priced-with and priced-without an existing discount, and each reader that
//! reports the figures.

use serde_json::{json, Value};
use service_com_amazonaws_costoptimizationhub_costoptimizationhubservice::ops::types::shared_types::{
    ACTION_TYPE_VALUES, SAVINGS_ESTIMATION_MODE_VALUES,
};
use service_com_amazonaws_costoptimizationhub_costoptimizationhubservice::state::AFTER_DISCOUNTS;
use smithy_server_runtime::MultiServiceRouter;

use crate::cost_optimization_hub::{enrolled_router_with_discounts_visible, ok};

/// Two money figures are the same figure: every reader rounds to cents, so
/// anything inside half a cent is the same value.
fn same_money(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() < 0.005
}

/// How many days of cost one monthly figure covers, for prorating a monthly
/// saving over a lookback period measured in days.
const DAYS_PER_MONTH: f64 = 30.0;

/// The `ActionType` values whose action is buying a COMMITMENT rather than
/// changing a resource: the two `Purchase…` values of the model's `ActionType`
/// enum (`ACTION_TYPE_VALUES`, read from the service so this list cannot drift
/// from the model's).
fn commitment_purchase_actions() -> Vec<&'static str> {
    ACTION_TYPE_VALUES
        .iter()
        .copied()
        .filter(|action| action.starts_with("Purchase"))
        .collect()
}

/// Every state of the `savingsEstimationMode` preference, in the order an
/// account moves through them: no preference at all first — read under the
/// default the service applies — then each value of the model's
/// `SavingsEstimationMode` enum.
fn modes() -> Vec<Option<&'static str>> {
    std::iter::once(None)
        .chain(SAVINGS_ESTIMATION_MODE_VALUES.iter().copied().map(Some))
        .collect()
}

/// Put the account into `mode`, leaving it under the service's default where the
/// case states no preference.
async fn store_mode(router: &MultiServiceRouter, mode: Option<&str>) {
    if let Some(mode) = mode {
        ok(
            router,
            "UpdatePreferences",
            &json!({"savingsEstimationMode": mode}),
        )
        .await;
    }
}

/// One money member of an answer.
fn money(payload: &Value, member: &str) -> f64 {
    payload[member]
        .as_f64()
        .unwrap_or_else(|| panic!("expected a number in {member}, got {payload}"))
}

/// The `costCalculation.pricing` of one half of a detail answer. The
/// `ResourceDetails` union carries exactly one member, and that member prices
/// the configuration it describes.
fn pricing_of(details: &Value) -> &Value {
    let resource = details
        .as_object()
        .and_then(|members| members.values().next())
        .unwrap_or_else(|| panic!("expected one resource in the details, got {details}"));
    &resource["costCalculation"]["pricing"]
}

/// What the caller pays per month for the configuration `details` describes, as
/// that half of the SAME answer prices it, under `mode`.
///
/// A commitment is priced by `SavingsPlansPricing`, whose
/// `estimatedMonthlyCommitment` is what adopting it costs; every other resource
/// is priced by `ResourcePricing`, whose before- and after-discount members are
/// the two views of its existing discounts the mode selects between.
fn reported_cost(details: &Value, mode: Option<&str>) -> f64 {
    let pricing = pricing_of(details);
    if let Some(commitment) = pricing["estimatedMonthlyCommitment"].as_f64() {
        return commitment;
    }
    let member = if mode == Some(AFTER_DISCOUNTS) {
        "estimatedCostAfterDiscounts"
    } else {
        "estimatedCostBeforeDiscounts"
    };
    money(pricing, member)
}

/// Every recommendation the caller may read, as whole list entries, under the
/// filter `request` carries.
async fn listed(router: &MultiServiceRouter, request: &Value) -> Vec<Value> {
    let payload = ok(router, "ListRecommendations", request).await;
    payload["items"].as_array().cloned().unwrap_or_default()
}

/// Every recommendation's money members are the difference between the two cost
/// calculations the same answer carries, under every state of the preference —
/// so no row reports a saving its own `ResourceDetails` contradict.
///
/// The table is every row of the catalog (spanning action types, resource types,
/// detail kinds and prices with and without an existing discount) crossed with
/// every state of the mode preference.
#[tokio::test]
async fn money_members_agree_with_the_cost_calculations_of_the_same_answer() {
    let router = enrolled_router_with_discounts_visible().await;

    for mode in modes() {
        store_mode(&router, mode).await;
        let items = listed(&router, &json!({"includeAllRecommendations": true})).await;
        assert!(
            items.len() > 1,
            "case {mode:?}: expected the catalog to offer several recommendations, got {items:?}"
        );

        for item in &items {
            let recommendation_id = item["recommendationId"].as_str().unwrap_or_default();
            let case = format!("{recommendation_id} under {mode:?}");
            let detail = ok(
                &router,
                "GetRecommendation",
                &json!({"recommendationId": recommendation_id}),
            )
            .await;

            let cost = money(&detail, "estimatedMonthlyCost");
            let savings = money(&detail, "estimatedMonthlySavings");
            let current = reported_cost(&detail["currentResourceDetails"], mode);
            let recommended = reported_cost(&detail["recommendedResourceDetails"], mode);

            assert!(
                same_money(cost, current),
                "case {case:?}: expected the monthly cost to be what the current resource's \
                 cost calculation prices it at ({current}), got {cost}"
            );
            assert!(
                same_money(savings, current - recommended),
                "case {case:?}: expected the saving to be the current cost {current} less the \
                 recommended configuration's {recommended} ({}), got {savings}",
                current - recommended
            );
            assert!(
                same_money(
                    money(&detail, "estimatedSavingsPercentage"),
                    savings / cost * 100.0
                ),
                "case {case:?}: expected {} percent of the {cost} monthly cost, got {}",
                savings / cost * 100.0,
                money(&detail, "estimatedSavingsPercentage")
            );

            let lookback = money(&detail, "costCalculationLookbackPeriodInDays");
            let over_lookback = money(&detail, "estimatedSavingsOverCostCalculationLookbackPeriod");
            assert!(
                same_money(over_lookback, savings * lookback / DAYS_PER_MONTH),
                "case {case:?}: expected {} over {lookback} days, got {over_lookback}",
                savings * lookback / DAYS_PER_MONTH
            );

            // The list reader reports the same figures for the same row, so the
            // agreement is not one reader's alone.
            for member in [
                "estimatedMonthlyCost",
                "estimatedMonthlySavings",
                "estimatedSavingsPercentage",
            ] {
                assert!(
                    same_money(money(item, member), money(&detail, member)),
                    "case {case:?}: expected the list and detail views to report the same \
                     {member}, got {} and {}",
                    money(item, member),
                    money(&detail, member)
                );
            }
        }
    }
}

/// A recommendation whose action is buying a commitment saves what the
/// commitment takes off the usage it covers, under every state of the
/// preference: the benefit is the commitment itself, so no mode reports it as
/// saving nothing.
///
/// The table is every commitment-purchase row the catalog offers crossed with
/// every state of the mode preference.
#[tokio::test]
async fn a_commitment_purchase_saves_what_its_commitment_takes_off_in_every_mode() {
    let router = enrolled_router_with_discounts_visible().await;
    let purchase_actions = commitment_purchase_actions();

    for mode in modes() {
        store_mode(&router, mode).await;
        let items = listed(&router, &json!({"includeAllRecommendations": true})).await;
        let purchases: Vec<&Value> = items
            .iter()
            .filter(|item| {
                purchase_actions.contains(&item["actionType"].as_str().unwrap_or_default())
            })
            .collect();
        assert!(
            !purchases.is_empty(),
            "case {mode:?}: expected the catalog to offer a commitment-purchase recommendation \
             among {items:?}"
        );

        for item in purchases {
            let recommendation_id = item["recommendationId"].as_str().unwrap_or_default();
            let case = format!("{recommendation_id} under {mode:?}");
            let detail = ok(
                &router,
                "GetRecommendation",
                &json!({"recommendationId": recommendation_id}),
            )
            .await;

            let pricing = pricing_of(&detail["recommendedResourceDetails"]);
            let eligible = money(pricing, "monthlySavingsPlansEligibleCost");
            let commitment = money(pricing, "estimatedMonthlyCommitment");
            assert!(
                commitment < eligible,
                "case {case:?}: expected a recommended commitment below the {eligible} the \
                 eligible usage costs, got {commitment}"
            );

            let savings = money(&detail, "estimatedMonthlySavings");
            assert!(
                savings > 0.0,
                "case {case:?}: expected the commitment's benefit to be reported as a saving, \
                 got {savings}"
            );
            assert!(
                same_money(savings, eligible - commitment),
                "case {case:?}: expected the saving to be the {eligible} eligible usage less \
                 the {commitment} commitment ({}), got {savings}",
                eligible - commitment
            );
            assert!(
                same_money(
                    money(pricing, "savingsPercentage"),
                    (eligible - commitment) / eligible * 100.0
                ),
                "case {case:?}: expected the cost calculation to report {} percent, got {}",
                (eligible - commitment) / eligible * 100.0,
                money(pricing, "savingsPercentage")
            );
            assert!(
                money(&detail, "estimatedSavingsPercentage") > 0.0,
                "case {case:?}: expected a positive savings percentage, got {}",
                money(&detail, "estimatedSavingsPercentage")
            );
        }

        // The same saving reaches the summary reader: each commitment-purchase
        // action's group totals what that action's de-duped rows save, and the
        // whole view's total is at least that much.
        let whole_view = ok(&router, "ListRecommendationSummaries", &json!({"groupBy": "ActionType"})).await;
        for action in &purchase_actions {
            let filter = json!({"actionTypes": [action]});
            let deduped = listed(
                &router,
                &json!({"includeAllRecommendations": false, "filter": filter}),
            )
            .await;
            if deduped.is_empty() {
                continue;
            }
            let expected: f64 = deduped
                .iter()
                .map(|item| money(item, "estimatedMonthlySavings"))
                .sum();
            let case = format!("{action} under {mode:?}");
            assert!(
                expected > 0.0,
                "case {case:?}: expected the action's rows to save something, got {expected}"
            );

            let summary = ok(
                &router,
                "ListRecommendationSummaries",
                &json!({"groupBy": "ActionType", "filter": {"actionTypes": [action]}}),
            )
            .await;
            let group = money(&summary["items"][0], "estimatedMonthlySavings");
            assert!(
                same_money(group, expected),
                "case {case:?}: expected the group to total {expected}, got {group}"
            );
            assert!(
                same_money(money(&summary, "estimatedTotalDedupedSavings"), expected),
                "case {case:?}: expected the filtered total to be {expected}, got {}",
                money(&summary, "estimatedTotalDedupedSavings")
            );
            assert!(
                money(&whole_view, "estimatedTotalDedupedSavings") >= expected - 0.005,
                "case {case:?}: expected the whole view's total to include the {expected} this \
                 action saves, got {}",
                money(&whole_view, "estimatedTotalDedupedSavings")
            );
        }
    }
}
