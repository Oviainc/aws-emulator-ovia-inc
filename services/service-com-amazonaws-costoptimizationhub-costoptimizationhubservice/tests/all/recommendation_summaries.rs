// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListRecommendationSummaries`, driven over the wire
//! (`awsJson1_0`) through the same multi-service router the emulator boots.
//!
//! The rule under test: a summary is the recommendation catalog aggregated along
//! one `groupBy` dimension — one group per distinct value of that dimension,
//! carrying the number of recommendations that fall under it and what they save
//! per month together — while `estimatedTotalDedupedSavings` is that total taken
//! over one recommendation per resource, so a resource several recommendation
//! types cover is counted once.
//!
//! Every case checks that rule against the SAME rows read through
//! `ListRecommendations`, aggregated by the test: the oracle is the other view of
//! the store, never a number copied into this file. The table spans every
//! grouping dimension, the spellings a dimension can be named by, each filter
//! class, every page size, and both `savingsEstimationMode` preferences.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cost_optimization_hub::{
    call, enrolled_router, error_code, ok, router, ANALYSIS_WAIT,
};

/// The `groupBy` values this service groups by, each paired with the
/// `Recommendation` member it names, so a case can aggregate the list view along
/// the same member.
///
/// The names are the model's own (`smithy_query(query="shape",
/// shape="Recommendation")`), with `ResourceType` the name
/// `smithy_query(query="shape", shape="Filter")` gives the type the resource has
/// today (`resourceTypes`), which is the type a recommendation is about.
const GROUPINGS: &[(&str, &str)] = &[
    ("AccountId", "accountId"),
    ("Region", "region"),
    ("ResourceType", "currentResourceType"),
    ("CurrentResourceType", "currentResourceType"),
    ("RecommendedResourceType", "recommendedResourceType"),
    ("ActionType", "actionType"),
    ("ImplementationEffort", "implementationEffort"),
    ("Source", "source"),
    ("CurrencyCode", "currencyCode"),
    ("RestartNeeded", "restartNeeded"),
    ("RollbackPossible", "rollbackPossible"),
    // The dimension is resolved case-insensitively, as `orderBy`'s is, so the
    // SDK spelling and the lower-case one name the same member.
    ("actiontype", "actionType"),
    ("REGION", "region"),
];

/// One group of a summary answer: its label, the savings it reports (in whole
/// cents, which is the precision every money figure this service reports is
/// rounded to) and the recommendations it counts.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Group {
    label: String,
    savings_cents: i64,
    count: i64,
}

/// Money in whole cents, so two figures derived by different routes compare
/// exactly rather than by float identity.
fn cents(amount: f64) -> i64 {
    (amount * 100.0).round() as i64
}

/// The groups a `ListRecommendationSummaries` answer reports.
fn groups(payload: &Value) -> Vec<Group> {
    payload["items"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|item| Group {
            label: item["group"].as_str().unwrap_or_default().to_string(),
            savings_cents: cents(item["estimatedMonthlySavings"].as_f64().unwrap_or_default()),
            count: item["recommendationCount"].as_i64().unwrap_or_default(),
        })
        .collect()
}

/// The label a recommendation falls under for one dimension: the member's value
/// as text, whatever type the member carries.
fn label(item: &Value, member: &str) -> String {
    match &item[member] {
        Value::String(text) => text.clone(),
        Value::Bool(flag) => flag.to_string(),
        other => panic!("case member {member:?}: no label for {other}"),
    }
}

/// The groups the listed rows must produce for one dimension: biggest saving
/// first, ties by label, which is the order a summary ranks its groups in.
fn expected_groups(items: &[Value], member: &str) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for item in items {
        let label = label(item, member);
        let savings = cents(item["estimatedMonthlySavings"].as_f64().unwrap_or_default());
        match groups.iter_mut().find(|group| group.label == label) {
            Some(group) => {
                group.savings_cents += savings;
                group.count += 1;
            }
            None => groups.push(Group {
                label,
                savings_cents: savings,
                count: 1,
            }),
        }
    }
    groups.sort_by(|a, b| {
        b.savings_cents
            .cmp(&a.savings_cents)
            .then_with(|| a.label.cmp(&b.label))
    });
    groups
}

