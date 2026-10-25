// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for an agent profile's optimization recommendations: the
//! `ListAgentRecommendations` that reports them and the
//! `UpdateAgentRecommendationStatus` that moves one "through the implementation
//! lifecycle".
//!
//! The rules pinned here, each over a TABLE of inputs rather than one request:
//!
//! 1. A recommendation is a RESULT of a generation: a generation whose analysis the clock
//!    has finished has produced one recommendation per pillar its scope named, each
//!    reporting that pillar's published finding, the `RecommendationType` the generation
//!    was started for, the generation's id and the `createdBy` / `createdAt` the service
//!    owns — and a generation still in flight has produced none.
//! 2. The finding belongs to the PILLAR, not to the request: two generations that scope
//!    the same pillar report the same title, description, priority, effort, impact and
//!    `roi` for it, whatever else differs between them.
//! 3. `UpdateAgentRecommendationStatus` records the status it is sent, and the
//!    `RecommendationState` a listing reports is DERIVED from that status: `ACTIVE` is
//!    `OPEN`, and `COMPLETED` / `SUPPRESSED` are `CLOSED`. `updateReason` is the reason of
//!    the MOST RECENT update, so an update that carries none leaves none behind, and
//!    `lastModifiedBy` / `lastModifiedAt` report the update.
//! 4. The `state` and `pillar` filters select on exactly those values, and a filter the
//!    request omits selects everything.
//! 5. A listing is a REPORT: repeating it reports the same recommendations, and it pages
//!    over every one of them exactly once in a stable order.
//! 6. A request the service cannot serve changes nothing: a `recommendationArn` that names
//!    no recommendation is `404 ResourceNotFoundException` carrying the ARN as sent and the
//!    missing resource's type, a `profileArn` that names no agent profile is that same 404
//!    for the profile, a `nextToken` this service did not issue is `400
//!    ValidationException`, and a value the modelled pattern or enum refuses never reaches
//!    the handler.
//! 7. Every finding this emulator publishes sits inside the bounds its modelled shape
//!    declares and names values its enums publish, for every pillar the `Pillar` enum has.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client lives in `super::workload_harness`; the profile the
//! recommendations hang off is minted by the operations in `super::agent_goals`.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

use super::agent_goals::{absent_profile_arn, member, PILLARS};
use super::workload_harness::{encode_path_segment, Answer, Client};

/// Every value the `RecommendationType` enum publishes, in the model's order
/// (`smithy_query(query="shape", shape="RecommendationType")`). Spelled out here so the
/// suite pins the catalog rather than reading it back out of the code under test.
const RECOMMENDATION_TYPES: [&str; 3] = ["RESOURCE", "ARCHITECTURE", "APPLICATION"];

/// Every value the `RecommendationStatus` enum publishes, in the model's order
/// (`smithy_query(query="shape", shape="RecommendationStatus")`).
const RECOMMENDATION_STATUSES: [&str; 3] = ["ACTIVE", "SUPPRESSED", "COMPLETED"];

/// Every value the `Priority` enum publishes (`smithy_query(query="shape",
/// shape="Priority")`).
const PRIORITIES: [&str; 3] = ["HIGH", "MEDIUM", "LOW"];

/// Every value the `Effort` enum publishes (`smithy_query(query="shape", shape="Effort")`).
const EFFORTS: [&str; 3] = ["LARGE", "MEDIUM", "SMALL"];

/// Every value the `ImpactCategory` enum publishes (`smithy_query(query="shape",
/// shape="ImpactCategory")`).
const IMPACTS: [&str; 3] = ["HIGH", "MEDIUM", "LOW"];

/// The modelled bounds on the text of one recommendation, from
/// `smithy_query(query="shape", shape="AgentRecommendationSummary")` and
/// `shape="Roi"`: `$title` `@length(3, 120)`, `$description` `@length(80, 500)`,
/// `Roi$estimate` `@length(2, 40)` and `Roi$detail` `@length(30, 300)`.
const TITLE_BOUNDS: (usize, usize) = (3, 120);
const DESCRIPTION_BOUNDS: (usize, usize) = (80, 500);
const ROI_ESTIMATE_BOUNDS: (usize, usize) = (2, 40);
const ROI_DETAIL_BOUNDS: (usize, usize) = (30, 300);

