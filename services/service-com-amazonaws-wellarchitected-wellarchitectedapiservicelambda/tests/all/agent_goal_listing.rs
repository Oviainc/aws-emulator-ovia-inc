// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListAgentGoals`: the page of `GoalSummary` projections an
//! agent profile's optimization goals are reported through.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one request:
//!
//! 1. The list reports the STORED goals: every entry is the identical `GoalSummary`
//!    `CreateAgentGoal` answered and `GetAgentGoal` reports, the entries come back in one
//!    stable ascending id order, and a page that reported them all carries no
//!    `nextToken`.
//! 2. The list is scoped to ONE profile: another profile's goals never appear, a profile
//!    holding none reports an empty `items`, and a well-formed `profileArn` that names no
//!    stored profile reports an empty `items` too — the operation models
//!    `AccessDenied`, `InternalServer`, `Throttling` and `Validation` and NO
//!    `ResourceNotFoundException`, so it has no modelled way to report a missing profile.
//! 3. Paging enumerates every goal EXACTLY ONCE: no page is longer than the `maxResults`
//!    asked for, the ids are strictly ascending across the pages, the union of the pages
//!    is the whole stored set, and only the last page omits `nextToken`.
//! 4. `maxResults` is held to its published `@range(min: 1, max: 1000)`: both bounds are
//!    served, and a value one past either bound is refused `400 ValidationException`.
//! 5. A `nextToken` this service did not issue is refused `400 ValidationException` with
//!    reason `CANNOT_PARSE` naming the member — never an empty page, and never the first
//!    page again. A cursor another paginated operation issued is not a cursor here.
//! 6. Listing is REPORTING: repeating a list (whole or paged) reports the same goals, and
//!    what the list reports changes only when a goal operation changed the stored set.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client lives in `super::workload_harness`; the profile the
//! goals hang off and the goal reader/writer come from `super::agent_goals`.

use serde_json::{json, Value};

use super::agent_goals::{absent_profile_arn, create_goal, get_goal, profile_arn_of, PILLARS};
use super::workload_harness::{encode_path_segment, Answer, Client};

/// The published bounds of `ListAgentGoalsRequest$maxResults`
/// (`@range(min: 1, max: 1000)` —
/// `smithy_query(query="operation", operation="ListAgentGoals")`).
const MAX_RESULTS_MIN: i64 = 1;
/// See [`MAX_RESULTS_MIN`].
const MAX_RESULTS_MAX: i64 = 1_000;

/// `GET /api/v1/agent-profiles/{profileArn}/goals`, with the paging members bound to the
/// query string. The ARN is a non-greedy `@httpLabel`, so it travels percent-encoded in
/// one path segment, exactly as an SDK sends it. `query` is passed as sent, empty for a
/// request that names none.
async fn list_goals(client: &Client, profile_arn: &str, query: &str) -> Answer {
    let path = format!(
        "/api/v1/agent-profiles/{}/goals",
        encode_path_segment(profile_arn)
    );
    let uri = if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    };
    client.send("GET", &uri, None).await
}

/// The `items` a `ListAgentGoals` answer carries.
fn items(answer: &Answer) -> &[Value] {
    answer
        .body
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_else(|| panic!("expected an items list in {answer:?}"))
}

fn id_of(value: &Value) -> String {
    value
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `nextToken` a page carries, or `None` when it is the last page.
fn next_token_of(answer: &Answer) -> Option<String> {
    answer
        .body
        .get("nextToken")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// The rows the listing spans. They differ along what the rule generalises over: how many
/// pillars a goal names (one, several, and the whole published catalog, which is the
/// modelled `Pillars` `@length(1, 5)` maximum), whether the optional `description` is
/// present, and titles of different size including the modelled `SensitiveString`
/// `@length(1, 1000)` bounds — so no two entries of the page are copies of one goal.
fn goal_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "one-pillar-no-description",
            json!({ "pillars": [PILLARS[1]], "title": "Harden the perimeter" }),
        ),
        (
            "two-pillars-with-description",
            json!({
                "pillars": [PILLARS[0], PILLARS[3]],
                "title": "Right-size the fleet",
                "description": "Trade idle capacity for latency headroom",
            }),
        ),
        (
            "every-published-pillar",
            json!({
                "pillars": PILLARS,
                "title": "Review the whole framework",
                "description": "One goal spanning every pillar the model publishes",
            }),
        ),
        (
            "pillars-the-issue-did-not-cite",
            json!({
                "pillars": [PILLARS[4], PILLARS[2]],
                "title": "Rehearse the failover runbook",
            }),
        ),
        (
            "shortest-title",
            json!({ "pillars": [PILLARS[2]], "title": "x" }),
        ),
        (
            "longest-title",
            json!({
                "pillars": [PILLARS[3]],
                "title": "t".repeat(1000),
                "description": "d".repeat(1000),
            }),
        ),
    ]
}

