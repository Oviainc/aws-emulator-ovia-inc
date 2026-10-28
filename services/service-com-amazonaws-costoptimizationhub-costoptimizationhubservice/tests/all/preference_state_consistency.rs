// SPDX-License-Identifier: CC0-1.0

//! `UpdatePreferences`' discount rule as a rule about the STATE a request
//! leaves, driven end-to-end over the wire (`awsJson1_0`) through the same
//! multi-service router the emulator boots.
//!
//! The rule under test: the discount-adjusted savings estimation mode
//! (`savingsEstimationMode` = `AfterDiscounts`) reports figures net of the
//! organization's member-account discounts, so the service accepts an
//! `UpdatePreferences` request only where the record the merge LEAVES pairs that
//! mode with `memberAccountDiscountVisibility` = `All` — whichever of the two
//! members the request carries, and whatever it carries them beside. The pair
//! the service refuses in one request is therefore unreachable in any number of
//! requests, and a refused request stores nothing.
//!
//! The rows span what the rule generalises over: which of the two members the
//! request carries (either, both, neither), the value each carries, the record
//! the request is applied to (including the account that has set no preference
//! at all, served under the mode the service applies in its place), and the
//! other members the request carries beside them. The second test enumerates
//! the whole reachable space — every sequence of three requests over the cross
//! product of the two members' modelled enums and their absence — so no order
//! of calls reaches the forbidden pair, and the third follows the consequence
//! out to the figures the recommendation readers report.

use serde_json::{json, Value};
use service_com_amazonaws_costoptimizationhub_costoptimizationhubservice as cost_optimization_hub;
use smithy_server_runtime::MultiServiceRouter;

/// Both members' modelled enums, read from the generated catalogue so the
/// tables cannot drift from the model (`smithy_query(query="shape",
/// shape="SavingsEstimationMode")` and `…shape="MemberAccountDiscountVisibility"`).
use cost_optimization_hub::ops::types::shared_types::{
    MEMBER_ACCOUNT_DISCOUNT_VISIBILITY_VALUES, SAVINGS_ESTIMATION_MODE_VALUES,
};

/// The two values the rule is about, and the mode the service applies for an
/// account that stated none — read from the service rather than copied here.
use cost_optimization_hub::state::{
    AFTER_DISCOUNTS, BEFORE_DISCOUNTS, DEFAULT_SAVINGS_ESTIMATION_MODE, DISCOUNT_VISIBILITY_NONE,
};

use crate::cost_optimization_hub::{
    assert_stated_rule_refusal, call, enrolled_router_with_discounts_visible,
    management_account_router, ok, refused_member, DISCOUNT_VISIBILITY_ALL, MODE_MEMBER,
    VISIBILITY_MEMBER,
};

/// The pair the rule is about, as `GetPreferences` reports it: the effective
/// savings estimation mode, and the organization's member-account discount
/// visibility where it has been set.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pair {
    mode: String,
    visibility: Option<String>,
}

/// Whether a pair satisfies the rule: the discount-adjusted mode is in effect
/// only while the organization's member-account discounts are visible.
fn satisfies_the_rule(pair: &Pair) -> bool {
    pair.mode != AFTER_DISCOUNTS || pair.visibility.as_deref() == Some(DISCOUNT_VISIBILITY_ALL)
}

/// The pair a read of the account's preferences reports.
async fn stored_pair(router: &MultiServiceRouter) -> Pair {
    let read = ok(router, "GetPreferences", &json!({})).await;
    Pair {
        mode: read[MODE_MEMBER]
            .as_str()
            .unwrap_or(DEFAULT_SAVINGS_ESTIMATION_MODE)
            .to_string(),
        visibility: read[VISIBILITY_MEMBER].as_str().map(str::to_string),
    }
}

/// The pair a request would leave: the value the request states for a member
/// replaces the stored one, and a member the request omits keeps it.
fn resulting_pair(stored: &Pair, request: &Value) -> Pair {
    Pair {
        mode: request[MODE_MEMBER]
            .as_str()
            .unwrap_or(&stored.mode)
            .to_string(),
        visibility: request[VISIBILITY_MEMBER]
            .as_str()
            .map(str::to_string)
            .or_else(|| stored.visibility.clone()),
    }
}

