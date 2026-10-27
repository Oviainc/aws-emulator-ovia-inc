// SPDX-License-Identifier: CC0-1.0

//! The ONE recommendation view both recommendation readers serve from.
//!
//! `ListRecommendations` and `ListRecommendationSummaries` select their rows
//! through the same rule: a populated `filter.accountIds` asks for the
//! recommendations of accounts other than the caller's, which only the
//! organization's management account may do; otherwise the rows are the ones the
//! account may read — an enrolled account whose analysis has come due — narrowed
//! by every member the filter sets, under the caller's effective
//! `savingsEstimationMode`.
//!
//! These tests drive that one path through BOTH operations over the wire and
//! assert they agree, so the rule cannot be changed for one reader and left
//! behind in the other. Each assertion is over a table whose rows differ in
//! which filter member narrows the view, how many values it names, and how many
//! rows it leaves.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cost_optimization_hub::{
    call, enrolled_router, enrolled_router_with_discounts_visible, error_code, ok, router,
};

/// Two money figures are the same figure: both readers round to cents, so
/// anything inside half a cent is the same value.
fn same_money(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() < 0.005
}

/// Every recommendation `ListRecommendations` selects for `filter`.
///
/// `include_all` chooses between the whole selection and the one-per-resource
/// de-duped view; no `maxResults` is sent, so the answer is the whole view
/// rather than a page of it.
async fn listed(
    router: &MultiServiceRouter,
    filter: Option<&Value>,
    include_all: bool,
) -> Vec<Value> {
    let mut request = json!({"includeAllRecommendations": include_all});
    if let Some(filter) = filter {
        request["filter"] = filter.clone();
    }
    let payload = ok(router, "ListRecommendations", &request).await;
    payload["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The summary of the same selection, grouped by `group_by`.
async fn summarised(
    router: &MultiServiceRouter,
    filter: Option<&Value>,
    group_by: &str,
) -> Value {
    let mut request = json!({"groupBy": group_by});
    if let Some(filter) = filter {
        request["filter"] = filter.clone();
    }
    ok(router, "ListRecommendationSummaries", &request).await
}

/// The distinct values of one member across a list of recommendations, sorted.
fn distinct(items: &[Value], member: &str) -> Vec<String> {
    let mut values: Vec<String> = items
        .iter()
        .map(|item| item[member].as_str().unwrap_or_default().to_string())
        .collect();
    values.sort();
    values.dedup();
    values
}

/// The group labels a summary answer carries, sorted.
fn groups(payload: &Value) -> Vec<String> {
    let mut labels: Vec<String> = payload["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item["group"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    labels.sort();
    labels
}

/// The filter tables below span the members a recommendation view can be
/// narrowed by: the three the summaries API does not support
/// (`recommendationIds`, `resourceArns`, `resourceIds`) are left out because
/// only one of the two readers serves them, and `accountIds` has its own table.
fn filter_cases() -> Vec<(&'static str, Option<Value>)> {
    vec![
        ("no filter at all", None),
        ("one region", Some(json!({"regions": ["us-east-1"]}))),
        ("the other region", Some(json!({"regions": ["us-west-2"]}))),
        (
            "both regions",
            Some(json!({"regions": ["us-east-1", "us-west-2"]})),
        ),
        (
            "a region nothing is in",
            Some(json!({"regions": ["eu-west-1"]})),
        ),
        (
            "one resource type",
            Some(json!({"resourceTypes": ["Ec2Instance"]})),
        ),
        (
            "two resource types",
            Some(json!({"resourceTypes": ["EbsVolume", "LambdaFunction"]})),
        ),
        ("one action type", Some(json!({"actionTypes": ["Rightsize"]}))),
        ("restart needed", Some(json!({"restartNeeded": true}))),
        ("restart not needed", Some(json!({"restartNeeded": false}))),
        ("rollback possible", Some(json!({"rollbackPossible": true}))),
        (
            "an implementation effort",
            Some(json!({"implementationEfforts": ["VeryLow"]})),
        ),
        (
            "a tag key and value",
            Some(json!({"tags": [{"key": "environment", "value": "production"}]})),
        ),
        (
            "a region and a tag together",
            Some(json!({
                "regions": ["us-west-2"],
                "tags": [{"key": "environment", "value": "staging"}],
            })),
        ),
    ]
}

/// Both readers select the same rows for the same filter: the summaries
/// aggregate exactly the recommendations the list's own de-duped view returns —
/// one per resource of the rows the filter selects — group them by the values
/// those recommendations carry, and total the saving that view adds up to.
#[tokio::test]
async fn both_recommendation_readers_select_the_same_rows() {
    let router = enrolled_router().await;

    for (case, filter) in filter_cases() {
        let filter = filter.as_ref();
        let items = listed(&router, filter, true).await;
        // The view a summary aggregates: one recommendation per resource, which
        // is what the list serves without `includeAllRecommendations`.
        let deduped = listed(&router, filter, false).await;

        // Grouped by region: one group per distinct region among the de-duped
        // rows, and the counts add up to exactly those rows — one per resource
        // of the whole selection.
        let by_region = summarised(&router, filter, "Region").await;
        let counted: i64 = by_region["items"]
            .as_array()
            .map(|groups| {
                groups
                    .iter()
                    .map(|group| group["recommendationCount"].as_i64().unwrap_or_default())
                    .sum()
            })
            .unwrap_or_default();
        assert_eq!(
            counted,
            deduped.len() as i64,
            "case {case:?}: the summaries counted {counted} recommendations where the list's \
             de-duped view returned {}, so the two readers selected different rows: {by_region}",
            deduped.len()
        );
        assert_eq!(
            counted,
            distinct(&items, "resourceId").len() as i64,
            "case {case:?}: expected one counted recommendation per resource of the listed \
             rows, got {by_region}"
        );
        assert_eq!(
            groups(&by_region),
            distinct(&deduped, "region"),
            "case {case:?}: expected one group per region of the de-duped rows, got {by_region}"
        );

        // Grouped by another dimension: the same selection, so the same counts
        // and one group per distinct resource type.
        let by_type = summarised(&router, filter, "ResourceType").await;
        assert_eq!(
            groups(&by_type),
            distinct(&deduped, "currentResourceType"),
            "case {case:?}: expected one group per resource type of the de-duped rows, got \
             {by_type}"
        );

        // The de-duped total is of the same view: the saving of the list's own
        // de-duped page, one recommendation per resource.
        let expected: f64 = deduped
            .iter()
            .map(|item| item["estimatedMonthlySavings"].as_f64().unwrap_or_default())
            .sum();
        let reported = by_region["estimatedTotalDedupedSavings"]
            .as_f64()
            .unwrap_or_default();
        assert!(
            same_money(reported, expected),
            "case {case:?}: expected the de-duped total {expected} the list adds up to, got \
             {reported} in {by_region}"
        );
    }
}

/// The organization gate is one rule: a populated `filter.accountIds` is refused
/// to a caller that is not the organization's management account — with the 403
/// `AccessDeniedException` each operation declares — and served, by both
/// readers, to the management account.
#[tokio::test]
async fn the_account_ids_gate_answers_the_same_for_both_readers() {
    let member = enrolled_router().await;
    let account = engine::emulator_account_id();

    // (case, the `accountIds` the filter names). The rows differ in which ids
    // the member names, how many, and what else the filter narrows by.
    let cases: Vec<(&str, Value)> = vec![
        ("the caller's own account", json!({"accountIds": [account]})),
        (
            "an account that owns nothing in the catalog",
            json!({"accountIds": ["111122223333"]}),
        ),
        (
            "two accounts",
            json!({"accountIds": ["111122223333", "444455556666"]}),
        ),
        (
            "beside another filter member",
            json!({"accountIds": [account], "regions": ["us-east-1"]}),
        ),
    ];

    for (case, filter) in &cases {
        // Both readers refuse it, and word the refusal as the view the caller
        // does not have rather than as a bad value.
        for (operation, body) in [
            (
                "ListRecommendations",
                json!({"includeAllRecommendations": true, "filter": filter}),
            ),
            (
                "ListRecommendationSummaries",
                json!({"groupBy": "Region", "filter": filter}),
            ),
        ] {
            let (status, payload) = call(&member, operation, &body).await;
            assert_eq!(
                status, 403,
                "case {case:?} {operation}: expected HTTP 403, got {status} with {payload}"
            );
            assert_eq!(
                error_code(&payload),
                "AccessDeniedException",
                "case {case:?} {operation}: expected an AccessDeniedException, got {payload}"
            );
            assert!(
                payload["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("management account")),
                "case {case:?} {operation}: expected a message naming the rule, got {payload}"
            );
        }
    }

    // The other arm of the same gate: the organization's management account is
    // served the filter, by both readers, and they agree on what it selects.
    let management = enrolled_router_with_discounts_visible().await;
    let filter = json!({"accountIds": [account]});
    let items = listed(&management, Some(&filter), true).await;
    assert!(
        !items.is_empty(),
        "case management account: expected the account's recommendations, got {items:?}"
    );
    let summary = summarised(&management, Some(&filter), "AccountId").await;
    assert_eq!(
        groups(&summary),
        distinct(&items, "accountId"),
        "case management account: expected one group per listed account, got {summary}"
    );
}

/// The visibility half of the same rule: an account that has not opted in reads
/// no recommendations, so neither reader reports a catalog — no items from the
/// list, no groups and a zero total from the summaries.
#[tokio::test]
async fn neither_reader_reports_a_catalog_the_account_has_not_opted_into() {
    let router = router();

    for (case, filter) in filter_cases() {
        let filter = filter.as_ref();
        let items = listed(&router, filter, true).await;
        assert!(
            items.is_empty(),
            "case {case:?}: expected no recommendations before enrollment, got {items:?}"
        );

        let summary = summarised(&router, filter, "Region").await;
        assert!(
            groups(&summary).is_empty(),
            "case {case:?}: expected no summary groups before enrollment, got {summary}"
        );
        let total = summary["estimatedTotalDedupedSavings"]
            .as_f64()
            .unwrap_or_default();
        assert!(
            same_money(total, 0.0),
            "case {case:?}: expected a zero de-duped total before enrollment, got {total} in \
             {summary}"
        );
    }
}
