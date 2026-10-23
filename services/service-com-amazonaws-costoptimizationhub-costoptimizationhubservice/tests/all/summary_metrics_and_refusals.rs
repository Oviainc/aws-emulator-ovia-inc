// SPDX-License-Identifier: CC0-1.0

//! Two rules about `ListRecommendationSummaries`' answers, driven end-to-end
//! over the wire (`awsJson1_0`) through the same multi-service router the
//! emulator boots.
//!
//! 1. **`metrics.savingsPercentage` carries a figure only where the metric is
//!    SUPPORTED for the view the request asked for.** `SummaryMetricsResult` is
//!    "the results or descriptions for the additional metrics"
//!    (`smithy_query(query="shape", shape="SummaryMetricsResult")`), so where the
//!    metric is not supported the member carries a description of why no figure
//!    is reported instead of a number. Two conditions decide supportedness, both
//!    about the requested view: the dimensions the request narrowed by (the
//!    model's note on the member: "Savings percentage is only supported when
//!    filtering by Region, account ID, or tags") and the caller's effective
//!    `savingsEstimationMode` preference, which is the undiscounted mode under
//!    `BeforeDiscounts` and so has no discounted spend to take a share of. An
//!    account that has stated no preference is served under the undiscounted
//!    mode, which is the one the service applies in its place.
//!
//! 2. **A refusal this service's HANDLER raises for a rule of the operation
//!    states, in the exception's top-level `message`, which member was rejected
//!    and the rule that rejects it** — the message is built from the request, so
//!    the actionable part of the refusal is in the member an SDK error surface
//!    shows. A member outside a constraint the MODEL declares keeps the fixed
//!    field-validation summary instead, because there the diagnosis is the field
//!    list the constraint walk produced; both classes still fill `reason` and
//!    `fields`.
//!
//! Each rule is asserted over a table spanning what it generalises over — every
//! `savingsEstimationMode` the model declares (and the account that stated
//! none) against each filter class and two grouping dimensions; every filter
//! member this API documents as unsupported, in two value forms, and every
//! handler-raised refusal of the service beside the constraint-class refusals
//! that keep the summary.

use serde_json::{json, Value};
use service_com_amazonaws_costoptimizationhub_costoptimizationhubservice as cost_optimization_hub;
use smithy_server_runtime::MultiServiceRouter;

use crate::cost_optimization_hub::{
    call, enrolled_router, error_code, fields, ok, router, FIELD_VALIDATION_FAILED,
    FIELD_VALIDATION_SUMMARY,
};

/// Every `SavingsEstimationMode` the model declares, read from the generated
/// catalogue so the table cannot drift from the enum
/// (`smithy_query(query="shape", shape="SavingsEstimationMode")`).
use cost_optimization_hub::ops::types::shared_types::SAVINGS_ESTIMATION_MODE_VALUES;

/// The discount-adjusted mode, and the mode the service applies to an account
/// that stated no preference — read from the service rather than copied here, so
/// the expectation is the one the handler computes against.
use cost_optimization_hub::state::{
    AFTER_DISCOUNTS, BEFORE_DISCOUNTS, DEFAULT_SAVINGS_ESTIMATION_MODE,
};

/// Money in whole cents, so two figures derived by different routes compare
/// exactly rather than by float identity.
fn cents(amount: f64) -> i64 {
    (amount * 100.0).round() as i64
}

/// The recommendations the store holds for a filter, as `ListRecommendations`
/// reports them — one row per resource when `deduped`, which is the set a
/// summary's totals are taken over.
async fn listed(router: &MultiServiceRouter, filter: &Value, deduped: bool) -> Vec<Value> {
    let payload = ok(
        router,
        "ListRecommendations",
        &json!({"includeAllRecommendations": !deduped, "filter": filter}),
    )
    .await;
    payload["items"].as_array().cloned().unwrap_or_default()
}

/// The monthly savings of a set of listed recommendations, in cents.
fn total_savings(items: &[Value]) -> i64 {
    items
        .iter()
        .map(|item| cents(item["estimatedMonthlySavings"].as_f64().unwrap_or_default()))
        .sum()
}

/// The monthly cost of a set of listed recommendations' current resources, in
/// cents — the spend the percentage is a share of.
fn total_cost(items: &[Value]) -> i64 {
    items
        .iter()
        .map(|item| cents(item["estimatedMonthlyCost"].as_f64().unwrap_or_default()))
        .sum()
}