/// Create one goal and answer the `GoalSummary` the create reported.
async fn created_summary(client: &Client, profile_arn: &str, body: Value, case: &str) -> Value {
    let answer = create_goal(client, profile_arn, body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateAgentGoal 200, got {answer:?}"
    );
    answer
        .body
        .get("goal")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a goal member in {answer:?}"))
}

/// Rule 1: every row the profile holds is reported by the list, as the same summary the
/// create answered and `GetAgentGoal` reports, in ascending id order.
#[tokio::test]
async fn every_stored_goal_is_reported_as_the_summary_get_reports() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "goal-listing-host", 1).await;

    let mut created: Vec<(&str, Value)> = Vec::new();
    for (case, body) in goal_rows() {
        let summary = created_summary(&client, &profile_arn, body, case).await;
        let read = get_goal(&client, &profile_arn, &id_of(&summary)).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected GetAgentGoal 200, got {read:?}"
        );
        assert_eq!(
            read.body.get("goal"),
            Some(&summary),
            "case {case:?}: GetAgentGoal must report the created summary, got {read:?}"
        );
        created.push((case, summary));
    }

    let listed = list_goals(&client, &profile_arn, "maxResults=1000").await;
    assert_eq!(
        listed.status, 200,
        "expected ListAgentGoals 200, got {listed:?}"
    );
    let reported = items(&listed);
    assert_eq!(
        reported.len(),
        created.len(),
        "the list must report every stored goal, got {listed:?}"
    );
    for (case, summary) in &created {
        assert!(
            reported.contains(summary),
            "case {case:?}: the list must report the stored summary {summary:?}, \
             got {reported:?}"
        );
    }
    let ids: Vec<String> = reported.iter().map(id_of).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(
        ids, sorted,
        "the list must report goals in one stable id order, got {ids:?}"
    );
    assert!(
        listed.body.get("nextToken").is_none_or(Value::is_null),
        "a page that reported every goal must carry no nextToken, got {listed:?}"
    );
}

/// Rule 2: the list is scoped to ONE profile — a sibling profile's goals, a profile
/// holding none and an ARN no profile carries.
#[tokio::test]
async fn goals_are_listed_per_profile() {
    let client = Client::new();
    let first = profile_arn_of(&client, "goal-listing-first", 1).await;
    let second = profile_arn_of(&client, "goal-listing-second", 2).await;
    let empty = profile_arn_of(&client, "goal-listing-empty", 0).await;

    // A different number of goals under each profile, so a handler that ignored the ARN
    // could not pass by accident.
    let mut expected: Vec<(&str, Vec<String>)> = Vec::new();
    for (case, profile_arn, count) in [("first", &first, 1usize), ("second", &second, 3)] {
        let mut ids = Vec::new();
        for index in 0..count {
            let summary = created_summary(
                &client,
                profile_arn,
                json!({
                    "pillars": [PILLARS[index % PILLARS.len()]],
                    "title": format!("{case} goal {index}"),
                }),
                case,
            )
            .await;
            ids.push(id_of(&summary));
        }
        ids.sort();
        expected.push((case, ids));
    }

    for ((case, ids), profile_arn) in expected.iter().zip([&first, &second]) {
        let answer = list_goals(&client, profile_arn, "").await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected ListAgentGoals 200, got {answer:?}"
        );
        let reported: Vec<String> = items(&answer).iter().map(id_of).collect();
        assert_eq!(
            &reported, ids,
            "case {case:?}: the list must report exactly this profile's goals, \
             got {answer:?}"
        );
    }

    // A profile that holds none, and an ARN that names no stored profile: the operation
    // models no `ResourceNotFoundException`, so both report an empty page rather than a
    // failure, and neither reports another profile's goals.
    for (case, profile_arn) in [
        ("profile-with-no-goals", empty),
        ("profile-that-does-not-exist", absent_profile_arn("absent")),
    ] {
        let answer = list_goals(&client, &profile_arn, "").await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected ListAgentGoals 200, got {answer:?}"
        );
        assert!(
            items(&answer).is_empty(),
            "case {case:?}: expected an empty items list, got {answer:?}"
        );
        assert!(
            answer.body.get("nextToken").is_none_or(Value::is_null),
            "case {case:?}: an empty page carries no nextToken, got {answer:?}"
        );
    }
}

