// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `GetAgentRecommendation`: the single read that "retrieves
//! detailed information about a specific optimization recommendation, including its impact
//! analysis, content, and implementation guidance".
//!
//! The rules pinned here, each over a TABLE of inputs rather than one request:
//!
//! 1. A read reports the recommendation the ARN resolves to: every member a listing of it
//!    carries is identical in the read, and the read adds the DETAIL members — what was
//!    detected, the impact, the insights, the remediation and the sources. The detail
//!    belongs to the recommendation's PILLAR, so two generations that scope the same pillar
//!    read back the same detail whatever else differs between them.
//! 2. A read is a REPORT: repeating it answers the same recommendation, and it reports the
//!    status `UpdateAgentRecommendationStatus` last moved the recommendation to, with the
//!    `RecommendationState` derived from that status and the reason of the most recent
//!    update.
//! 3. `remediationType` is a filter: a read that names one reports exactly the published
//!    remediations of that type — none of them when the finding publishes none — and a read
//!    that names none reports every one of them.
//! 4. `goals` are the profile's own optimization goals that target this recommendation's
//!    pillar, and only those: a profile with no such goal reports none.
//! 5. A request the service cannot serve changes nothing: a `recommendationArn` that names
//!    no recommendation is `404 ResourceNotFoundException` carrying the ARN as sent and the
//!    missing resource's type, and a `remediationType` or ARN the model refuses is `400
//!    ValidationException`.
//! 6. Every detail entry this emulator publishes sits inside the bounds its modelled shape
//!    declares, its lists inside their own bounds, and its enum-typed members name values
//!    those enums publish — for every pillar the `Pillar` enum has.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client lives in `super::workload_harness`; the profile, its goals
//! and the generation whose completion produces the recommendations come from the suites
//! beside this one.

use serde_json::{json, Value};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

use super::agent_goals::{absent_profile_arn, create_goal, member, PILLARS};
use super::agent_recommendations::{
    recommendations_of, start_generation, text, until_complete, update_status,
    RECOMMENDATION_TYPES,
};
use super::workload_harness::{encode_path_segment, Answer, Client};

/// Every value the `RemediationType` enum publishes, in the model's order
/// (`smithy_query(query="shape", shape="RemediationType")`). Spelled out here so the suite
/// pins the catalog rather than reading it back out of the code under test.
const REMEDIATION_TYPES: [&str; 6] = [
    "AUTO_REMEDIATION",
    "CONSOLE",
    "CLI",
    "SDK",
    "IAC",
    "MCP",
];

/// Every value the `RecommendationStatus` enum publishes, in the model's order
/// (`smithy_query(query="shape", shape="RecommendationStatus")`), paired with the
/// `RecommendationState` the model's OPEN/CLOSED reading of that lifecycle puts it in.
const STATUS_STATES: [(&str, &str); 3] = [
    ("ACTIVE", "OPEN"),
    ("SUPPRESSED", "CLOSED"),
    ("COMPLETED", "CLOSED"),
];

/// Every value the `RiskRating` enum publishes (`smithy_query(query="shape",
/// shape="RiskRating")`).
const RISK_RATINGS: [&str; 3] = ["LOW", "MEDIUM", "HIGH"];

/// Every value the `ImpactCategory` enum publishes (`smithy_query(query="shape",
/// shape="ImpactCategory")`).
const IMPACT_CATEGORIES: [&str; 3] = ["HIGH", "MEDIUM", "LOW"];

/// The `RecommendationSource` a recommendation this service produced names: the agent whose
/// generation produced it (`smithy_query(query="shape", shape="RecommendationSource")`).
const AGENT_SOURCE: &str = "WELL_ARCHITECTED_AGENT";

/// `GET /api/v1/agent-recommendations/{recommendationArn}`, with `query` carrying the
/// filter the case exercises. The ARN is a non-greedy `@httpLabel`, so it travels
/// percent-encoded in one path segment, exactly as an SDK sends it.
async fn get_recommendation(client: &Client, recommendation_arn: &str, query: &str) -> Answer {
    client
        .send(
            "GET",
            &format!(
                "/api/v1/agent-recommendations/{}{query}",
                encode_path_segment(recommendation_arn)
            ),
            None,
        )
        .await
}