/// How long after a start the generation it accepted is certainly complete: both spans the
/// clock owes, plus a margin so a slow machine does not decide the assertion.
fn until_complete() -> Duration {
    state::GENERATION_QUEUE_DURATION + state::GENERATION_RUN_DURATION + Duration::from_millis(500)
}

/// `POST /api/v1/agent-profiles/{profileArn}/generations`: the ARN is a non-greedy
/// `@httpLabel`, so it travels percent-encoded in one path segment, exactly as an SDK
/// sends it.
async fn start_generation(
    client: &Client,
    profile_arn: &str,
    pillars: &[&str],
    recommendation_type: &str,
) -> Answer {
    client
        .send(
            "POST",
            &format!(
                "/api/v1/agent-profiles/{}/generations",
                encode_path_segment(profile_arn)
            ),
            Some(json!({
                "types": [recommendation_type],
                "scope": { "pillars": pillars },
            })),
        )
        .await
}

/// `GET /api/v1/agent-profiles/{profileArn}/recommendations`, with `query` carrying the
/// filters and paging members the case exercises (already percent-encoded).
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

/// `PATCH /api/v1/agent-recommendations/{recommendationArn}/status`.
async fn update_status(client: &Client, recommendation_arn: &str, body: Value) -> Answer {
    client
        .send(
            "PATCH",
            &format!(
                "/api/v1/agent-recommendations/{}/status",
                encode_path_segment(recommendation_arn)
            ),
            Some(body),
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

/// One string member of a listed recommendation, or `""` when it carries none.
fn text(item: &Value, name: &str) -> String {
    item.get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Every recommendation of one profile, in the listing's own order, taken in one page.
async fn recommendations_of(client: &Client, profile_arn: &str, case: &str) -> Vec<Value> {
    let answer = list_recommendations(client, profile_arn, "?maxResults=1000").await;
    items(&answer, case)
}

/// A profile whose one generation over `pillars` has completed, with that generation's id.
async fn generated(
    client: &Client,
    name: &str,
    pillars: &[&str],
    recommendation_type: &str,
) -> (String, String) {
    let profile_arn = super::agent_goals::profile_arn_of(client, name, 1).await;
    let started = start_generation(client, &profile_arn, pillars, recommendation_type).await;
    assert_eq!(
        started.status, 200,
        "case {name:?}: expected StartAgentRecommendationGeneration 200, got {started:?}"
    );
    let generation_id = member(&started, "id").to_string();
    tokio::time::sleep(until_complete()).await;
    (profile_arn, generation_id)
}

/// One row of the table the generation tests span: which pillars a generation scopes and
/// which kind of recommendation it was started for.
struct Row {
    case: &'static str,
    pillars: Vec<&'static str>,
    recommendation_type: &'static str,
}

/// The rows: a scope of one pillar, of two, of three and of every published pillar (the
/// modelled `Scope$pillars` `@length(1, 5)` bounds among them), each paired with a
/// different `RecommendationType`, and one scoping a pillar no other row does.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one-pillar",
            pillars: vec!["SECURITY"],
            recommendation_type: RECOMMENDATION_TYPES[0],
        },
        Row {
            case: "two-pillars",
            pillars: vec!["COST_OPTIMIZATION", "RESILIENCE"],
            recommendation_type: RECOMMENDATION_TYPES[1],
        },
        Row {
            case: "three-pillars",
            pillars: vec!["PERFORMANCE", "SECURITY", "RESILIENCE"],
            recommendation_type: RECOMMENDATION_TYPES[2],
        },
        Row {
            case: "a-pillar-no-other-row-scopes",
            pillars: vec!["OPERATIONAL_EXCELLENCE"],
            recommendation_type: RECOMMENDATION_TYPES[0],
        },
        Row {
            case: "every-published-pillar",
            pillars: PILLARS.to_vec(),
            recommendation_type: RECOMMENDATION_TYPES[1],
        },
    ]
}