/// Send one `UpdatePreferences` and assert the service judged it by the rule:
/// accepted and merged where the record it leaves satisfies the rule, and
/// otherwise the operation's modelled `ValidationException` (400) stating the
/// rule, naming the request's own member, with nothing stored.
async fn judged_by_the_rule(router: &MultiServiceRouter, case: &str, request: &Value) {
    let before = stored_pair(router).await;
    let resulting = resulting_pair(&before, request);
    let allowed = satisfies_the_rule(&resulting);

    let (status, payload) = call(router, "UpdatePreferences", request).await;
    let expected_status = if allowed { 200 } else { 400 };
    assert_eq!(
        status, expected_status,
        "case {case:?}: {request} applied to {before:?} leaves {resulting:?}, so expected \
         HTTP {expected_status}, got {status} with {payload}"
    );

    if allowed {
        let after = stored_pair(router).await;
        assert_eq!(
            after, resulting,
            "case {case:?}: expected the merge to leave {resulting:?}, got {after:?}"
        );
        return;
    }

    assert_stated_rule_refusal(&payload, case, refused_member(request));
    let after = stored_pair(router).await;
    assert_eq!(
        after, before,
        "case {case:?}: a refused write must leave the record at {before:?}, got {after:?}"
    );
}

/// The rule is judged on the record the request leaves, not on the members the
/// request happens to carry: a request that carries only the discount
/// visibility is refused exactly when hiding the discounts would leave the
/// stored discount-adjusted mode in effect, and the same request is accepted
/// once the mode it would strand is changed — in an earlier request or in the
/// same one.
#[tokio::test]
async fn the_discount_rule_is_judged_on_the_record_the_request_leaves() {
    // The record every row below is applied to, as the requests that establish
    // it: the discounts visible and the discount-adjusted mode stored, which is
    // the state the rule can be left unsatisfied from.
    let discounted: Vec<Value> = vec![
        json!({VISIBILITY_MEMBER: DISCOUNT_VISIBILITY_ALL}),
        json!({MODE_MEMBER: AFTER_DISCOUNTS}),
    ];

    // (case, the accepted requests that establish the record, the request under
    // test). Each row's verdict is derived from the rule by
    // `judged_by_the_rule`, so the table carries inputs only.
    let cases: Vec<(&str, Vec<Value>, Value)> = vec![
        (
            "hide the discounts under the discounted mode",
            discounted.clone(),
            json!({VISIBILITY_MEMBER: DISCOUNT_VISIBILITY_NONE}),
        ),
        (
            "restate the visibility the discounted mode needs",
            discounted.clone(),
            json!({VISIBILITY_MEMBER: DISCOUNT_VISIBILITY_ALL}),
        ),
        (
            "hide the discounts beside the commitment",
            discounted.clone(),
            json!({
                VISIBILITY_MEMBER: DISCOUNT_VISIBILITY_NONE,
                "preferredCommitment": {"term": "OneYear", "paymentOption": "NoUpfront"},
            }),
        ),
        (
            "hide the discounts and leave the discounted mode together",
            discounted.clone(),
            json!({
                VISIBILITY_MEMBER: DISCOUNT_VISIBILITY_NONE,
                MODE_MEMBER: BEFORE_DISCOUNTS,
            }),
        ),
        (
            "hide the discounts and keep the discounted mode together",
            discounted.clone(),
            json!({
                VISIBILITY_MEMBER: DISCOUNT_VISIBILITY_NONE,
                MODE_MEMBER: AFTER_DISCOUNTS,
            }),
        ),
        (
            "a member the rule is not about, under the discounted mode",
            discounted.clone(),
            json!({"preferredCommitment": {"term": "OneYear"}}),
        ),
        (
            "the empty request under the discounted mode",
            discounted.clone(),
            json!({}),
        ),
        (
            "hide the discounts after leaving the discounted mode",
            vec![
                json!({VISIBILITY_MEMBER: DISCOUNT_VISIBILITY_ALL}),
                json!({MODE_MEMBER: AFTER_DISCOUNTS}),
                json!({MODE_MEMBER: BEFORE_DISCOUNTS}),
            ],
            json!({VISIBILITY_MEMBER: DISCOUNT_VISIBILITY_NONE}),
        ),
        (
            "hide the discounts on an account that stated no preference",
            vec![],
            json!({VISIBILITY_MEMBER: DISCOUNT_VISIBILITY_NONE}),
        ),
        (
            "ask for the discounted mode on an account that stated no preference",
            vec![],
            json!({MODE_MEMBER: AFTER_DISCOUNTS}),
        ),
        (
            "ask for the discounted mode with the discounts hidden",
            vec![json!({VISIBILITY_MEMBER: DISCOUNT_VISIBILITY_NONE})],
            json!({MODE_MEMBER: AFTER_DISCOUNTS}),
        ),
        (
            "ask for the discounted mode again after the discounts were hidden",
            vec![
                json!({VISIBILITY_MEMBER: DISCOUNT_VISIBILITY_ALL}),
                json!({MODE_MEMBER: AFTER_DISCOUNTS}),
                json!({MODE_MEMBER: BEFORE_DISCOUNTS}),
                json!({VISIBILITY_MEMBER: DISCOUNT_VISIBILITY_NONE}),
            ],
            json!({MODE_MEMBER: AFTER_DISCOUNTS}),
        ),
    ];

    for (case, setup, request) in &cases {
        // A fresh organization per row, with the caller recorded as its
        // management account, so the organization-wide member is its to set.
        let router = management_account_router();
        for (index, step) in setup.iter().enumerate() {
            let established = ok(&router, "UpdatePreferences", step).await;
            let pair = stored_pair(&router).await;
            assert!(
                satisfies_the_rule(&pair),
                "case {case:?} setup {index}: {step} left {pair:?}, which the rule forbids, \
                 answered {established}"
            );
        }

        judged_by_the_rule(&router, case, request).await;
    }
}