/// Rule 3: paging over a stored set with several page sizes enumerates every goal exactly
/// once, in ascending id order, and only the last page omits `nextToken`.
#[tokio::test]
async fn paging_enumerates_every_goal_exactly_once() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "goal-listing-paging", 1).await;

    let stored = 7;
    let mut all_ids: Vec<String> = Vec::new();
    for index in 0..stored {
        let summary = created_summary(
            &client,
            &profile_arn,
            json!({
                "pillars": [PILLARS[index % PILLARS.len()]],
                "title": format!("paged goal {index}"),
            }),
            "paging",
        )
        .await;
        all_ids.push(id_of(&summary));
    }
    all_ids.sort();

    // A page size below, at and above the stored count, plus the request that names none
    // (the member's `@default(100)`), so the rule is asserted where the page divides the
    // set evenly, unevenly and not at all.
    for page_size in [None, Some(1), Some(2), Some(3), Some(stored), Some(stored + 4)] {
        let case = format!("maxResults={page_size:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = match page_size {
                Some(size) => format!("maxResults={size}"),
                None => String::new(),
            };
            if let Some(token) = &next_token {
                if !query.is_empty() {
                    query.push('&');
                }
                query.push_str(&format!("nextToken={token}"));
            }
            let answer = list_goals(&client, &profile_arn, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected ListAgentGoals 200, got {answer:?}"
            );
            let page: Vec<String> = items(&answer).iter().map(id_of).collect();
            if let Some(size) = page_size {
                assert!(
                    page.len() <= size,
                    "case {case:?}: a page must not be longer than maxResults, got {page:?}"
                );
            }
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= stored + 1,
                "case {case:?}: paging must terminate, got {pages} pages for {stored} goals"
            );
            next_token = next_token_of(&answer);
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, all_ids,
            "case {case:?}: paging must enumerate every stored goal exactly once, in \
             ascending id order"
        );
    }
}

/// Rule 4: `maxResults` at each published bound is served; one past either bound is
/// refused by the boundary as a `400 ValidationException`.
#[tokio::test]
async fn max_results_is_held_to_its_published_range() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "goal-listing-max-results", 1).await;
    for (index, pillar) in PILLARS.iter().enumerate().take(2) {
        created_summary(
            &client,
            &profile_arn,
            json!({
                "pillars": [pillar],
                "title": format!("bounded goal {index}"),
            }),
            "bounds",
        )
        .await;
    }

    for max_results in [MAX_RESULTS_MIN, MAX_RESULTS_MAX] {
        let query = format!("maxResults={max_results}");
        let answer = list_goals(&client, &profile_arn, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {max_results}: a published bound must be served, got {answer:?}"
        );
        let served = items(&answer).len();
        assert!(
            served >= 1 && served <= usize::try_from(max_results).expect("a positive bound"),
            "case {max_results}: expected at most {max_results} of the 2 stored goals, \
             got {served}"
        );
    }

    for max_results in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let query = format!("maxResults={max_results}");
        let answer = list_goals(&client, &profile_arn, &query).await;
        assert_eq!(
            answer.status, 400,
            "case {max_results}: a value past a published bound must be refused, \
             got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {max_results}: expected a ValidationException, got {answer:?}"
        );
    }
}