/// Rule 1 and 2: a completed generation produces one recommendation per pillar it scoped,
/// each reporting that pillar's finding, and the finding is the same wherever that pillar
/// is scoped.
#[tokio::test]
async fn a_completed_generation_produces_one_recommendation_per_scoped_pillar() {
    let client = Client::new();

    // Each row gets its own profile, so a listing reports exactly what that row's
    // generation produced; every generation is started before the one sleep.
    let mut started: Vec<(Row, String, String)> = Vec::new();
    for row in rows() {
        let profile_arn =
            super::agent_goals::profile_arn_of(&client, &format!("rec-{}", row.case), 1).await;

        // Rule 1's other half: nothing has been analysed yet, so there is nothing to list.
        let before = recommendations_of(&client, &profile_arn, row.case).await;
        assert!(
            before.is_empty(),
            "case {:?}: a profile whose generation has not completed has no recommendations, \
             got {before:?}",
            row.case
        );

        let answer =
            start_generation(&client, &profile_arn, &row.pillars, row.recommendation_type).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected StartAgentRecommendationGeneration 200, got {answer:?}",
            row.case
        );
        started.push((row, profile_arn, member(&answer, "id").to_string()));
    }

    tokio::time::sleep(until_complete()).await;

    // pillar -> the finding reported for it, so rule 2 is checked ACROSS the rows.
    let mut findings: Vec<(String, Value)> = Vec::new();
    for (row, profile_arn, generation_id) in &started {
        let listed = recommendations_of(&client, profile_arn, row.case).await;
        assert_eq!(
            listed.len(),
            row.pillars.len(),
            "case {:?}: a completed generation produces one recommendation per scoped \
             pillar ({:?}), got {listed:?}",
            row.case,
            row.pillars
        );

        let mut pillars: Vec<String> = listed.iter().map(|item| text(item, "pillar")).collect();
        pillars.sort();
        let mut scoped: Vec<String> = row.pillars.iter().map(|p| (*p).to_string()).collect();
        scoped.sort();
        assert_eq!(
            pillars, scoped,
            "case {:?}: the recommendations must address exactly the scoped pillars, got \
             {listed:?}",
            row.case
        );

        for item in &listed {
            let case = format!("{}/{}", row.case, text(item, "pillar"));
            let arn = text(item, "recommendationArn");
            assert!(
                arn.starts_with("arn:aws:wellarchitected:")
                    && arn.contains(":agent-recommendation/"),
                "case {case:?}: recommendationArn must be a RecommendationArn, got {arn:?}"
            );
            assert_eq!(
                text(item, "profileArn"),
                *profile_arn,
                "case {case:?}: a recommendation belongs to the profile that generated it, \
                 got {item:?}"
            );
            assert_eq!(
                text(item, "generationId"),
                *generation_id,
                "case {case:?}: generationId must name the generation that produced it, got \
                 {item:?}"
            );
            assert_eq!(
                text(item, "type"),
                row.recommendation_type,
                "case {case:?}: type must be the kind the generation was started for, got \
                 {item:?}"
            );
            assert_eq!(
                text(item, "status"),
                "ACTIVE",
                "case {case:?}: a recommendation nobody has acted on is ACTIVE, got {item:?}"
            );
            assert_eq!(
                text(item, "state"),
                "OPEN",
                "case {case:?}: an ACTIVE recommendation is OPEN, got {item:?}"
            );
            assert_eq!(
                text(item, "createdBy"),
                engine::emulator_account_id(),
                "case {case:?}: the analysis is the service's own work, got {item:?}"
            );
            assert!(
                item.get("updateReason").is_none() || item["updateReason"].is_null(),
                "case {case:?}: a recommendation nobody has updated has no updateReason, got \
                 {item:?}"
            );
            assert!(
                item.get("lastModifiedAt").is_none() || item["lastModifiedAt"].is_null(),
                "case {case:?}: a recommendation nobody has updated has no lastModifiedAt, \
                 got {item:?}"
            );
            assert_finding_within_bounds(item, &case);

            // Rule 2: the finding is the pillar's, so the same pillar in another row
            // reports it identically.
            let pillar = text(item, "pillar");
            let finding = json!({
                "title": item.get("title"),
                "description": item.get("description"),
                "priority": item.get("priority"),
                "effort": item.get("effort"),
                "impact": item.get("impact"),
                "roi": item.get("roi"),
                "awsServices": item.get("awsServices"),
            });
            match findings.iter().find(|(seen, _)| *seen == pillar) {
                None => findings.push((pillar, finding)),
                Some((_, expected)) => assert_eq!(
                    &finding, expected,
                    "case {case:?}: the finding for a pillar must not depend on the request \
                     that asked for it, got {item:?}"
                ),
            }
        }
    }
    assert_eq!(
        findings.len(),
        PILLARS.len(),
        "the rows must between them scope every published pillar, got {findings:?}"
    );
}