/// The recommendations the store holds for a filter, as `ListRecommendations`
/// reports them: every one of them, or one per resource when `deduped`.
async fn listed(router: &MultiServiceRouter, filter: &Value, deduped: bool) -> Vec<Value> {
    let payload = ok(
        router,
        "ListRecommendations",
        &json!({"includeAllRecommendations": !deduped, "filter": filter}),
    )
    .await;
    payload["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The monthly savings of a set of listed recommendations, in cents.
fn total_savings(items: &[Value]) -> i64 {
    items
        .iter()
        .map(|item| cents(item["estimatedMonthlySavings"].as_f64().unwrap_or_default()))
        .sum()
}

/// The monthly cost of a set of listed recommendations' current resources, in
/// cents.
fn total_cost(items: &[Value]) -> i64 {
    items
        .iter()
        .map(|item| cents(item["estimatedMonthlyCost"].as_f64().unwrap_or_default()))
        .sum()
}

/// Store one `savingsEstimationMode` preference, so the figures a later summary
/// reports are the ones that mode names.
async fn set_mode(router: &MultiServiceRouter, mode: &str) {
    ok(
        router,
        "UpdatePreferences",
        &json!({"savingsEstimationMode": mode}),
    )
    .await;
}

// ---------------------------------------------------------------------------
// Grouping
// ---------------------------------------------------------------------------

/// Every grouping dimension aggregates the catalog along the member it names:
/// one group per distinct value, counting the recommendations under it and
/// totalling what they save, with the request's `groupBy` echoed back.
#[tokio::test]
async fn a_summary_aggregates_the_catalog_along_the_grouping_dimension() {
    let router = enrolled_router().await;
    let items = listed(&router, &json!({}), false).await;

    for (group_by, member) in GROUPINGS {
        let payload = ok(
            &router,
            "ListRecommendationSummaries",
            &json!({"groupBy": group_by}),
        )
        .await;

        let expected = expected_groups(&items, member);
        let actual = groups(&payload);
        assert_eq!(
            actual, expected,
            "case groupBy {group_by:?}: expected {expected:?}, got {payload}"
        );
        assert!(
            !expected.is_empty(),
            "case groupBy {group_by:?}: the catalog must produce at least one group"
        );
        assert_eq!(
            payload["groupBy"].as_str(),
            Some(*group_by),
            "case groupBy {group_by:?}: expected the grouping echoed back, got {payload}"
        );
        assert_eq!(
            payload["currencyCode"].as_str(),
            items[0]["currencyCode"].as_str(),
            "case groupBy {group_by:?}: expected the catalog's currency, got {payload}"
        );
        // Every recommendation lands in exactly one group.
        let counted: i64 = actual.iter().map(|group| group.count).sum();
        assert_eq!(
            counted,
            items.len() as i64,
            "case groupBy {group_by:?}: expected {} recommendations across the groups, got {payload}",
            items.len()
        );
        // A group's savings is the sum of its rows', so the groups together are
        // the whole (not de-duped) saving.
        let grouped: i64 = actual.iter().map(|group| group.savings_cents).sum();
        assert_eq!(
            grouped,
            total_savings(&items),
            "case groupBy {group_by:?}: expected the listed savings in total, got {payload}"
        );
    }
}

/// `estimatedTotalDedupedSavings` counts each resource once — the saving of its
/// best recommendation — whatever dimension the view is grouped by, and under
/// either `savingsEstimationMode`. The catalog holds a resource with two
/// recommendations, so the de-duped total is strictly below the grouped one.
#[tokio::test]
async fn the_total_is_deduped_across_recommendation_types() {
    let router = enrolled_router().await;

    // (case, the mode to store first) — the absent preference is the store's
    // initial value, read under the mode the service applies by default
    // (`BeforeDiscounts`), and each value of the model's enum is then stored
    // explicitly. The de-duped total is checked against the list view under the
    // same mode, so every row is mode-agnostic by construction.
    let modes: [(&str, Option<&str>); 3] = [
        ("no stored preference", None),
        ("BeforeDiscounts", Some("BeforeDiscounts")),
        ("AfterDiscounts", Some("AfterDiscounts")),
    ];

    for (case, mode) in modes {
        if let Some(mode) = mode {
            set_mode(&router, mode).await;
        }
        let all = listed(&router, &json!({}), false).await;
        let deduped = listed(&router, &json!({}), true).await;
        let expected = total_savings(&deduped);
        assert!(
            deduped.len() < all.len(),
            "case {case:?}: the catalog must hold a resource with several recommendations"
        );

        for (group_by, _) in GROUPINGS {
            let payload = ok(
                &router,
                "ListRecommendationSummaries",
                &json!({"groupBy": group_by}),
            )
            .await;
            let actual = cents(
                payload["estimatedTotalDedupedSavings"]
                    .as_f64()
                    .unwrap_or_default(),
            );
            assert_eq!(
                actual, expected,
                "case {case:?} groupBy {group_by:?}: expected {expected} cents de-duped, got {payload}"
            );
            assert!(
                actual < total_savings(&all),
                "case {case:?} groupBy {group_by:?}: the de-duped total must drop the \
                 second recommendation for the shared resource, got {payload}"
            );
        }
    }
}

/// Each `filter` class narrows the rows before they are grouped, so a filtered
/// summary is the summary of exactly the rows the same filter selects from
/// `ListRecommendations`.
#[tokio::test]
async fn a_filter_narrows_the_rows_before_they_are_grouped() {
    let router = enrolled_router().await;

    let filters: Vec<Value> = vec![
        json!({}),
        json!({"regions": ["us-west-2"]}),
        json!({"regions": ["us-east-1", "us-west-2"]}),
        json!({"resourceTypes": ["Ec2Instance"]}),
        json!({"resourceTypes": ["EbsVolume", "NatGateway"]}),
        json!({"actionTypes": ["Rightsize"]}),
        json!({"implementationEfforts": ["High", "VeryHigh"]}),
        json!({"restartNeeded": true}),
        json!({"rollbackPossible": false}),
        json!({"tags": [{"key": "environment", "value": "production"}]}),
        json!({"tags": [{"key": "team"}]}),
        // A filter that selects nothing: an empty summary, not a refusal.
        json!({"regions": ["eu-west-1"]}),
        json!({"regions": ["us-west-2"], "resourceTypes": ["Ec2Instance"]}),
        json!({"regions": ["us-east-1"], "actionTypes": ["Rightsize"]}),
    ];

    for filter in &filters {
        let items = listed(&router, filter, false).await;
        let deduped = listed(&router, filter, true).await;

        for (group_by, member) in GROUPINGS {
            let payload = ok(
                &router,
                "ListRecommendationSummaries",
                &json!({"groupBy": group_by, "filter": filter}),
            )
            .await;
            let expected = expected_groups(&items, member);
            let actual = groups(&payload);
            assert_eq!(
                actual, expected,
                "case filter {filter} groupBy {group_by:?}: expected {expected:?}, got {payload}"
            );
            assert_eq!(
                cents(
                    payload["estimatedTotalDedupedSavings"]
                        .as_f64()
                        .unwrap_or_default()
                ),
                total_savings(&deduped),
                "case filter {filter} groupBy {group_by:?}: expected the de-duped total of \
                 the filtered rows, got {payload}"
            );
            // The currency is reported even where the filter selected no row.
            assert!(
                payload["currencyCode"].as_str().is_some_and(|code| !code.is_empty()),
                "case filter {filter} groupBy {group_by:?}: expected a currency, got {payload}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Pagination over the groups resumes where the previous page ended, for every
/// page size; the last page carries no token, a zero-size page ends the walk,
/// and a token this list did not issue is refused.
#[tokio::test]
async fn summary_pagination_resumes_where_the_previous_page_ended() {
    let router = enrolled_router().await;

    for group_by in ["ActionType", "Region", "ImplementationEffort"] {
        let whole = ok(
            &router,
            "ListRecommendationSummaries",
            &json!({"groupBy": group_by}),
        )
        .await;
        let expected = groups(&whole);

        for page_size in [1_i64, 2, 3, expected.len() as i64, 50] {
            let mut collected: Vec<Group> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut request = json!({"groupBy": group_by, "maxResults": page_size});
                if let Some(token) = &token {
                    request["nextToken"] = json!(token);
                }
                let payload = ok(&router, "ListRecommendationSummaries", &request).await;
                let page = groups(&payload);
                assert!(
                    page.len() as i64 <= page_size,
                    "case groupBy {group_by:?} maxResults {page_size}: a page returned {} groups, got {payload}",
                    page.len()
                );
                collected.extend(page);
                pages += 1;
                assert!(
                    pages <= expected.len() + 1,
                    "case groupBy {group_by:?} maxResults {page_size}: pagination did not \
                     terminate after {pages} pages"
                );
                match payload["nextToken"].as_str() {
                    None => break,
                    Some(next) => token = Some(next.to_string()),
                }
            }
            assert_eq!(
                collected, expected,
                "case groupBy {group_by:?} maxResults {page_size}: paging must yield every \
                 group exactly once"
            );
        }
    }

    // A zero-size page cannot advance, so it ends the walk rather than handing
    // out a token that would repeat forever.
    let empty = ok(
        &router,
        "ListRecommendationSummaries",
        &json!({"groupBy": "ActionType", "maxResults": 0}),
    )
    .await;
    assert_eq!(
        groups(&empty),
        Vec::new(),
        "case maxResults 0: expected an empty page, got {empty}"
    );
    assert!(
        empty["nextToken"].is_null(),
        "case maxResults 0: expected no nextToken, got {empty}"
    );

    // A token this operation did not issue is refused, including one issued for
    // another paginated list of the same service.
    let from_recommendations = ok(
        &router,
        "ListRecommendations",
        &json!({"includeAllRecommendations": true, "maxResults": 1}),
    )
    .await;
    let foreign = from_recommendations["nextToken"]
        .as_str()
        .expect("a one-row page of the catalog leaves more rows")
        .to_string();
    for token in ["nope", foreign.as_str()] {
        let (status, payload) = call(
            &router,
            "ListRecommendationSummaries",
            &json!({"groupBy": "ActionType", "nextToken": token}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case nextToken {token:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case nextToken {token:?}: expected a ValidationException, got {payload}"
        );
    }
}

// ---------------------------------------------------------------------------
// The additional metric
// ---------------------------------------------------------------------------

/// `metrics.savingsPercentage` is reported only when the request asks for it,
/// as the de-duped saving's share of what the summarised resources cost — and
/// as the metric's own description where the view is narrowed by a dimension
/// the metric is not defined over (the model's note: "Savings percentage is only
/// supported when filtering by Region, account ID, or tags").
///
/// The account is moved to the discounted `savingsEstimationMode` first, because
/// the metric is supported only under that mode — the other half of the rule,
/// which `summary_metrics_and_refusals` owns.
#[tokio::test]
async fn the_savings_percentage_metric_is_reported_only_when_requested() {
    let router = enrolled_router().await;
    set_mode(&router, "AfterDiscounts").await;

    // No metric asked for: nothing to report.
    for request in [
        json!({"groupBy": "ActionType"}),
        json!({"groupBy": "Region", "filter": {"regions": ["us-east-1"]}}),
    ] {
        let payload = ok(&router, "ListRecommendationSummaries", &request).await;
        assert!(
            payload["metrics"].is_null(),
            "case {request}: expected no metrics, got {payload}"
        );
    }

    // (case, filter, whether the metric is defined over that view). An
    // `accountIds` filter is the organization view this caller does not have
    // (`management_account_gating`), so the dimensions left to narrow by are
    // Region and tags.
    let cases: Vec<(&str, Value, bool)> = vec![
        ("no filter at all", json!({}), true),
        ("by region", json!({"regions": ["us-east-1"]}), true),
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
        ("by action type", json!({"actionTypes": ["Delete"]}), false),
        ("by restartNeeded", json!({"restartNeeded": true}), false),
        (
            "by implementation effort",
            json!({"implementationEfforts": ["Low"]}),
            false,
        ),
    ];

    for (case, filter, defined) in &cases {
        let payload = ok(
            &router,
            "ListRecommendationSummaries",
            &json!({
                "groupBy": "ResourceType",
                "filter": filter,
                "metrics": ["SavingsPercentage"],
            }),
        )
        .await;
        let reported = payload["metrics"]["savingsPercentage"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        if !*defined {
            assert!(
                reported.parse::<f64>().is_err() && reported.contains("Region"),
                "case {case:?}: expected the metric's description, got {reported:?} in {payload}"
            );
            continue;
        }

        let deduped = listed(&router, filter, true).await;
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

/// A metric this operation cannot report is refused, naming the member it came
/// from, rather than served with the metric silently missing.
#[tokio::test]
async fn an_unreportable_metric_is_refused() {
    let router = router();

    for metric in ["savingsPercentage", "CostPercentage", ""] {
        let (status, payload) = call(
            &router,
            "ListRecommendationSummaries",
            &json!({"groupBy": "ActionType", "metrics": [metric]}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case metric {metric:?}: expected HTTP 400, got {status} with {payload}"
        );
        let named: Vec<String> = payload["fields"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|entry| entry["fieldName"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            named,
            vec!["metrics".to_string()],
            "case metric {metric:?}: expected the refusal to name metrics, got {payload}"
        );
    }
}

// ---------------------------------------------------------------------------
// Visibility
// ---------------------------------------------------------------------------

/// A summary reports the recommendations the caller may read, which is the
/// visibility the list view serves: an account that never opted in, one that
/// opted out, and one that has just opted in all summarise the empty set — no
/// groups and a zero total — and the currency the zero is in is still reported.
#[tokio::test]
async fn a_summary_reports_only_what_the_caller_may_read() {
    let router = router();
    let request = json!({"groupBy": "ResourceType", "metrics": ["SavingsPercentage"]});
    // The discounted mode, under which the metric reports a figure at all
    // (`summary_metrics_and_refusals`), so the percentage below follows what is
    // visible rather than the mode.
    set_mode(&router, "AfterDiscounts").await;

    // (the status this step moves the account to — `None` is "never called" —
    // whether the summary reports groups in the instant after that call, and
    // whether it reports them once the analysis has had time to run)
    let cases: [(Option<&str>, bool, bool); 4] = [
        (None, false, false),
        (Some("Active"), false, true),
        (Some("Inactive"), false, false),
        (Some("Active"), false, true),
    ];

    for (step, (status, now_visible, later_visible)) in cases.iter().enumerate() {
        let case = format!("step {step} status {status:?}");
        if let Some(status) = status {
            ok(
                &router,
                "UpdateEnrollmentStatus",
                &json!({"status": status, "includeMemberAccounts": false}),
            )
            .await;
        }

        for (when, expected) in [("immediately", now_visible), ("once due", later_visible)] {
            if when == "once due" {
                std::thread::sleep(ANALYSIS_WAIT);
            }
            let payload = ok(&router, "ListRecommendationSummaries", &request).await;
            let reported = groups(&payload);
            assert_eq!(
                !reported.is_empty(),
                *expected,
                "case {case} {when}: expected reported={expected}, got {payload}"
            );
            let total = cents(
                payload["estimatedTotalDedupedSavings"]
                    .as_f64()
                    .unwrap_or_default(),
            );
            assert_eq!(
                total > 0,
                *expected,
                "case {case} {when}: the total must follow what is reported, got {payload}"
            );
            assert!(
                payload["currencyCode"]
                    .as_str()
                    .is_some_and(|code| !code.is_empty()),
                "case {case} {when}: expected a currency, got {payload}"
            );
            // The metric is still answered, as the share of a spend that is
            // zero while nothing is visible.
            let percentage = payload["metrics"]["savingsPercentage"]
                .as_str()
                .unwrap_or_default()
                .parse::<f64>()
                .unwrap_or(-1.0);
            assert_eq!(
                percentage > 0.0,
                *expected,
                "case {case} {when}: expected a percentage following what is reported, \
                 got {payload}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Reporting, not mutating
// ---------------------------------------------------------------------------

/// Summarising the catalog changes nothing a later caller sees: the same request
/// answers the same thing before and after the other readers run, and the list
/// view is unchanged by it.
#[tokio::test]
async fn summarising_does_not_change_what_a_later_caller_sees() {
    let router = enrolled_router().await;
    let request = json!({
        "groupBy": "ResourceType",
        "metrics": ["SavingsPercentage"],
    });
    let listing = json!({"includeAllRecommendations": true});

    let first_list = ok(&router, "ListRecommendations", &listing).await;
    let first = ok(&router, "ListRecommendationSummaries", &request).await;
    ok(&router, "ListRecommendationSummaries", &request).await;
    ok(&router, "GetPreferences", &json!({})).await;
    let second = ok(&router, "ListRecommendationSummaries", &request).await;
    let second_list = ok(&router, "ListRecommendations", &listing).await;

    assert_eq!(
        first, second,
        "case repeated summary: the answer changed between two reads"
    );
    assert_eq!(
        first_list, second_list,
        "case repeated listing: summarising changed the catalog"
    );
}