/// The body of a successful read.
fn detail(answer: &Answer, case: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetAgentRecommendation 200, got {answer:?}"
    );
    answer.body.clone()
}

/// One string list of a read, or an empty list when it carries none.
fn strings(body: &Value, name: &str) -> Vec<String> {
    body.get(name)
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// One structure list of a read, or an empty list when it carries none.
fn rows(body: &Value, name: &str) -> Vec<Value> {
    body.get(name)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// A profile whose one generation over `pillars` has completed, with the recommendations it
/// produced in the listing's order.
async fn generated(
    client: &Client,
    name: &str,
    pillars: &[&str],
    recommendation_type: &str,
) -> (String, Vec<Value>) {
    let profile_arn = super::agent_goals::profile_arn_of(client, name, 1).await;
    let started = start_generation(client, &profile_arn, pillars, recommendation_type).await;
    assert_eq!(
        started.status, 200,
        "case {name:?}: expected StartAgentRecommendationGeneration 200, got {started:?}"
    );
    tokio::time::sleep(until_complete()).await;
    let listed = recommendations_of(client, &profile_arn, name).await;
    assert_eq!(
        listed.len(),
        pillars.len(),
        "case {name:?}: the setup produced one recommendation per scoped pillar, got {listed:?}"
    );
    (profile_arn, listed)
}

/// Rule 1: a read reports what the listing reports, plus the published detail of the
/// recommendation's pillar — and the detail of a pillar is the same wherever it is scoped.
#[tokio::test]
async fn a_read_reports_the_listed_recommendation_and_the_detail_of_its_pillar() {
    let client = Client::new();

    // Two generations, so the rule is checked over a SET of inputs that differ in the
    // pillars they scope and in the `RecommendationType` they were started for: one over
    // every published pillar, one over two of them.
    let (_, every_pillar) =
        generated(&client, "read-all-pillars", &PILLARS, RECOMMENDATION_TYPES[0]).await;
    let (_, two_pillars) = generated(
        &client,
        "read-two-pillars",
        &[PILLARS[1], PILLARS[3]],
        RECOMMENDATION_TYPES[2],
    )
    .await;

    // pillar -> the detail read back for it, so the "the finding belongs to the pillar"
    // half of the rule is checked ACROSS the two generations.
    let mut by_pillar: Vec<(String, Value)> = Vec::new();

    for listed in [&every_pillar, &two_pillars] {
        for item in listed {
            let pillar = text(item, "pillar");
            let arn = text(item, "recommendationArn");
            let case = format!("{pillar}/{}", text(item, "type"));
            let body = detail(&get_recommendation(&client, &arn, "").await, &case);

            // Every member the listing carries, reported identically by the read: the two
            // readings of one stored recommendation cannot disagree.
            for name in [
                "recommendationArn",
                "profileArn",
                "generationId",
                "title",
                "description",
                "type",
                "pillar",
                "priority",
                "effort",
                "status",
                "state",
                "impact",
                "createdBy",
                "createdAt",
                "roi",
                "awsServices",
            ] {
                assert_eq!(
                    body.get(name),
                    item.get(name),
                    "case {case:?}: the read must report the listed {name}, got {:?} for a \
                     listed {:?}",
                    body.get(name),
                    item.get(name)
                );
            }

            // The detail members the read ADDS, against the finding this emulator publishes
            // for that pillar.
            let content = state::recommendation_content(&pillar)
                .unwrap_or_else(|| panic!("case {case:?}: no published finding for {pillar:?}"));
            assert_eq!(
                strings(&body, "highlights"),
                content.highlights.to_vec(),
                "case {case:?}: highlights must be the pillar's published highlights, got \
                 {body:?}"
            );
            assert_eq!(
                strings(&body, "impactDetails"),
                content.impact_details.to_vec(),
                "case {case:?}: impactDetails must be the pillar's published impact, got \
                 {body:?}"
            );
            let insights = rows(&body, "insights");
            assert_eq!(
                insights.len(),
                content.insights.len(),
                "case {case:?}: insights must be the pillar's published insights, got {body:?}"
            );
            for (got, want) in insights.iter().zip(content.insights) {
                assert_eq!(
                    text(got, "usagePattern"),
                    want.usage_pattern,
                    "case {case:?}: an insight reports the pattern the analysis read, got \
                     {got:?}"
                );
                assert_eq!(
                    text(got, "signalsDetected"),
                    want.signals_detected,
                    "case {case:?}: an insight reports the signals it was read from, got \
                     {got:?}"
                );
            }
            let summary = body
                .get("remediationSummary")
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                text(&summary, "recommendation"),
                content.remediation_recommendation,
                "case {case:?}: remediationSummary must be the pillar's published fix, got \
                 {body:?}"
            );
            assert_eq!(
                strings(&summary, "steps"),
                content.remediation_steps.to_vec(),
                "case {case:?}: the summary's steps must be the published ones, got {body:?}"
            );
            assert_eq!(
                strings(&body, "sources"),
                vec![AGENT_SOURCE.to_string()],
                "case {case:?}: a recommendation the agent's generation produced names that \
                 source, got {body:?}"
            );
            // The optional analysis of other pillars, reported exactly when the finding
            // publishes it.
            assert_eq!(
                rows(&body, "crossPillarBenefits").len(),
                content.cross_pillar_benefits.len(),
                "case {case:?}: crossPillarBenefits must be the published ones, got {body:?}"
            );
            assert_eq!(
                rows(&body, "tradeOffs").len(),
                content.trade_offs.len(),
                "case {case:?}: tradeOffs must be the published ones, got {body:?}"
            );
            // The members that describe a customer's own estate, which this emulator
            // analyses none of: all three are modelled optional and reported as absent.
            for name in ["numberOfResources", "businessUnits", "applications", "tags"] {
                assert!(
                    body.get(name).is_none_or(Value::is_null),
                    "case {case:?}: {name} describes what this emulator does not analyse, so \
                     it must be absent, got {body:?}"
                );
            }

            by_pillar.push((pillar, body));
        }
    }

    // The same pillar scoped by two different generations reads back the same finding: the
    // content belongs to the pillar, not to the request that asked for it.
    for (pillar, body) in &by_pillar {
        for (other_pillar, other) in &by_pillar {
            if pillar != other_pillar {
                continue;
            }
            for name in [
                "title",
                "description",
                "priority",
                "effort",
                "impact",
                "roi",
                "highlights",
                "impactDetails",
                "insights",
                "remediationSummary",
                "crossPillarBenefits",
                "tradeOffs",
            ] {
                assert_eq!(
                    body.get(name),
                    other.get(name),
                    "case {pillar:?}: the published finding of a pillar must not depend on the \
                     generation that scoped it, {name} read back as {:?} and {:?}",
                    body.get(name),
                    other.get(name)
                );
            }
        }
    }
}

/// Rule 2: a read reports the status a later update moved the recommendation to, and
/// repeating the read reports the same thing.
#[tokio::test]
async fn a_read_reports_the_status_and_reason_of_the_most_recent_update() {
    let client = Client::new();
    let (_, listed) = generated(
        &client,
        "read-status",
        &[PILLARS[0], PILLARS[1], PILLARS[2]],
        RECOMMENDATION_TYPES[1],
    )
    .await;

    // One row per published `RecommendationStatus`, each paired with a different shape of
    // `updateReason`: a reason, no reason at all, and a reason on a status that closes the
    // recommendation.
    let rows: Vec<(&str, &str, Option<&str>)> = vec![
        (
            "status-with-reason",
            STATUS_STATES[2].0,
            Some("implemented in the spring change window"),
        ),
        ("status-without-reason", STATUS_STATES[1].0, None),
        ("status-back-to-active", STATUS_STATES[0].0, Some("reopened")),
    ];

    for ((case, status, reason), item) in rows.iter().zip(&listed) {
        let arn = text(item, "recommendationArn");

        // Before the update: the recommendation nobody has acted on.
        let before = detail(&get_recommendation(&client, &arn, "").await, case);
        assert_eq!(
            text(&before, "status"),
            STATUS_STATES[0].0,
            "case {case:?}: a recommendation nobody has acted on is ACTIVE, got {before:?}"
        );
        assert!(
            before.get("lastModifiedBy").is_none_or(Value::is_null)
                && before.get("lastModifiedAt").is_none_or(Value::is_null),
            "case {case:?}: a recommendation nobody has updated reports no last \
             modification, got {before:?}"
        );

        let body = match reason {
            Some(reason) => json!({ "status": status, "updateReason": reason }),
            None => json!({ "status": status }),
        };
        let updated = update_status(&client, &arn, body).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected UpdateAgentRecommendationStatus 200, got {updated:?}"
        );

        let after = detail(&get_recommendation(&client, &arn, "").await, case);
        let expected_state = STATUS_STATES
            .iter()
            .find(|(published, _)| published == status)
            .map(|(_, state)| *state)
            .unwrap_or_else(|| {
                panic!("case {case:?}: {status:?} is not a published RecommendationStatus")
            });
        assert_eq!(
            text(&after, "status"),
            *status,
            "case {case:?}: the read must report the status the update recorded, got {after:?}"
        );
        assert_eq!(
            text(&after, "state"),
            expected_state,
            "case {case:?}: the state is derived from the status, got {after:?}"
        );
        match reason {
            Some(reason) => assert_eq!(
                text(&after, "updateReason"),
                *reason,
                "case {case:?}: the read must report the reason of the most recent update, \
                 got {after:?}"
            ),
            None => assert!(
                after.get("updateReason").is_none_or(Value::is_null),
                "case {case:?}: an update that carries no reason leaves none behind, got \
                 {after:?}"
            ),
        }
        assert_eq!(
            text(&after, "lastModifiedBy"),
            engine::emulator_account_id(),
            "case {case:?}: the update is attributed to the account that made it, got {after:?}"
        );
        assert!(
            after.get("lastModifiedAt").is_some_and(|at| !at.is_null()),
            "case {case:?}: an updated recommendation reports when it was last modified, got \
             {after:?}"
        );

        // A read is a REPORT: it answers the same recommendation however often it runs.
        let again = detail(&get_recommendation(&client, &arn, "").await, case);
        assert_eq!(
            again, after,
            "case {case:?}: repeating a read must report the same recommendation"
        );
    }
}