/// Every text member of a reported recommendation against the bounds its modelled shape
/// declares, and every enum-typed member against the values its enum publishes.
fn assert_finding_within_bounds(item: &Value, case: &str) {
    for (member, value, (min, max)) in [
        ("title", text(item, "title"), TITLE_BOUNDS),
        ("description", text(item, "description"), DESCRIPTION_BOUNDS),
    ] {
        let length = value.chars().count();
        assert!(
            (min..=max).contains(&length),
            "case {case:?}: {member} must be {min}..={max} characters, got {length} in \
             {value:?}"
        );
    }
    let roi = item.get("roi").cloned().unwrap_or(Value::Null);
    for (member, bounds) in [
        ("estimate", ROI_ESTIMATE_BOUNDS),
        ("detail", ROI_DETAIL_BOUNDS),
    ] {
        let value = text(&roi, member);
        let length = value.chars().count();
        assert!(
            (bounds.0..=bounds.1).contains(&length),
            "case {case:?}: roi.{member} must be {}..={} characters, got {length} in {value:?}",
            bounds.0,
            bounds.1
        );
    }
    for (member, published) in [
        ("pillar", PILLARS.to_vec()),
        ("priority", PRIORITIES.to_vec()),
        ("effort", EFFORTS.to_vec()),
        ("impact", IMPACTS.to_vec()),
        ("type", RECOMMENDATION_TYPES.to_vec()),
        ("status", RECOMMENDATION_STATUSES.to_vec()),
        ("state", vec!["OPEN", "CLOSED"]),
    ] {
        let value = text(item, member);
        assert!(
            published.contains(&value.as_str()),
            "case {case:?}: {member} must be one of {published:?}, got {value:?}"
        );
    }
    let services = item
        .get("awsServices")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        !services.is_empty(),
        "case {case:?}: a finding names the services it applies to, got {item:?}"
    );
}

