// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the PAGING contract of `ListAgentRecommendations` — the two
//! rules a page is held to besides which recommendations it reports (those are pinned by
//! `super::agent_recommendations`).
//!
//! The rules pinned here, each over a TABLE of values rather than one request:
//!
//! 1. `maxResults` is held to the range its member publishes, `@range(min: 1, max: 1000)`
//!    (`smithy_query(query="operation", operation="ListAgentRecommendations")`): each
//!    published bound serves exactly the page it asks for, and a value one past either
//!    bound is refused `400 ValidationException` at the boundary.
//! 2. `nextToken` is this service's OWN opaque cursor, so a token it did not issue is
//!    refused `400 ValidationException` with reason `CANNOT_PARSE` naming the member —
//!    never an empty page, and never the first page again. A cursor ANOTHER paginated
//!    operation of this service issued (`ListAgentContexts`, `ListWorkloads`) is such a
//!    token: it is collected from a real page rather than composed by hand, so each row is
//!    a cursor this emulator really hands out. A refused request changes nothing — the
//!    listing it refused goes on answering exactly what it did before. The literal
//!    `not-a-cursor` row is the degenerate case of the same table and overlaps one row of
//!    `super::agent_recommendations`; the cross-operation rows are this suite's own.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The recommendations are produced the way the service produces them — by a
//! generation whose analysis the clock has finished.

use serde_json::{json, Value};

use super::agent_contexts::create_context;
use super::agent_goals::{profile_arn_of, PILLARS};
use super::agent_recommendations::{recommendations_of, start_generation, until_complete};
use super::workload_harness::{encode_path_segment, minimal_create, Answer, Client};

/// The published bounds of `ListAgentRecommendationsRequest$maxResults`
/// (`@range(min: 1, max: 1000)` — `smithy_query(query="operation",
/// operation="ListAgentRecommendations")`).
const MAX_RESULTS_MIN: i64 = 1;
/// See [`MAX_RESULTS_MIN`].
const MAX_RESULTS_MAX: i64 = 1_000;

/// A `RecommendationType` the generations here are started for
/// (`smithy_query(query="shape", shape="RecommendationType")`).
const RECOMMENDATION_TYPE: &str = "ARCHITECTURE";

/// `GET /api/v1/agent-profiles/{profileArn}/recommendations`, with `query` carrying the
/// paging member the row exercises (already percent-encoded, empty for a request that
/// names none). The ARN is a non-greedy `@httpLabel`, so it travels percent-encoded in one
/// path segment, exactly as an SDK sends it.
async fn list_recommendations(client: &Client, profile_arn: &str, query: &str) -> Answer {
    client
        .send(
            "GET",
            &format!(
                "/api/v1/agent-profiles/{}/recommendations{query}",
                encode_path_segment(profile_arn)
            ),
            None,
        )
        .await
}

/// `GET /api/v1/agent-profiles/{profileArn}/contexts` — the sibling listing this suite
/// collects a cursor from, so the token under test is one this service really issued.
async fn list_contexts(client: &Client, profile_arn: &str, query: &str) -> Answer {
    client
        .send(
            "GET",
            &format!(
                "/api/v1/agent-profiles/{}/contexts?{query}",
                encode_path_segment(profile_arn)
            ),
            None,
        )
        .await
}

/// The `items` of a successful listing.
fn items(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListAgentRecommendations 200, got {answer:?}"
    );
    answer
        .body
        .get("items")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: items is modelled @required, got {answer:?}"))
        .clone()
}

/// A profile holding one recommendation per published pillar: the generation scopes every
/// `Pillar` the enum has, so the listing has several entries to cut a page out of.
async fn profile_with_recommendations(client: &Client, name: &str) -> String {
    let profile_arn = profile_arn_of(client, name, 1).await;
    let started = start_generation(client, &profile_arn, &PILLARS, RECOMMENDATION_TYPE).await;
    assert_eq!(
        started.status, 200,
        "case {name:?}: expected StartAgentRecommendationGeneration 200, got {started:?}"
    );
    tokio::time::sleep(until_complete()).await;
    profile_arn
}