/// Rule 3: `remediationType` selects the published remediations of exactly that type.
#[tokio::test]
async fn the_remediation_type_filter_selects_the_remediations_of_that_type() {
    let client = Client::new();
    let (_, listed) =
        generated(&client, "read-remediations", &PILLARS, RECOMMENDATION_TYPES[0]).await;

    // Which types any finding publishes a remediation for, so the table below is known to
    // exercise both halves of the filter: a type that selects something somewhere, and a
    // type that selects nothing for a given recommendation.
    let mut published_types: Vec<String> = Vec::new();

    for item in &listed {
        let pillar = text(item, "pillar");
        let arn = text(item, "recommendationArn");
        let unfiltered = detail(&get_recommendation(&client, &arn, "").await, &pillar);
        let all = rows(&unfiltered, "remediations");
        assert!(
            !all.is_empty(),
            "case {pillar:?}: a finding publishes the remediations that apply its fix, got \
             {unfiltered:?}"
        );
        for remediation in &all {
            assert_eq!(
                text(remediation, "recommendationArn"),
                arn,
                "case {pillar:?}: a remediation names the recommendation it belongs to, got \
                 {remediation:?}"
            );
            assert!(
                REMEDIATION_TYPES.contains(&text(remediation, "type").as_str()),
                "case {pillar:?}: a remediation's type must be a published RemediationType, \
                 got {remediation:?}"
            );
            let steps = rows(remediation, "steps");
            assert!(
                (1..=10).contains(&steps.len()),
                "case {pillar:?}: RemediationSteps is @length(1, 10), got {remediation:?}"
            );
            assert_eq!(
                text(remediation, "createdBy"),
                engine::emulator_account_id(),
                "case {pillar:?}: the remediation was published with the finding, by the \
                 service, got {remediation:?}"
            );
            published_types.push(text(remediation, "type"));
        }

        // One row per published `RemediationType`: the filter keeps exactly the
        // remediations whose own type equals it, which is none of them for a type this
        // finding publishes no remediation for.
        for remediation_type in REMEDIATION_TYPES {
            let case = format!("{pillar}?remediationType={remediation_type}");
            let filtered = detail(
                &get_recommendation(&client, &arn, &format!("?remediationType={remediation_type}"))
                    .await,
                &case,
            );
            let selected = rows(&filtered, "remediations");
            let expected: Vec<Value> = all
                .iter()
                .filter(|remediation| text(remediation, "type") == remediation_type)
                .cloned()
                .collect();
            assert_eq!(
                selected, expected,
                "case {case:?}: the filter must select exactly the remediations of that \
                 type, got {selected:?}"
            );
            // The filter selects remediations and nothing else: the recommendation itself
            // reads back unchanged.
            for name in ["recommendationArn", "pillar", "title", "remediationSummary"] {
                assert_eq!(
                    filtered.get(name),
                    unfiltered.get(name),
                    "case {case:?}: a remediation filter must not change {name}, got {:?}",
                    filtered.get(name)
                );
            }
        }
    }

    published_types.sort_unstable();
    published_types.dedup();
    let mut expected_types: Vec<String> =
        REMEDIATION_TYPES.iter().map(|t| (*t).to_string()).collect();
    expected_types.sort_unstable();
    assert_eq!(
        published_types, expected_types,
        "the published findings together must cover every RemediationType the enum names, so \
         the filter table above exercises each one on a type that exists somewhere"
    );
}