/// No sequence of requests reaches the forbidden pair.
///
/// The space is finite, so it is enumerated rather than sampled: every sequence
/// of three requests drawn from the cross product of the two members' modelled
/// enums and their absence, each request judged by the rule and each resulting
/// record read back, so a refusal is shown to store nothing and an acceptance to
/// store exactly the merge.
#[tokio::test]
async fn no_sequence_of_requests_reaches_the_forbidden_pair() {
    // One request per combination of "state this member" × "omit it", over both
    // members' modelled enums.
    let mut requests: Vec<Value> = Vec::new();
    for mode in [None].into_iter().chain(SAVINGS_ESTIMATION_MODE_VALUES.iter().map(Some)) {
        for visibility in [None]
            .into_iter()
            .chain(MEMBER_ACCOUNT_DISCOUNT_VISIBILITY_VALUES.iter().map(Some))
        {
            let mut request = json!({});
            let object = request.as_object_mut().expect("the request is an object");
            if let Some(mode) = mode {
                object.insert(MODE_MEMBER.to_string(), Value::String((*mode).to_string()));
            }
            if let Some(visibility) = visibility {
                object.insert(
                    VISIBILITY_MEMBER.to_string(),
                    Value::String((*visibility).to_string()),
                );
            }
            requests.push(request);
        }
    }

    for first in &requests {
        for second in &requests {
            for third in &requests {
                let router = management_account_router();
                for (step, request) in [first, second, third].into_iter().enumerate() {
                    let case = format!("{first} → {second} → {third}, step {step}");
                    judged_by_the_rule(&router, &case, request).await;
                    let pair = stored_pair(&router).await;
                    assert!(
                        satisfies_the_rule(&pair),
                        "case {case:?}: the store holds {pair:?}, which the rule forbids"
                    );
                }
            }
        }
    }
}