/// Rule 3 and 4: an update records the status it is sent, the state is derived from it, the
/// reason is the latest update's, and the filters select on exactly those values.
#[tokio::test]
async fn an_update_records_the_status_and_the_listing_derives_the_state() {
    let client = Client::new();
    let (profile_arn, _) = generated(&client, "rec-status", &PILLARS, RECOMMENDATION_TYPES[0]).await;
    let listed = recommendations_of(&client, &profile_arn, "status-setup").await;
    assert_eq!(
        listed.len(),
        PILLARS.len(),
        "the setup generation must have produced one recommendation per pillar, got {listed:?}"
    );

    // One row per published status, with the reason present and absent, so the table spans
    // the status catalog and both forms of the optional member.
    let plan: Vec<(&str, Option<&str>, &str)> = vec![
        ("ACTIVE", Some("still being worked on"), "OPEN"),
        ("COMPLETED", Some("implemented in change 4711"), "CLOSED"),
        ("SUPPRESSED", Some("accepted risk for this quarter"), "CLOSED"),
        ("COMPLETED", None, "CLOSED"),
        ("SUPPRESSED", None, "CLOSED"),
    ];
    assert_eq!(
        plan.len(),
        listed.len(),
        "the plan must cover every recommendation the setup produced"
    );

    let mut expected: Vec<(String, &str, Option<&str>, &str)> = Vec::new();
    for (item, (status, reason, derived_state)) in listed.iter().zip(&plan) {
        let arn = text(item, "recommendationArn");
        let case = format!("{}-{status}-{}", text(item, "pillar"), reason.is_some());
        let mut body = json!({ "status": status });
        if let Some(reason) = reason {
            body["updateReason"] = json!(reason);
        }
        let answer = update_status(&client, &arn, body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected UpdateAgentRecommendationStatus 200, got {answer:?}"
        );
        expected.push((arn, status, *reason, derived_state));
    }

    let after = recommendations_of(&client, &profile_arn, "status-applied").await;
    for (arn, status, reason, derived_state) in &expected {
        let item = after
            .iter()
            .find(|item| text(item, "recommendationArn") == *arn)
            .unwrap_or_else(|| panic!("case {arn:?}: the updated recommendation must still be listed, got {after:?}"));
        let case = format!("{arn}-{status}");
        assert_eq!(
            text(item, "status"),
            *status,
            "case {case:?}: the listing must report the recorded status, got {item:?}"
        );
        assert_eq!(
            text(item, "state"),
            *derived_state,
            "case {case:?}: the state is derived from the status, got {item:?}"
        );
        match reason {
            Some(reason) => assert_eq!(
                text(item, "updateReason"),
                *reason,
                "case {case:?}: updateReason is the reason of the latest update, got {item:?}"
            ),
            None => assert!(
                item.get("updateReason").is_none() || item["updateReason"].is_null(),
                "case {case:?}: an update carrying no reason leaves none behind, got {item:?}"
            ),
        }
        assert_eq!(
            text(item, "lastModifiedBy"),
            engine::emulator_account_id(),
            "case {case:?}: the update is attributed to this emulator's account, got {item:?}"
        );
        assert!(
            item.get("lastModifiedAt").is_some_and(|at| !at.is_null()),
            "case {case:?}: an updated recommendation reports when it was modified, got \
             {item:?}"
        );
        assert_finding_within_bounds(item, &case);
    }

    // Rule 4: the state filter selects exactly the recommendations whose derived state
    // equals it, and the two filters together select the intersection.
    for state_filter in ["OPEN", "CLOSED"] {
        let answer =
            list_recommendations(&client, &profile_arn, &format!("?state={state_filter}")).await;
        let filtered = items(&answer, state_filter);
        let wanted: Vec<&(String, &str, Option<&str>, &str)> = expected
            .iter()
            .filter(|(_, _, _, derived)| *derived == state_filter)
            .collect();
        assert_eq!(
            filtered.len(),
            wanted.len(),
            "case {state_filter:?}: the state filter must select every recommendation in that \
             state and no other, got {filtered:?}"
        );
        for item in &filtered {
            assert_eq!(
                text(item, "state"),
                state_filter,
                "case {state_filter:?}: a filtered listing reports only that state, got {item:?}"
            );
        }
    }
    for pillar in PILLARS {
        let answer = list_recommendations(&client, &profile_arn, &format!("?pillar={pillar}")).await;
        let filtered = items(&answer, pillar);
        assert_eq!(
            filtered.len(),
            1,
            "case {pillar:?}: the setup produced one recommendation per pillar, got {filtered:?}"
        );
        assert_eq!(
            text(&filtered[0], "pillar"),
            pillar,
            "case {pillar:?}: a pillar-filtered listing reports only that pillar, got {filtered:?}"
        );

        // Both filters at once: the intersection, which is empty for the state this
        // pillar's recommendation is not in.
        let state_of = text(&filtered[0], "state");
        for state_filter in ["OPEN", "CLOSED"] {
            let answer = list_recommendations(
                &client,
                &profile_arn,
                &format!("?pillar={pillar}&state={state_filter}"),
            )
            .await;
            let both = items(&answer, pillar);
            let wanted = usize::from(state_filter == state_of);
            assert_eq!(
                both.len(),
                wanted,
                "case {pillar:?}/{state_filter:?}: the filters select the intersection, got \
                 {both:?}"
            );
        }
    }

    // Rule 3's idempotence: re-sending a status a recommendation already carries leaves it
    // where it is.
    let (arn, status, reason, derived_state) = &expected[1];
    let mut body = json!({ "status": status });
    if let Some(reason) = reason {
        body["updateReason"] = json!(reason);
    }
    let repeat = update_status(&client, arn, body).await;
    assert_eq!(
        repeat.status, 200,
        "case {arn:?}: repeating an update must be served, got {repeat:?}"
    );
    let after = recommendations_of(&client, &profile_arn, "status-repeat").await;
    let item = after
        .iter()
        .find(|item| text(item, "recommendationArn") == *arn)
        .unwrap_or_else(|| panic!("case {arn:?}: the recommendation must still be listed"));
    assert_eq!(
        (text(item, "status"), text(item, "state")),
        ((*status).to_string(), (*derived_state).to_string()),
        "case {arn:?}: repeating an update must not move the recommendation, got {item:?}"
    );
}