/// Rule 4: `goals` are the profile's own goals that target the recommendation's pillar.
#[tokio::test]
async fn a_read_reports_the_profile_goals_that_target_the_pillar() {
    let client = Client::new();
    let profile_arn = super::agent_goals::profile_arn_of(&client, "read-goals", 1).await;

    // A goal of one pillar, a goal of two, and a goal of every pillar: the goals a
    // recommendation targets are the ones whose pillars include its own.
    let goals: Vec<(&str, Vec<&str>)> = vec![
        ("Harden the perimeter", vec![PILLARS[1]]),
        ("Cut the monthly bill", vec![PILLARS[0], PILLARS[2]]),
        ("Review every pillar", PILLARS.to_vec()),
    ];
    for (title, pillars) in &goals {
        let answer = create_goal(
            &client,
            &profile_arn,
            json!({ "pillars": pillars, "title": title }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {title:?}: expected CreateAgentGoal 200, got {answer:?}"
        );
    }

    let started = start_generation(&client, &profile_arn, &PILLARS, RECOMMENDATION_TYPES[0]).await;
    assert_eq!(
        started.status, 200,
        "case \"read-goals\": expected StartAgentRecommendationGeneration 200, got {started:?}"
    );
    tokio::time::sleep(until_complete()).await;

    for item in recommendations_of(&client, &profile_arn, "read-goals").await {
        let pillar = text(&item, "pillar");
        let arn = text(&item, "recommendationArn");
        let body = detail(&get_recommendation(&client, &arn, "").await, &pillar);

        let mut reported: Vec<String> = rows(&body, "goals")
            .iter()
            .map(|goal| text(goal, "title"))
            .collect();
        reported.sort();
        let mut expected: Vec<String> = goals
            .iter()
            .filter(|(_, pillars)| pillars.contains(&pillar.as_str()))
            .map(|(title, _)| (*title).to_string())
            .collect();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {pillar:?}: the read must report the profile's goals that target this \
             pillar, got {body:?}"
        );
    }

    // A profile with no goals at all: the member is optional, so the read reports none.
    let (_, listed) =
        generated(&client, "read-no-goals", &[PILLARS[3]], RECOMMENDATION_TYPES[1]).await;
    let arn = text(&listed[0], "recommendationArn");
    let body = detail(&get_recommendation(&client, &arn, "").await, "no-goals").clone();
    assert!(
        rows(&body, "goals").is_empty(),
        "case \"no-goals\": a profile with no goal targets nothing, got {body:?}"
    );
}