/// Rule 5: a `nextToken` this service did not issue is a refusal, not a page.
#[tokio::test]
async fn an_unissued_next_token_is_refused() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "goal-listing-tokens", 1).await;
    created_summary(
        &client,
        &profile_arn,
        json!({ "pillars": [PILLARS[0]], "title": "the only goal" }),
        "tokens",
    )
    .await;

    // Each row satisfies the modelled `NextToken` pattern `^[A-Za-z0-9+/=_-]+$`, so the
    // boundary passes it to the handler: what is under test is the handler's own refusal.
    // The last two rows are cursors OTHER paginated operations of this service issue,
    // which are not cursors here.
    let rows: Vec<(&str, String)> = vec![
        ("not-a-cursor", "notacursor".to_string()),
        ("hex-of-nothing", "6e756c6c".to_string()),
        ("one-character", "a".to_string()),
        (
            "another-operations-cursor",
            service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::encode_share_next_token(
                "0123456789abcdef0123456789abcdef",
            ),
        ),
        (
            "the-sibling-lists-cursor",
            service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::encode_agent_context_next_token(
                "00000000-0000-4000-8000-000000000000",
            ),
        ),
    ];
    for (case, token) in rows {
        let answer = list_goals(&client, &profile_arn, &format!("nextToken={token}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: an unissued nextToken must be refused, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected a ValidationException, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Reason").and_then(Value::as_str),
            Some("CANNOT_PARSE"),
            "case {case:?}: an undecodable cursor is CANNOT_PARSE, got {answer:?}"
        );
        let fields: Vec<&str> = answer
            .body
            .get("Fields")
            .and_then(Value::as_array)
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(|field| field.get("Name").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            fields,
            vec!["NextToken"],
            "case {case:?}: the finding must name the member whose value could not be \
             decoded, got {answer:?}"
        );
    }
}

/// Rule 6: the list REPORTS — repeating it, whole or page by page, reports the same goals,
/// and a cursor a page issued stays usable because nothing the read did consumed it.
#[tokio::test]
async fn listing_does_not_change_what_a_later_caller_sees() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "goal-listing-reporting", 1).await;
    for index in 0..4 {
        created_summary(
            &client,
            &profile_arn,
            json!({
                "pillars": [PILLARS[index % PILLARS.len()]],
                "title": format!("repeated goal {index}"),
            }),
            "reporting",
        )
        .await;
    }

    let first = list_goals(&client, &profile_arn, "maxResults=1000").await;
    assert_eq!(
        first.status, 200,
        "expected ListAgentGoals 200, got {first:?}"
    );
    let baseline = items(&first).to_vec();
    assert_eq!(
        baseline.len(),
        4,
        "expected every stored goal in one page, got {first:?}"
    );

    // Pages of 2 over the same set, twice: the cursor the first page issued resumes the
    // same enumeration however often it is replayed.
    for round in 0..2 {
        let page = list_goals(&client, &profile_arn, "maxResults=2").await;
        assert_eq!(
            page.status, 200,
            "round {round}: expected ListAgentGoals 200, got {page:?}"
        );
        assert_eq!(
            items(&page),
            &baseline[..2],
            "round {round}: the first page must report the same goals, got {page:?}"
        );
        let token = next_token_of(&page)
            .unwrap_or_else(|| panic!("round {round}: expected a nextToken in {page:?}"));
        for replay in 0..2 {
            let resumed =
                list_goals(&client, &profile_arn, &format!("maxResults=2&nextToken={token}")).await;
            assert_eq!(
                resumed.status, 200,
                "round {round}/replay {replay}: expected ListAgentGoals 200, got {resumed:?}"
            );
            assert_eq!(
                items(&resumed),
                &baseline[2..],
                "round {round}/replay {replay}: a replayed cursor must report the same \
                 goals, got {resumed:?}"
            );
        }
    }

    let repeated = list_goals(&client, &profile_arn, "maxResults=1000").await;
    assert_eq!(
        items(&repeated),
        baseline.as_slice(),
        "a whole-set list must report the same goals after the paged reads, \
         got {repeated:?}"
    );
}