/// Rule 5: the listing is a report, and it pages over every recommendation exactly once.
#[tokio::test]
async fn the_listing_reports_the_same_recommendations_and_pages_over_each_one_once() {
    let client = Client::new();
    let (profile_arn, _) = generated(&client, "rec-paging", &PILLARS, RECOMMENDATION_TYPES[2]).await;

    let first = recommendations_of(&client, &profile_arn, "paging-first").await;
    let again = recommendations_of(&client, &profile_arn, "paging-again").await;
    assert_eq!(
        first, again,
        "case \"repeat\": a listing must report the same recommendations to a later caller"
    );
    assert_eq!(
        first.len(),
        PILLARS.len(),
        "case \"repeat\": the setup produced one recommendation per pillar, got {first:?}"
    );

    // One at a time: the pages together are the whole listing, in the same order, with no
    // recommendation served twice.
    let mut paged: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..=first.len() {
        let query = match &token {
            None => "?maxResults=1".to_string(),
            Some(token) => format!("?maxResults=1&nextToken={token}"),
        };
        let answer = list_recommendations(&client, &profile_arn, &query).await;
        let items = items(&answer, &format!("page-{page}"));
        assert!(
            items.len() <= 1,
            "case \"page-{page}\": maxResults=1 serves at most one item, got {items:?}"
        );
        paged.extend(items);
        token = answer
            .body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert!(
        token.is_none(),
        "case \"paging\": the pages must run out after every recommendation was served, \
         {token:?} was still offered"
    );
    assert_eq!(
        paged, first,
        "case \"paging\": paging must enumerate the whole listing exactly once, in order"
    );
}

/// Rule 6: a request the service cannot serve is refused, and the refusal names what was
/// looked for.
#[tokio::test]
async fn a_request_the_service_cannot_serve_is_refused() {
    let client = Client::new();
    let (profile_arn, _) =
        generated(&client, "rec-refusals", &["SECURITY"], RECOMMENDATION_TYPES[0]).await;
    let listed = recommendations_of(&client, &profile_arn, "refusal-setup").await;
    let arn = text(&listed[0], "recommendationArn");

    // A well-formed `AgentRecommendationArn` no recommendation carries.
    let absent_arn = format!(
        "arn:aws:wellarchitected:us-west-2:{}:agent-recommendation/abcdefab-0000-4a00-8b00-00000000000c",
        engine::emulator_account_id()
    );
    let answer = update_status(&client, &absent_arn, json!({ "status": "COMPLETED" })).await;
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "case \"absent recommendation\": expected a 404 ResourceNotFoundException, got {answer:?}"
    );
    assert_eq!(
        member(&answer, "ResourceId"),
        absent_arn,
        "case \"absent recommendation\": the 404 names the ARN as sent, got {answer:?}"
    );
    assert_eq!(
        member(&answer, "ResourceType"),
        state::AGENT_RECOMMENDATION_RESOURCE_TYPE,
        "case \"absent recommendation\": the 404 names the missing resource's type, got \
         {answer:?}"
    );

    // A `profileArn` that names no agent profile: this listing models the 404 for it.
    let absent_profile = absent_profile_arn("absent-recommendation-host");
    let answer = list_recommendations(&client, &absent_profile, "").await;
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "case \"absent profile\": expected a 404 ResourceNotFoundException, got {answer:?}"
    );
    assert_eq!(
        member(&answer, "ResourceId"),
        absent_profile,
        "case \"absent profile\": the 404 names the ARN as sent, got {answer:?}"
    );

    // Values the boundary refuses before any lookup: an ARN the modelled pattern rejects,
    // a status outside the published enum, and a page token this service did not issue.
    let refusals: Vec<(&str, Answer)> = vec![
        (
            "a recommendation ARN the pattern refuses",
            update_status(
                &client,
                &format!(
                    "arn:aws:wellarchitected:us-west-2:{}:agent-recommendation/not-a-uuid",
                    engine::emulator_account_id()
                ),
                json!({ "status": "COMPLETED" }),
            )
            .await,
        ),
        (
            "a status outside the published enum",
            update_status(&client, &arn, json!({ "status": "DISMISSED" })).await,
        ),
        (
            "a next token this service did not issue",
            list_recommendations(&client, &profile_arn, "?nextToken=not-a-cursor").await,
        ),
        (
            "a state filter outside the published enum",
            list_recommendations(&client, &profile_arn, "?state=HALF_OPEN").await,
        ),
    ];
    for (case, answer) in refusals {
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException, got {answer:?}"
        );
    }

    // None of those refusals changed the recommendation they named.
    let after = recommendations_of(&client, &profile_arn, "refusal-after").await;
    assert_eq!(
        after, listed,
        "case \"refusals\": a refused request must leave the listing as it was"
    );
}