/// Rule 5: a request the service cannot serve is refused, and the refusal names what was
/// looked for.
#[tokio::test]
async fn a_read_the_service_cannot_serve_is_refused() {
    let client = Client::new();
    let (_, listed) =
        generated(&client, "read-refusals", &[PILLARS[1]], RECOMMENDATION_TYPES[0]).await;
    let arn = text(&listed[0], "recommendationArn");
    let before = detail(&get_recommendation(&client, &arn, "").await, "refusal-setup");

    // A well-formed `AgentRecommendationArn` no recommendation carries.
    let absent_arn = format!(
        "arn:aws:wellarchitected:us-west-2:{}:agent-recommendation/abcdefab-0000-4a00-8b00-00000000000d",
        engine::emulator_account_id()
    );
    let answer = get_recommendation(&client, &absent_arn, "").await;
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

    // An agent profile's ARN is not a recommendation's: the modelled pattern refuses it
    // before any lookup.
    let refusals: Vec<(&str, Answer)> = vec![
        (
            "a remediation type outside the published enum",
            get_recommendation(&client, &arn, "?remediationType=TERRAFORM").await,
        ),
        (
            "an ARN the recommendation pattern refuses",
            get_recommendation(
                &client,
                &format!(
                    "arn:aws:wellarchitected:us-west-2:{}:agent-recommendation/not-a-uuid",
                    engine::emulator_account_id()
                ),
                "",
            )
            .await,
        ),
        (
            "an agent profile ARN where a recommendation ARN belongs",
            get_recommendation(&client, &absent_profile_arn("read-refusals"), "").await,
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
    let after = detail(&get_recommendation(&client, &arn, "").await, "refusal-after");
    assert_eq!(
        after, before,
        "case \"refusals\": a refused request must leave the recommendation as it was"
    );
}

/// Rule 6: the detail this emulator publishes for every pillar sits inside the bounds the
/// modelled shapes declare and names values its enums publish.
#[test]
fn the_published_detail_of_every_pillar_is_within_the_modelled_bounds() {
    // The bounds the model declares for the detail members, from
    // `smithy_query(query="closure", operation="GetAgentRecommendation")`: `Highlight`
    // `@length(20, 250)` in `Highlights` `@length(1, 5)`, `ImpactDetail` `@length(10, 100)`
    // in `ImpactDetails` `@length(2, 3)`, `Insight$usagePattern` `@length(100, 2000)` and
    // `$signalsDetected` `@length(30, 1500)` in `InsightList` `@length(1, 5)`,
    // `RemediationSummary$recommendation` `@length(80, 240)` with `RecommendedFixStep`
    // `@length(20, 200)` in `RecommendedFixSteps` `@length(1, 5)`, `RemediationStep$title`
    // `@length(3, 80)` and `$content` `@length(80, 8000)` in `RemediationSteps`
    // `@length(1, 10)`, and for the cross-pillar members `$title` `@length(3, 80)`,
    // `CrossPillarBenefit$description` `@length(30, 300)`, `TradeOff$description` and
    // `$mitigation` `@length(30, 450)` and `$riskExplanation` `@length(1, 160)`.
    for content in state::RECOMMENDATION_CATALOG {
        let case = content.pillar;

        assert!(
            (1..=5).contains(&content.highlights.len()),
            "case {case:?}: Highlights is @length(1, 5), got {} entries",
            content.highlights.len()
        );
        assert!(
            (2..=3).contains(&content.impact_details.len()),
            "case {case:?}: ImpactDetails is @length(2, 3), got {} entries",
            content.impact_details.len()
        );
        assert!(
            (1..=5).contains(&content.insights.len()),
            "case {case:?}: InsightList is @length(1, 5), got {} entries",
            content.insights.len()
        );
        assert!(
            (1..=5).contains(&content.remediation_steps.len()),
            "case {case:?}: RecommendedFixSteps is @length(1, 5), got {} entries",
            content.remediation_steps.len()
        );

        let mut bounded: Vec<(String, &str, (usize, usize))> = Vec::new();
        for highlight in content.highlights {
            bounded.push(("highlight".to_string(), highlight, (20, 250)));
        }
        for impact_detail in content.impact_details {
            bounded.push(("impactDetail".to_string(), impact_detail, (10, 100)));
        }
        for insight in content.insights {
            bounded.push((
                "insight.usagePattern".to_string(),
                insight.usage_pattern,
                (100, 2000),
            ));
            bounded.push((
                "insight.signalsDetected".to_string(),
                insight.signals_detected,
                (30, 1500),
            ));
        }
        bounded.push((
            "remediationSummary.recommendation".to_string(),
            content.remediation_recommendation,
            (80, 240),
        ));
        for step in content.remediation_steps {
            bounded.push(("remediationSummary.step".to_string(), step, (20, 200)));
        }
        for benefit in content.cross_pillar_benefits {
            bounded.push(("crossPillarBenefit.title".to_string(), benefit.title, (3, 80)));
            bounded.push((
                "crossPillarBenefit.description".to_string(),
                benefit.description,
                (30, 300),
            ));
        }
        for trade_off in content.trade_offs {
            bounded.push(("tradeOff.title".to_string(), trade_off.title, (3, 80)));
            bounded.push((
                "tradeOff.description".to_string(),
                trade_off.description,
                (30, 450),
            ));
            bounded.push((
                "tradeOff.mitigation".to_string(),
                trade_off.mitigation,
                (30, 450),
            ));
            bounded.push((
                "tradeOff.riskExplanation".to_string(),
                trade_off.risk_explanation,
                (1, 160),
            ));
        }
        for remediation in content.remediations {
            assert!(
                (1..=10).contains(&remediation.steps.len()),
                "case {case:?}: RemediationSteps is @length(1, 10), got {} entries for {:?}",
                remediation.steps.len(),
                remediation.remediation_type
            );
            assert!(
                REMEDIATION_TYPES.contains(&remediation.remediation_type),
                "case {case:?}: a remediation's type must be one of {REMEDIATION_TYPES:?}, got \
                 {:?}",
                remediation.remediation_type
            );
            for step in remediation.steps {
                bounded.push(("remediation.step.title".to_string(), step.title, (3, 80)));
                bounded.push((
                    "remediation.step.content".to_string(),
                    step.content,
                    (80, 8000),
                ));
            }
        }

        for (name, value, (min, max)) in bounded {
            let length = value.chars().count();
            assert!(
                (min..=max).contains(&length),
                "case {case:?}: {name} must be {min}..={max} characters, got {length} in \
                 {value:?}"
            );
        }

        for benefit in content.cross_pillar_benefits {
            assert!(
                PILLARS.contains(&benefit.pillar) && benefit.pillar != content.pillar,
                "case {case:?}: a cross-pillar benefit names ANOTHER published pillar, got \
                 {:?}",
                benefit.pillar
            );
            assert!(
                IMPACT_CATEGORIES.contains(&benefit.impact),
                "case {case:?}: a benefit's impact must be one of {IMPACT_CATEGORIES:?}, got \
                 {:?}",
                benefit.impact
            );
        }
        for trade_off in content.trade_offs {
            assert!(
                PILLARS.contains(&trade_off.pillar) && trade_off.pillar != content.pillar,
                "case {case:?}: a trade-off names ANOTHER published pillar, got {:?}",
                trade_off.pillar
            );
            assert!(
                RISK_RATINGS.contains(&trade_off.risk),
                "case {case:?}: a trade-off's risk must be one of {RISK_RATINGS:?}, got {:?}",
                trade_off.risk
            );
        }
    }
}