/// The consequence the rule protects: the recommendation readers report
/// discount-adjusted money, and a numeric savings-percentage metric, only while
/// the organization's member-account discounts are visible — because the pair
/// that would have them report net figures with the discounts hidden cannot be
/// stored.
#[tokio::test]
async fn discount_adjusted_figures_are_reported_only_while_the_discounts_are_visible() {
    let router = enrolled_router_with_discounts_visible().await;

    // (case, the request) — the path the two-request route took: into the
    // discounted mode, then hiding the discounts under it, then the route back
    // out, and the discounted mode asked for again with the discounts hidden.
    let steps: Vec<(&str, Value)> = vec![
        (
            "into the discounted mode",
            json!({MODE_MEMBER: AFTER_DISCOUNTS}),
        ),
        (
            "hide the discounts under the discounted mode",
            json!({VISIBILITY_MEMBER: DISCOUNT_VISIBILITY_NONE}),
        ),
        (
            "leave the discounted mode",
            json!({MODE_MEMBER: BEFORE_DISCOUNTS}),
        ),
        (
            "hide the discounts",
            json!({VISIBILITY_MEMBER: DISCOUNT_VISIBILITY_NONE}),
        ),
        (
            "ask for the discounted mode with the discounts hidden",
            json!({MODE_MEMBER: AFTER_DISCOUNTS}),
        ),
    ];

    // The monthly cost the catalog reports under each mode, in whole cents, so
    // the figures are compared to each other rather than to a number copied
    // into this file.
    let mut cost_by_mode: Vec<(String, i64)> = Vec::new();

    for (case, request) in &steps {
        judged_by_the_rule(&router, case, request).await;
        let pair = stored_pair(&router).await;
        assert!(
            satisfies_the_rule(&pair),
            "case {case:?}: the store holds {pair:?}, which the rule forbids"
        );

        let summary = ok(
            &router,
            "ListRecommendationSummaries",
            &json!({"groupBy": "Region", "metrics": ["SavingsPercentage"]}),
        )
        .await;
        let metric = summary["metrics"]["savingsPercentage"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let is_figure = metric.parse::<f64>().is_ok();
        assert_eq!(
            is_figure,
            pair.mode == AFTER_DISCOUNTS,
            "case {case:?}: with {pair:?} in effect, expected a numeric metric only under \
             the discounted mode, got {metric:?} in {summary}"
        );
        if is_figure {
            assert_eq!(
                pair.visibility.as_deref(),
                Some(DISCOUNT_VISIBILITY_ALL),
                "case {case:?}: a discount-adjusted figure requires the organization's \
                 discounts visible, got {pair:?}"
            );
        }

        let listed = ok(
            &router,
            "ListRecommendations",
            &json!({"includeAllRecommendations": true}),
        )
        .await;
        let cost: i64 = listed["items"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|item| {
                (item["estimatedMonthlyCost"].as_f64().unwrap_or_default() * 100.0).round() as i64
            })
            .sum();
        match cost_by_mode.iter().find(|(mode, _)| *mode == pair.mode) {
            Some((_, recorded)) => assert_eq!(
                cost, *recorded,
                "case {case:?}: the money reported under {:?} must not change, expected \
                 {recorded} cents, got {cost}",
                pair.mode
            ),
            None => cost_by_mode.push((pair.mode.clone(), cost)),
        }
    }

    let discounted = cost_by_mode
        .iter()
        .find(|(mode, _)| mode.as_str() == AFTER_DISCOUNTS)
        .map(|(_, cost)| *cost)
        .expect("the discounted mode was reached while the discounts were visible");
    let undiscounted = cost_by_mode
        .iter()
        .find(|(mode, _)| mode.as_str() == BEFORE_DISCOUNTS)
        .map(|(_, cost)| *cost)
        .expect("the undiscounted mode was reached");
    assert!(
        discounted < undiscounted,
        "case the two modes: expected the discount-adjusted spend below the undiscounted \
         one, got {discounted} and {undiscounted} cents"
    );
}