/// Rule 1: each published bound of `maxResults` serves exactly the page it asks for; one
/// past either bound is refused.
#[tokio::test]
async fn max_results_is_held_to_its_published_range() {
    let client = Client::new();
    let profile_arn = profile_with_recommendations(&client, "recommendation-page-size").await;
    let total = recommendations_of(&client, &profile_arn, "every-recommendation")
        .await
        .len();
    assert!(
        total > 1,
        "case \"every-recommendation\": the profile must hold more than one \
         recommendation for a page to be cut, got {total}"
    );

    // Each bound the member publishes, plus a value between them, so the rows differ along
    // the size asked for rather than only at the edges. The listing is complete, so the
    // page a value asks for is known exactly: as many as it asked for, or everything the
    // profile holds when it asked for more than that.
    for max_results in [MAX_RESULTS_MIN, 2, MAX_RESULTS_MAX] {
        let answer =
            list_recommendations(&client, &profile_arn, &format!("?maxResults={max_results}"))
                .await;
        let served = items(&answer, &format!("maxResults={max_results}")).len();
        let ceiling = usize::try_from(max_results).expect("a positive published bound");
        assert_eq!(
            served,
            ceiling.min(total),
            "case maxResults={max_results}: expected {} of the {total} recommendations, \
             got {served}",
            ceiling.min(total)
        );
    }

    for max_results in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let answer =
            list_recommendations(&client, &profile_arn, &format!("?maxResults={max_results}"))
                .await;
        assert_eq!(
            answer.status, 400,
            "case maxResults={max_results}: a value past a published bound must be \
             refused, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case maxResults={max_results}: expected a ValidationException, got {answer:?}"
        );
    }
}

/// Rule 2: a `nextToken` this service did not issue — including a cursor another of its
/// paginated operations issued — is refused rather than served a plausible page.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let client = Client::new();
    let profile_arn = profile_with_recommendations(&client, "recommendation-cursors").await;
    let before = recommendations_of(&client, &profile_arn, "before-the-refusals").await;

    // `ListAgentContexts` over the SAME profile: its cursor carries a context id.
    for index in 0..2 {
        let created = create_context(
            &client,
            &profile_arn,
            json!({
                "title": format!("context {index}"),
                "contextType": "APPLICATION",
                "content": {},
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case \"a-context-cursor\": expected CreateAgentContext 200, got {created:?}"
        );
    }
    let contexts = list_contexts(&client, &profile_arn, "maxResults=1").await;
    let context_cursor = contexts
        .body
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case \"a-context-cursor\": expected a ListAgentContexts cursor in {contexts:?}")
        })
        .to_string();

    // `ListWorkloads`: a cursor of another resource entirely, carried in another shape.
    for index in 0..2 {
        let created = client
            .create(minimal_create(
                &format!("cursor-workload-{index}"),
                &format!("cursor-workload-{index}"),
            ))
            .await;
        assert_eq!(
            created.status, 200,
            "case \"a-workload-cursor\": expected CreateWorkload 200, got {created:?}"
        );
    }
    let workloads = client.list(json!({ "MaxResults": 1 })).await;
    let workload_cursor = workloads
        .body
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case \"a-workload-cursor\": expected a ListWorkloads cursor in {workloads:?}")
        })
        .to_string();

    // Every row satisfies the modelled `NextToken` pattern `^[A-Za-z0-9+/=_-]+$`, so the
    // boundary hands it to the handler: what is under test is the handler's own refusal.
    let rows: Vec<(&str, String)> = vec![
        ("not-a-cursor", "notacursor".to_string()),
        ("hex-of-nothing", "6e756c6c".to_string()),
        ("one-character", "a".to_string()),
        ("an-arn-like-token", "arn-like_token=".to_string()),
        ("a-context-cursor", context_cursor),
        ("a-workload-cursor", workload_cursor),
    ];
    for (case, token) in rows {
        let answer =
            list_recommendations(&client, &profile_arn, &format!("?nextToken={token}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a token this service did not issue must be refused, \
             got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected a ValidationException, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Reason").and_then(Value::as_str),
            Some("CANNOT_PARSE"),
            "case {case:?}: expected reason CANNOT_PARSE, got {answer:?}"
        );
        assert_eq!(
            answer
                .body
                .get("Fields")
                .and_then(Value::as_array)
                .and_then(|fields| fields.first())
                .and_then(|field| field.get("Name"))
                .and_then(Value::as_str),
            Some("NextToken"),
            "case {case:?}: the finding must name the NextToken member, got {answer:?}"
        );
    }

    // The refusals changed nothing: the listing answers exactly what it did before them.
    let after = recommendations_of(&client, &profile_arn, "after-the-refusals").await;
    assert_eq!(
        after, before,
        "case \"after-the-refusals\": a refused page must leave the listing reporting \
         what it did before, got {after:?}"
    );
}