/// Rule 7: the content this emulator publishes covers every pillar the `Pillar` enum names
/// and sits inside the bounds the modelled shapes declare.
#[test]
fn the_published_finding_of_every_pillar_is_within_the_modelled_bounds() {
    let catalog = state::RECOMMENDATION_CATALOG;
    let mut covered: Vec<&str> = catalog.iter().map(|content| content.pillar).collect();
    covered.sort_unstable();
    let mut published: Vec<&str> = PILLARS.to_vec();
    published.sort_unstable();
    assert_eq!(
        covered, published,
        "the catalog must publish exactly one finding per pillar the Pillar enum names"
    );

    for content in catalog {
        let case = content.pillar;
        for (member, value, (min, max)) in [
            ("title", content.title, TITLE_BOUNDS),
            ("description", content.description, DESCRIPTION_BOUNDS),
            ("roi.estimate", content.roi_estimate, ROI_ESTIMATE_BOUNDS),
            ("roi.detail", content.roi_detail, ROI_DETAIL_BOUNDS),
        ] {
            let length = value.chars().count();
            assert!(
                (min..=max).contains(&length),
                "case {case:?}: {member} must be {min}..={max} characters, got {length} in \
                 {value:?}"
            );
        }
        for (member, value, published) in [
            ("priority", content.priority, PRIORITIES.to_vec()),
            ("effort", content.effort, EFFORTS.to_vec()),
            ("impact", content.impact, IMPACTS.to_vec()),
        ] {
            assert!(
                published.contains(&value),
                "case {case:?}: {member} must be one of {published:?}, got {value:?}"
            );
        }
        assert!(
            !content.aws_services.is_empty()
                && content
                    .aws_services
                    .iter()
                    .all(|service| !service.trim().is_empty()),
            "case {case:?}: a finding names the services it applies to, got {:?}",
            content.aws_services
        );
    }
}