/// Store one `savingsEstimationMode` preference and read it back through
/// `GetPreferences`, so a case knows the preference the figures are computed
/// under is the one the service holds.
async fn set_mode(router: &MultiServiceRouter, mode: &str) {
    ok(
        router,
        "UpdatePreferences",
        &json!({"savingsEstimationMode": mode}),
    )
    .await;
    let read = ok(router, "GetPreferences", &json!({})).await;
    assert_eq!(
        read["savingsEstimationMode"].as_str(),
        Some(mode),
        "case mode {mode:?}: expected the stored preference back, got {read}"
    );
}

/// The `savingsPercentage` the answer reports, which `SummaryMetricsResult`
/// models as a String whichever kind of value it carries.
fn reported_metric(payload: &Value) -> String {
    payload["metrics"]["savingsPercentage"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a savingsPercentage metric in {payload}"))
        .to_string()
}

// ---------------------------------------------------------------------------
// Rule 1 — the metric reports a figure only where it is supported
// ---------------------------------------------------------------------------

/// `metrics.savingsPercentage` is the de-duped saving's share of the summarised
/// resources' spend where the metric is supported, and the description of why no
/// figure is reported where it is not — whichever of the two conditions makes it
/// unsupported, and whatever dimension the view is grouped by.
#[tokio::test]
async fn savings_percentage_is_a_figure_only_where_the_metric_is_supported() {
    let router = enrolled_router().await;

    // (case, the mode to store first — `None` is the account that has stated no
    // preference, read under the default the service applies). Every value the
    // model's enum declares is stored in turn, and the first is stored again at
    // the end so the metric is shown to follow the preference in both
    // directions.
    let mut modes: Vec<(String, Option<&str>)> = vec![("no stored preference".to_string(), None)];
    for mode in SAVINGS_ESTIMATION_MODE_VALUES {
        modes.push((format!("stored {mode}"), Some(*mode)));
    }
    let first = SAVINGS_ESTIMATION_MODE_VALUES[0];
    modes.push((format!("back to {first}"), Some(first)));

    // (case, the filter sent, whether the metric is defined over the dimensions
    // it narrows by). The supported dimensions are the model's note's — Region,
    // account id and tags — of which `accountIds` is the organization view this
    // caller does not have (`management_account_gating`), so Region and tags are
    // the ones a case can send; the rest pick out individual recommendations
    // within the spend rather than scoping it.
    let filters: Vec<(&str, Value, bool)> = vec![
        ("no filter at all", json!({}), true),
        ("by region", json!({"regions": ["us-east-1"]}), true),
        (
            "by two regions",
            json!({"regions": ["us-east-1", "us-west-2"]}),
            true,
        ),
        (
            "by tag",
            json!({"tags": [{"key": "environment", "value": "production"}]}),
            true,
        ),
        (
            "by region and tag",
            json!({
                "regions": ["us-west-2"],
                "tags": [{"key": "environment", "value": "staging"}],
            }),
            true,
        ),
        (
            "by resource type",
            json!({"resourceTypes": ["Ec2Instance"]}),
            false,
        ),
        ("by action type", json!({"actionTypes": ["Rightsize"]}), false),
        ("by restartNeeded", json!({"restartNeeded": true}), false),
        ("by rollbackPossible", json!({"rollbackPossible": false}), false),
        (
            "by implementation effort",
            json!({"implementationEfforts": ["Low", "Medium"]}),
            false,
        ),
        (
            "by a supported and an unsupported dimension together",
            json!({"regions": ["us-east-1"], "resourceTypes": ["EbsVolume"]}),
            false,
        ),
    ];

    for (mode_case, mode) in &modes {
        if let Some(mode) = mode {
            set_mode(&router, mode).await;
        }
        let effective = mode.unwrap_or(DEFAULT_SAVINGS_ESTIMATION_MODE);
        let mode_supports = effective == AFTER_DISCOUNTS;

        for (filter_case, filter, filter_supports) in &filters {
            let deduped = listed(&router, filter, true).await;

            for group_by in ["ResourceType", "Region"] {
                let case = format!("{mode_case} / {filter_case} / groupBy {group_by}");
                let payload = ok(
                    &router,
                    "ListRecommendationSummaries",
                    &json!({
                        "groupBy": group_by,
                        "filter": filter,
                        "metrics": ["SavingsPercentage"],
                    }),
                )
                .await;
                let reported = reported_metric(&payload);

                // Where the metric is not supported the member carries a
                // description — never a number a client could read as a
                // measurement — naming the condition that makes it so.
                if !mode_supports || !*filter_supports {
                    assert!(
                        reported.parse::<f64>().is_err(),
                        "case {case:?}: expected a description, got the figure {reported:?} \
                         in {payload}"
                    );
                    assert!(
                        reported.contains("supported"),
                        "case {case:?}: expected a description of why no figure is reported, \
                         got {reported:?} in {payload}"
                    );
                    let reason = if !mode_supports {
                        "savings estimation mode"
                    } else {
                        "Region"
                    };
                    assert!(
                        reported.contains(reason),
                        "case {case:?}: expected the description to name {reason:?}, \
                         got {reported:?} in {payload}"
                    );
                    continue;
                }

                // Where it is supported the figure is the de-duped saving's
                // share of the spend of the same rows, read through the list
                // view: the oracle is the other view of the store.
                let spend = total_cost(&deduped);
                let expected = if spend == 0 {
                    0.0
                } else {
                    total_savings(&deduped) as f64 / spend as f64 * 100.0
                };
                let actual = reported.parse::<f64>().unwrap_or_else(|_| {
                    panic!("case {case:?}: expected a percentage, got {reported:?} in {payload}")
                });
                assert!(
                    (actual - expected).abs() < 0.02,
                    "case {case:?}: expected {expected} percent, got {actual} in {payload}"
                );
            }
        }
    }
}

/// An account that has stated no `savingsEstimationMode` preference is served
/// under the undiscounted mode, so the metric it asks for is answered with the
/// description and not with a figure — and moving the account to the discounted
/// mode makes the same request report a figure, so it is the preference in
/// effect that decides and not the absence of one.
#[tokio::test]
async fn an_account_that_stated_no_preference_is_served_before_discounts() {
    let router = enrolled_router().await;
    let request = json!({"groupBy": "Region", "metrics": ["SavingsPercentage"]});

    assert_eq!(
        DEFAULT_SAVINGS_ESTIMATION_MODE, BEFORE_DISCOUNTS,
        "case no preference: expected the undiscounted mode to be the one the service applies"
    );
    let read = ok(&router, "GetPreferences", &json!({})).await;
    assert_eq!(
        read["savingsEstimationMode"].as_str(),
        Some(BEFORE_DISCOUNTS),
        "case no preference: expected the undiscounted mode reported back, got {read}"
    );

    let before = ok(&router, "ListRecommendationSummaries", &request).await;
    let described = reported_metric(&before);
    assert!(
        described.parse::<f64>().is_err() && described.contains("savings estimation mode"),
        "case no preference: expected the description of the undiscounted mode, \
         got {described:?} in {before}"
    );

    set_mode(&router, AFTER_DISCOUNTS).await;
    let after = ok(&router, "ListRecommendationSummaries", &request).await;
    let figure = reported_metric(&after);
    assert!(
        figure.parse::<f64>().is_ok(),
        "case moved to the discounted mode: expected a figure, got {figure:?} in {after}"
    );
}

/// The metric is reported only where the request asks for it: an answer with no
/// `metrics` member carries no `metrics` object at all, under either preference.
#[tokio::test]
async fn no_metric_is_reported_where_none_was_asked_for() {
    let router = enrolled_router().await;

    for mode in SAVINGS_ESTIMATION_MODE_VALUES {
        set_mode(&router, mode).await;
        for request in [
            json!({"groupBy": "ActionType"}),
            json!({"groupBy": "Region", "filter": {"regions": ["us-east-1"]}}),
            json!({"groupBy": "ResourceType", "filter": {"resourceTypes": ["Ec2Instance"]}}),
        ] {
            let payload = ok(&router, "ListRecommendationSummaries", &request).await;
            assert!(
                payload["metrics"].is_null(),
                "case {mode} {request}: expected no metrics, got {payload}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2 — a handler-raised refusal states the member and the rule
// ---------------------------------------------------------------------------

/// The `filter` members this API documents as unsupported, each with the
/// identifier it filters on — the name the refusal states it by — and the member
/// path the refusal is attributed to.
///
/// Source: this operation's own documentation, "The following filters are not
/// supported for this API: recommendationIds, resourceArns, and resourceIds"
/// (`smithy_query(query="operation", operation="ListRecommendationSummaries")`).
const UNSUPPORTED_FILTERS: &[(&str, &str, &str)] = &[
    (
        "recommendationIds",
        "recommendationId",
        "filter.recommendationIds",
    ),
    ("resourceArns", "resourceArn", "filter.resourceArns"),
    ("resourceIds", "resourceId", "filter.resourceIds"),
];

/// Every filter this API documents as unsupported is refused with a
/// `ValidationException` whose `message` names the filter that was rejected —
/// not the fixed field-validation summary — while `reason` and `fields` still
/// carry the per-member diagnosis; and the SAME member is served by
/// `ListRecommendations`, which supports it, so the refusal is about this
/// operation and not about the member.
#[tokio::test]
async fn an_unsupported_filter_is_refused_with_a_message_naming_it() {
    let router = enrolled_router().await;

    // Two value forms per member — one identifier and two — because the rule is
    // about the member being present, not about what it holds.
    let values: [(&str, Value); 2] = [
        ("one value", json!(["0ec38a3a-1a0a-4a4f-9d2a-1f1e9a7c0001"])),
        (
            "two values",
            json!([
                "0ec38a3a-1a0a-4a4f-9d2a-1f1e9a7c0001",
                "0ec38a3a-1a0a-4a4f-9d2a-1f1e9a7c0002",
            ]),
        ),
    ];

    for (member, dimension, attributed) in UNSUPPORTED_FILTERS {
        for (form, value) in &values {
            let filter = json!({(*member): value});
            let case = format!("{member} with {form}");
            let (status, payload) = call(
                &router,
                "ListRecommendationSummaries",
                &json!({"groupBy": "ResourceType", "filter": filter, "metrics": ["SavingsPercentage"]}),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {payload}"
            );
            assert_eq!(
                error_code(&payload),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {payload}"
            );

            let message = payload["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(dimension),
                "case {case:?}: expected the message to name the rejected filter \
                 {dimension:?}, got {message:?} in {payload}"
            );
            assert!(
                message.contains("filter"),
                "case {case:?}: expected the message to state the rule about filters, \
                 got {message:?} in {payload}"
            );
            assert_ne!(
                message, FIELD_VALIDATION_SUMMARY,
                "case {case:?}: a refusal built from the request must not state the fixed \
                 summary, got {payload}"
            );
            // The structured half of the envelope is unchanged: the diagnosis is
            // in `fields` as well as in the message.
            assert_eq!(
                payload["reason"].as_str(),
                Some(FIELD_VALIDATION_FAILED),
                "case {case:?}: expected reason {FIELD_VALIDATION_FAILED:?}, got {payload}"
            );
            let named: Vec<String> = fields(&payload)
                .into_iter()
                .map(|(field_name, _)| field_name)
                .collect();
            assert_eq!(
                named,
                vec![(*attributed).to_string()],
                "case {case:?}: expected fields naming {attributed:?}, got {payload}"
            );

            // The same filter member on the list view, which supports it, is
            // served.
            let served = ok(
                &router,
                "ListRecommendations",
                &json!({"includeAllRecommendations": true, "filter": filter}),
            )
            .await;
            assert!(
                served["items"].is_array(),
                "case {case:?}: expected ListRecommendations to serve the filter, got {served}"
            );
        }
    }
}

/// Rule 2 over the whole service: every refusal a handler raises for a rule of
/// its operation states the member and the rule in `message`, and every refusal
/// of a member outside a constraint the model declares states the fixed summary.
#[tokio::test]
async fn the_message_states_the_rule_a_handler_enforces_and_summarises_the_rest() {
    let router = router();

    // (case, operation, request, the member the message must name, the rejected
    // value it must state where the rule rejects a value)
    let handler_raised: Vec<(&str, &str, Value, &str, Option<&str>)> = vec![
        (
            "a groupBy that names no dimension",
            "ListRecommendationSummaries",
            json!({"groupBy": "NotADimension"}),
            "groupBy",
            Some("NotADimension"),
        ),
        (
            "a groupBy naming a per-resource identifier",
            "ListRecommendationSummaries",
            json!({"groupBy": "ResourceId", "metrics": ["SavingsPercentage"]}),
            "groupBy",
            Some("ResourceId"),
        ),
        (
            "a filter this operation does not support",
            "ListRecommendationSummaries",
            json!({"groupBy": "ResourceType", "filter": {"resourceArns": ["arn:aws:ec2:::x"]}}),
            "filter",
            Some("resourceArn"),
        ),
        (
            "a nextToken this service never issued",
            "ListRecommendationSummaries",
            json!({"groupBy": "ActionType", "nextToken": "not-a-token"}),
            "nextToken",
            None,
        ),
        (
            "a nextToken this service never issued, on the enrollment list",
            "ListEnrollmentStatuses",
            json!({"includeOrganizationInfo": false, "nextToken": "not-a-token"}),
            "nextToken",
            None,
        ),
        (
            "a nextToken from another list",
            "ListRecommendations",
            json!({"nextToken": "6e6f7065"}),
            "nextToken",
            None,
        ),
        (
            "an orderBy dimension that names no member",
            "ListRecommendations",
            json!({"orderBy": {"dimension": "NotADimension", "order": "Asc"}}),
            "orderBy",
            Some("NotADimension"),
        ),
    ];

    for (case, operation, request, member, rejected) in &handler_raised {
        let (status, payload) = call(&router, operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        let message = payload["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(member),
            "case {case:?}: expected the message to name {member:?}, got {message:?} in {payload}"
        );
        if let Some(rejected) = rejected {
            assert!(
                message.contains(rejected),
                "case {case:?}: expected the message to state the rejected {rejected:?}, \
                 got {message:?} in {payload}"
            );
        }
        assert_ne!(
            message, FIELD_VALIDATION_SUMMARY,
            "case {case:?}: a refusal built from the request must not state the fixed summary, \
             got {payload}"
        );
        assert_eq!(
            payload["reason"].as_str(),
            Some(FIELD_VALIDATION_FAILED),
            "case {case:?}: expected reason {FIELD_VALIDATION_FAILED:?}, got {payload}"
        );
        assert_eq!(
            fields(&payload).len(),
            1,
            "case {case:?}: expected the member named in fields too, got {payload}"
        );
    }

    // The other side of the rule: a member outside a constraint the MODEL
    // declares — whether the constraint walk refuses it or a handler enforces
    // the member's own bound or value set — keeps the fixed summary, because
    // there the diagnosis is the field list.
    let constraint_class: Vec<(&str, &str, Value)> = vec![
        (
            "a pattern on a scalar member",
            "ListEnrollmentStatuses",
            json!({"includeOrganizationInfo": false, "accountId": "12345"}),
        ),
        (
            "a range the shape declares",
            "ListEnrollmentStatuses",
            json!({"includeOrganizationInfo": false, "maxResults": 101}),
        ),
        (
            "a range the member itself declares, enforced by the handler",
            "ListRecommendationSummaries",
            json!({"groupBy": "ActionType", "maxResults": 1001}),
        ),
        (
            "a range the member itself declares, on the list view",
            "ListRecommendations",
            json!({"maxResults": 1001}),
        ),
        (
            "a value set the service accepts on the wire",
            "UpdateEnrollmentStatus",
            json!({"status": "Enabled"}),
        ),
        (
            "an enum value set the model declares",
            "UpdatePreferences",
            json!({"savingsEstimationMode": "Nope"}),
        ),
        (
            // The metrics this operation reports are a modelled enum
            // (`SummaryMetrics`), so a metric it cannot report is a value
            // outside a declared set and the constraint walk refuses it — the
            // handler's own check for one is reached only if that set ever
            // widens.
            "a metric outside the modelled set",
            "ListRecommendationSummaries",
            json!({"groupBy": "ActionType", "metrics": ["CostPercentage"]}),
        ),
        (
            "a required member absent",
            "GetRecommendation",
            json!({}),
        ),
    ];

    for (case, operation, request) in &constraint_class {
        let (status, payload) = call(&router, operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            payload["message"].as_str(),
            Some(FIELD_VALIDATION_SUMMARY),
            "case {case:?}: expected the fixed field-validation summary, got {payload}"
        );
        assert_eq!(
            payload["reason"].as_str(),
            Some(FIELD_VALIDATION_FAILED),
            "case {case:?}: expected reason {FIELD_VALIDATION_FAILED:?}, got {payload}"
        );
    }
}
