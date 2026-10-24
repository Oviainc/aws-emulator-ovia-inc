// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for an agent profile's recommendation generations:
//! `StartAgentRecommendationGeneration`, which records one, and the
//! `GetAgentRecommendationGeneration` that reports it.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one request:
//!
//! 1. `GetAgentRecommendationGeneration` REPORTS the generation the start recorded: the
//!    minted `UUID`, the `profileArn` it was started against, the `name`, `scope` and
//!    `additionalContext` as sent, and the `createdBy` / `createdAt` the service owns.
//! 2. A generation is a JOB whose status is owed by the CLOCK: it is `QUEUED` until its
//!    analysis is due to start, `IN_PROGRESS` until it is due to finish and then the
//!    terminal `COMPLETED`, with `startedAt` / `endedAt` appearing exactly when those
//!    instants pass and `lastModifiedAt` reporting the latest transition that happened.
//!    Both spans are non-zero, so a caller polling straight after the start call still
//!    observes `QUEUED`.
//! 3. `progress` agrees with the scope and the status: `totalSteps` is how many pillars
//!    the generation's scope names, `stepsCompleted` grows from 0 to that total and
//!    `completionPercentage` is that same share as a percentage — 0 while queued and 100
//!    once complete.
//! 4. Reading a generation changes nothing a later caller sees: repeated polls report the
//!    same identity, scope and instants, and the status only ever moves forwards.
//! 5. A generation is reached only THROUGH its profile: an id that belongs to another
//!    profile, or to none, is a `404 ResourceNotFoundException` naming the composite
//!    resource id and the missing resource's type, and a value the modelled `UUID` /
//!    `AgentProfileArn` pattern refuses never reaches the lookup — the boundary refuses it
//!    `400 ValidationException`.
//! 6. `StartAgentRecommendationGeneration` is modelled `@idempotent`: repeating a start
//!    request while the generation it started is still in flight answers THAT generation
//!    and records no second one, while a request differing in any member starts its own.
//! 7. A start the service cannot serve is refused `400 ValidationException` and records
//!    nothing: a `profileArn` that names no stored profile, a profile that names no
//!    aggregation scope to analyse (the derived `eligibleFor…` answer `GetAgentProfile`
//!    reports), a `scope.goalIds` entry that names no goal of that profile, and a value
//!    outside a published enum's set.
//! 8. `additionalContext` is an untyped `Document`, so its nesting is caller-chosen: a
//!    document nested to the ceiling this service holds documents to
//!    ([`state::MAX_DOCUMENT_DEPTH`]) round-trips as sent, and one past it is refused
//!    `400 ValidationException` rather than walked.
//! 9. A generation does not outlive the profile it is reached through: deleting the
//!    profile takes its generations with it, which is what keeps the per-profile ceiling
//!    a real bound on the store — an `AgentProfileArn` is never minted twice, so a
//!    generation left under a deleted profile's ARN could never be reclaimed.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client lives in `super::workload_harness`; the profile the
//! generations hang off, and the goals a scope may name, are minted by the operations in
//! `super::agent_goals`.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

use super::agent_goals::{absent_profile_arn, create_goal, profile_arn_of, PILLARS};
use super::agent_profile_lifecycle::delete_profile;
use super::workload_harness::{encode_path_segment, Answer, Client};

/// Every value the `GenerationStatus` enum publishes, in the order a generation passes
/// through them; `ERROR` is the terminal status of a FAILED generation and is not on that
/// path (`smithy_query(query="shape", shape="GenerationStatus")`). Spelled out here so the
/// suite pins the catalog rather than reading it back out of the code under test.
const GENERATION_STATUSES: [&str; 3] = ["QUEUED", "IN_PROGRESS", "COMPLETED"];

/// Every value the `RecommendationType` enum publishes, in the model's order
/// (`smithy_query(query="shape", shape="RecommendationType")`).
const RECOMMENDATION_TYPES: [&str; 3] = ["RESOURCE", "ARCHITECTURE", "APPLICATION"];

/// `POST /api/v1/agent-profiles/{profileArn}/generations`: the ARN is a non-greedy
/// `@httpLabel`, so it travels percent-encoded in one path segment, exactly as an SDK
/// sends it.
async fn start_generation(client: &Client, profile_arn: &str, body: Value) -> Answer {
    client
        .send(
            "POST",
            &format!(
                "/api/v1/agent-profiles/{}/generations",
                encode_path_segment(profile_arn)
            ),
            Some(body),
        )
        .await
}

/// `GET /api/v1/agent-profiles/{profileArn}/generations/{generationId}`.
async fn get_generation(client: &Client, profile_arn: &str, generation_id: &str) -> Answer {
    client
        .send(
            "GET",
            &format!(
                "/api/v1/agent-profiles/{}/generations/{}",
                encode_path_segment(profile_arn),
                encode_path_segment(generation_id)
            ),
            None,
        )
        .await
}

/// The generation id a successful start answered with, checked against the modelled
/// `UUID` pattern (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`).
fn started_id(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected StartAgentRecommendationGeneration 200, got {answer:?}"
    );
    let id = answer
        .body
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let groups: Vec<&str> = id.split('-').collect();
    assert!(
        groups.len() == 5
            && [8usize, 4, 4, 4, 12]
                .iter()
                .zip(&groups)
                .all(|(len, group)| group.len() == *len
                    && group.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))),
        "case {case:?}: the generation id must match the modelled UUID pattern, got {id:?}"
    );
    id
}

/// A timestamp this operation reports, in epoch seconds.
///
/// Every `Timestamp` of these two shapes carries `@timestampFormat("date-time")`
/// (`smithy_query(query="operation", operation="GetAgentRecommendationGeneration")`), so
/// it travels as an RFC 3339 string; a number is accepted too, so the helper pins the
/// INSTANT rather than one spelling of it.
fn instant_seconds(value: Option<&Value>, case: &str, label: &str) -> f64 {
    match value {
        Some(Value::Number(number)) => number.as_f64().unwrap_or_default(),
        Some(Value::String(text)) => ::aws_smithy_types::DateTime::from_str(
            text,
            ::aws_smithy_types::date_time::Format::DateTime,
        )
        .unwrap_or_else(|error| {
            panic!("case {case:?}: {label} must be an RFC 3339 instant, got {text:?}: {error}")
        })
        .as_secs_f64(),
        other => panic!("case {case:?}: expected a {label} timestamp, got {other:?}"),
    }
}

/// The test's own clock in epoch seconds — what a reported instant is checked against,
/// since no two runs share a recorded one.
fn now_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the test clock is after the Unix epoch")
        .as_secs_f64()
}

/// The status the clock owes a generation at `instant`, from the two instants the start
/// answered: `QUEUED` until its analysis is due to start (`createdAt` plus the queue
/// span), `IN_PROGRESS` until it is due to finish (`estimatedCompletionTime`), then the
/// terminal `COMPLETED`.
fn status_at(created_at: f64, estimated: f64, instant: f64) -> &'static str {
    if instant < created_at + state::GENERATION_QUEUE_DURATION.as_secs_f64() {
        "QUEUED"
    } else if instant < estimated {
        "IN_PROGRESS"
    } else {
        "COMPLETED"
    }
}

/// The statuses a read taken somewhere between `before` and `after` may report: the
/// contiguous span the clock passed through while the request was in flight. A read is
/// never asserted against the status at ONE instant, because the request takes time to
/// serve — but it must report a status the clock really owed while it was being served.
fn statuses_between(created_at: f64, estimated: f64, before: f64, after: f64) -> Vec<&'static str> {
    let index = |instant| {
        GENERATION_STATUSES
            .iter()
            .position(|published| *published == status_at(created_at, estimated, instant))
            .expect("the derived status is one the enum publishes")
    };
    GENERATION_STATUSES[index(before)..=index(after)].to_vec()
}

/// Read one generation, with the statuses the clock owed while the read was in flight.
async fn read_generation(
    client: &Client,
    profile_arn: &str,
    id: &str,
    created_at: f64,
    estimated: f64,
) -> (Answer, Vec<&'static str>) {
    let before = now_seconds();
    let answer = get_generation(client, profile_arn, id).await;
    let after = now_seconds();
    (
        answer,
        statuses_between(created_at, estimated, before, after),
    )
}

/// The status a read reported, checked against the statuses the clock owed while it was
/// in flight.
fn reported_status(answer: &Answer, owed: &[&str], case: &str) -> String {
    let status = answer
        .body
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        owed.contains(&status.as_str()),
        "case {case:?}: a read taken while the clock owed one of {owed:?} must report one \
         of them, got {status:?} in {answer:?}"
    );
    status
}

/// A document nested `levels` deep: `levels` objects around one string leaf, so its
/// deepest node sits at `levels + 1`. What the document-depth bound is tested on.
fn nested_document(levels: usize) -> Value {
    let mut document = json!("leaf");
    for _ in 0..levels {
        document = json!({ "nested": document });
    }
    document
}

/// One row of the table every generation test spans: what the start request carries.
struct Row {
    case: &'static str,
    /// The pillars the generation's scope names — what `progress.totalSteps` counts.
    pillars: Vec<&'static str>,
    recommendation_type: &'static str,
    name: Option<&'static str>,
    /// The untyped `additionalContext` document, in the forms a `Document` takes.
    additional_context: Option<Value>,
    /// Per-pillar item filtering, so `Scope$items` is exercised as well as omitted.
    items: Option<Value>,
    /// How many of the profile's goals `scope.goalIds` names.
    goal_ids: usize,
}

/// The rows every generation test spans: scopes of one, several and every published
/// pillar (the modelled `Scope$pillars` `@length(1, 5)` bounds among them), each
/// recommendation type the enum publishes, the optional `name` present and absent, and an
/// `additionalContext` document of each form one can take — object, nested object, string,
/// number and array — beside a request that carries none.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one-pillar-bare",
            pillars: vec!["SECURITY"],
            recommendation_type: "RESOURCE",
            name: None,
            additional_context: None,
            items: None,
            goal_ids: 0,
        },
        Row {
            case: "two-pillars-named-object-context",
            pillars: vec!["COST_OPTIMIZATION", "RESILIENCE"],
            recommendation_type: "ARCHITECTURE",
            name: Some("quarterly-review"),
            additional_context: Some(json!({ "budget": "fixed", "window": "nights" })),
            items: None,
            goal_ids: 1,
        },
        Row {
            case: "every-published-pillar-nested-context",
            pillars: PILLARS.to_vec(),
            recommendation_type: "APPLICATION",
            name: Some("whole-framework"),
            additional_context: Some(json!({
                "constraints": { "regions": ["us-east-1", "eu-west-1"], "maxChanges": 3 },
                "notes": ["migrating", "regulated"],
            })),
            items: Some(json!([{ "pillar": "SECURITY", "ids": ["item-1", "item-2"] }])),
            goal_ids: 2,
        },
        Row {
            case: "pillar-the-issue-did-not-cite-string-context",
            pillars: vec!["OPERATIONAL_EXCELLENCE"],
            recommendation_type: "RESOURCE",
            name: Some("runbook-rehearsal"),
            additional_context: Some(json!("free-form guidance")),
            items: None,
            goal_ids: 0,
        },
        Row {
            case: "three-pillars-numeric-context",
            pillars: vec!["PERFORMANCE", "SECURITY", "RESILIENCE"],
            recommendation_type: "ARCHITECTURE",
            name: None,
            additional_context: Some(json!(42)),
            // `PillarItem$ids` is modelled `@length(min: 1)`, so every row names at
            // least one item.
            items: Some(json!([
                { "pillar": "PERFORMANCE", "ids": ["p-1"] },
                { "pillar": "RESILIENCE", "ids": ["r-1", "r-2"] },
            ])),
            goal_ids: 1,
        },
        Row {
            case: "four-pillars-array-context",
            pillars: vec![
                "SECURITY",
                "COST_OPTIMIZATION",
                "PERFORMANCE",
                "OPERATIONAL_EXCELLENCE",
            ],
            recommendation_type: "APPLICATION",
            name: Some("broad-sweep"),
            additional_context: Some(json!(["one", 2, true, null])),
            items: None,
            goal_ids: 0,
        },
    ]
}

impl Row {
    /// The `StartAgentRecommendationGeneration` body this row sends, with `goals` the ids
    /// of the goals its `scope.goalIds` may name.
    fn body(&self, goals: &[String]) -> Value {
        let mut scope = json!({ "pillars": self.pillars });
        if self.goal_ids > 0 {
            scope["goalIds"] = json!(goals[..self.goal_ids]);
        }
        if let Some(items) = self.items.clone() {
            scope["items"] = items;
        }
        let mut body = json!({ "types": [self.recommendation_type], "scope": scope });
        if let Some(name) = self.name {
            body["name"] = json!(name);
        }
        if let Some(context) = self.additional_context.clone() {
            body["additionalContext"] = context;
        }
        body
    }
}

/// Two goals of `profile_arn`, so a row whose scope names goals has stored ones to name.
async fn goals_of(client: &Client, profile_arn: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for (index, pillar) in ["SECURITY", "COST_OPTIMIZATION"].iter().enumerate() {
        let answer = create_goal(
            client,
            profile_arn,
            json!({ "pillars": [pillar], "title": format!("goal {index}") }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "expected CreateAgentGoal 200 for the scope's goals, got {answer:?}"
        );
        ids.push(
            answer
                .body
                .get("goal")
                .and_then(|goal| goal.get("id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        );
    }
    ids
}

/// Assert the projection of one generation: every member the start request carried comes
/// back as sent, the service-owned members are its own, and the clock-derived members
/// agree with `expected_status`.
///
/// The one place the projection is pinned, so both the start answer and the read are
/// checked against the same rule. `reader` names the operation under test.
fn assert_generation(
    answer: &Answer,
    row: &Row,
    sent: &Value,
    profile_arn: &str,
    id: &str,
    expected_status: &str,
    reader: &str,
) {
    let case = row.case;
    assert_eq!(
        answer.status, 200,
        "case {case:?}/{reader}: expected 200, got {answer:?}"
    );
    let body = &answer.body;
    assert_eq!(
        body.get("id").and_then(Value::as_str),
        Some(id),
        "case {case:?}/{reader}: expected the generation the start minted, got {body:?}"
    );
    assert_eq!(
        body.get("profileArn").and_then(Value::as_str),
        Some(profile_arn),
        "case {case:?}/{reader}: the generation must name the profile it was started \
         against, got {body:?}"
    );
    assert_eq!(
        body.get("name").and_then(Value::as_str),
        row.name,
        "case {case:?}/{reader}: expected the name as sent, got {body:?}"
    );
    assert_eq!(
        body.get("status").and_then(Value::as_str),
        Some(expected_status),
        "case {case:?}/{reader}: expected status {expected_status:?}, got {body:?}"
    );
    assert_eq!(
        body.get("createdBy").and_then(Value::as_str),
        Some(engine::emulator_account_id()),
        "case {case:?}/{reader}: createdBy must be the account the request is attributed \
         to, got {body:?}"
    );

    let created_at = instant_seconds(body.get("createdAt"), case, "createdAt");
    let now = now_seconds();
    assert!(
        created_at > 0.0 && (now - created_at).abs() < 300.0,
        "case {case:?}/{reader}: createdAt must be the instant of the start call \
         (about {now}), got {created_at}"
    );
    let estimated = instant_seconds(
        body.get("estimatedCompletionTime"),
        case,
        "estimatedCompletionTime",
    );
    assert!(
        estimated > created_at,
        "case {case:?}/{reader}: estimatedCompletionTime must be after createdAt \
         ({created_at}), got {estimated}"
    );

    // Members only `GetAgentRecommendationGeneration` reports.
    if reader == "GetAgentRecommendationGeneration" {
        assert_eq!(
            body.get("additionalContext"),
            sent.get("additionalContext"),
            "case {case:?}/{reader}: expected additionalContext as sent, got {body:?}"
        );
        assert_eq!(
            body.get("scope"),
            sent.get("scope"),
            "case {case:?}/{reader}: expected the scope as sent, got {body:?}"
        );
        assert!(
            body.get("errorDetails").is_none(),
            "case {case:?}/{reader}: a generation that did not fail reports no \
             errorDetails, got {body:?}"
        );

        let progress = body.get("progress").cloned().unwrap_or(Value::Null);
        let total = progress.get("totalSteps").and_then(Value::as_i64);
        assert_eq!(
            total,
            Some(row.pillars.len() as i64),
            "case {case:?}/{reader}: totalSteps must count the pillars the scope names, \
             got {progress:?}"
        );
        let completed = progress
            .get("stepsCompleted")
            .and_then(Value::as_i64)
            .unwrap_or(-1);
        let percentage = progress
            .get("completionPercentage")
            .and_then(Value::as_f64)
            .unwrap_or(-1.0);
        let expected_completed = match expected_status {
            "QUEUED" => Some(0),
            "COMPLETED" => total,
            _ => None,
        };
        if let Some(expected) = expected_completed {
            assert_eq!(
                completed,
                expected,
                "case {case:?}/{reader}: a {expected_status} generation reports \
                 {expected} of {total:?} steps, got {progress:?}"
            );
        } else {
            assert!(
                (0..total.unwrap_or_default()).contains(&completed),
                "case {case:?}/{reader}: an in-flight generation reports fewer steps \
                 than its total, got {progress:?}"
            );
        }
        let expected_percentage =
            completed as f64 * 100.0 / total.unwrap_or(1).max(1) as f64;
        assert!(
            (percentage - expected_percentage).abs() < 1e-6,
            "case {case:?}/{reader}: completionPercentage must be the share of steps \
             done ({expected_percentage}), got {progress:?}"
        );
    }

    // `startedAt`, `endedAt` and the `lastModified…` pair are owed by the clock: each
    // appears exactly once its instant has passed, and the latest transition that
    // happened is what last modified the generation.
    let started = body.get("startedAt");
    let ended = body.get("endedAt");
    let last_modified_at = body.get("lastModifiedAt");
    match expected_status {
        "QUEUED" => {
            assert!(
                started.is_none() && ended.is_none() && last_modified_at.is_none(),
                "case {case:?}/{reader}: a QUEUED generation has not started, ended or \
                 been modified, got {body:?}"
            );
            assert!(
                body.get("lastModifiedBy").is_none(),
                "case {case:?}/{reader}: a QUEUED generation reports no lastModifiedBy, \
                 got {body:?}"
            );
        }
        "IN_PROGRESS" => {
            let started = instant_seconds(started, case, "startedAt");
            assert!(
                ended.is_none(),
                "case {case:?}/{reader}: an IN_PROGRESS generation has not ended, \
                 got {body:?}"
            );
            assert!(
                (instant_seconds(last_modified_at, case, "lastModifiedAt") - started).abs()
                    < 1e-6,
                "case {case:?}/{reader}: lastModifiedAt must be the latest transition \
                 ({started}), got {body:?}"
            );
            assert!(
                started >= created_at,
                "case {case:?}/{reader}: startedAt must not precede createdAt \
                 ({created_at}), got {started}"
            );
        }
        _ => {
            let started = instant_seconds(started, case, "startedAt");
            let ended = instant_seconds(ended, case, "endedAt");
            assert!(
                ended >= started && started >= created_at,
                "case {case:?}/{reader}: expected createdAt {created_at} <= startedAt \
                 {started} <= endedAt {ended}, got {body:?}"
            );
            assert!(
                (instant_seconds(last_modified_at, case, "lastModifiedAt") - ended).abs()
                    < 1e-6,
                "case {case:?}/{reader}: lastModifiedAt must be the latest transition \
                 ({ended}), got {body:?}"
            );
            assert_eq!(
                body.get("lastModifiedBy").and_then(Value::as_str),
                Some(engine::emulator_account_id()),
                "case {case:?}/{reader}: the service is what moved the status, \
                 got {body:?}"
            );
        }
    }
    if reader == "GetAgentRecommendationGeneration" && expected_status == "COMPLETED" {
        let ended = instant_seconds(ended, case, "endedAt");
        let estimated = instant_seconds(
            body.get("estimatedCompletionTime"),
            case,
            "estimatedCompletionTime",
        );
        assert!(
            (ended - estimated).abs() < 1e-6,
            "case {case:?}/{reader}: a completed generation ended at the instant it was \
             estimated to ({estimated}), got {ended}"
        );
    }
}

/// Rule 1, 2 and 3 at the start of the job: every row's generation is recorded and read
/// back as the clock owes it — `QUEUED` for a read taken inside the queue span, which is
/// non-zero on purpose so a caller polling straight after the start can observe it.
#[tokio::test]
async fn a_started_generation_round_trips_through_the_read() {
    assert!(
        !state::GENERATION_QUEUE_DURATION.is_zero(),
        "a generation's queue span must be non-zero so a caller can observe QUEUED, got {:?}",
        state::GENERATION_QUEUE_DURATION
    );

    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "generation-host", 1).await;
    let goals = goals_of(&client, &profile_arn).await;

    let mut ids: Vec<String> = Vec::new();
    for row in rows() {
        let body = row.body(&goals);
        let started = start_generation(&client, &profile_arn, body.clone()).await;
        let id = started_id(&started, row.case);
        // The start answers the generation as of the instant it accepted it, which is
        // inside the queue span whatever the machine's load.
        assert_generation(
            &started,
            &row,
            &body,
            &profile_arn,
            &id,
            "QUEUED",
            "StartAgentRecommendationGeneration",
        );
        let created_at = instant_seconds(started.body.get("createdAt"), row.case, "createdAt");
        let estimated = instant_seconds(
            started.body.get("estimatedCompletionTime"),
            row.case,
            "estimatedCompletionTime",
        );

        let (fetched, owed) =
            read_generation(&client, &profile_arn, &id, created_at, estimated).await;
        let status = reported_status(&fetched, &owed, row.case);
        assert_generation(
            &fetched,
            &row,
            &body,
            &profile_arn,
            &id,
            &status,
            "GetAgentRecommendationGeneration",
        );

        assert!(
            !ids.contains(&id),
            "case {:?}: every generation gets its own id, got a repeat of {id:?}",
            row.case
        );
        ids.push(id);
    }
}

/// Rule 2 and 3 over the whole job: a generation's status and progress are whatever the
/// clock owes at the instant of the read, and once the spans have passed it is
/// `COMPLETED` with every step of its scope done.
#[tokio::test]
async fn a_generation_moves_through_the_statuses_the_clock_owes() {
    assert!(
        !state::GENERATION_RUN_DURATION.is_zero(),
        "a generation's run span must be non-zero so a caller can observe IN_PROGRESS, \
         got {:?}",
        state::GENERATION_RUN_DURATION
    );

    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "generation-clock", 2).await;
    let goals = goals_of(&client, &profile_arn).await;

    // Start every row first, so one pair of sleeps covers the whole table.
    let mut started: Vec<(Row, Value, String, f64, f64)> = Vec::new();
    for row in rows() {
        let body = row.body(&goals);
        let answer = start_generation(&client, &profile_arn, body.clone()).await;
        let id = started_id(&answer, row.case);
        let created_at = instant_seconds(answer.body.get("createdAt"), row.case, "createdAt");
        let estimated = instant_seconds(
            answer.body.get("estimatedCompletionTime"),
            row.case,
            "estimatedCompletionTime",
        );
        started.push((row, body, id, created_at, estimated));
    }

    // Partway into the analysis: each read reports the status the clock owed while it was
    // in flight, and its members agree with that status.
    tokio::time::sleep(state::GENERATION_QUEUE_DURATION + state::GENERATION_RUN_DURATION / 2)
        .await;
    for (row, body, id, created_at, estimated) in &started {
        let (answer, owed) =
            read_generation(&client, &profile_arn, id, *created_at, *estimated).await;
        let status = reported_status(&answer, &owed, row.case);
        assert_generation(
            &answer,
            row,
            body,
            &profile_arn,
            id,
            &status,
            "GetAgentRecommendationGeneration",
        );
    }

    // Past the instant every completion was due, plus a margin: the only status the clock
    // owes now is the terminal one.
    tokio::time::sleep(state::GENERATION_RUN_DURATION / 2 + Duration::from_millis(500)).await;
    for (row, body, id, created_at, estimated) in &started {
        let (answer, owed) =
            read_generation(&client, &profile_arn, id, *created_at, *estimated).await;
        assert_eq!(
            owed,
            vec!["COMPLETED"],
            "case {:?}: the sleeps must have outlasted both spans the clock owes",
            row.case
        );
        let status = reported_status(&answer, &owed, row.case);
        assert_generation(
            &answer,
            row,
            body,
            &profile_arn,
            id,
            &status,
            "GetAgentRecommendationGeneration",
        );
    }
}

/// Rule 2 and 3 exactly, at the instants the transitions come due: the stored record
/// derives each published status, its progress and its transition timestamps from the
/// clock, so every phase is pinned without a sleep deciding which one a read lands in.
#[tokio::test]
async fn the_stored_record_derives_each_status_and_progress_from_the_clock() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "generation-derived", 1).await;
    let goals = goals_of(&client, &profile_arn).await;

    for row in rows() {
        let answer = start_generation(&client, &profile_arn, row.body(&goals)).await;
        let id = started_id(&answer, row.case);
        let store = client.store();
        let guard = store.lock();
        let record = guard
            .agent_generation(&profile_arn, &id)
            .expect("the start must have recorded the generation under its profile");
        let total = i32::try_from(row.pillars.len()).expect("a scope names at most 5 pillars");
        let tick = Duration::from_millis(1);

        let phases: [(&str, SystemTime, &str); 8] = [
            ("the instant of the start call", record.created_at, "QUEUED"),
            (
                "halfway through the queue",
                record.created_at + state::GENERATION_QUEUE_DURATION / 2,
                "QUEUED",
            ),
            (
                "one tick before the analysis starts",
                record.starts_at - tick,
                "QUEUED",
            ),
            (
                "the instant the analysis starts",
                record.starts_at,
                "IN_PROGRESS",
            ),
            (
                "halfway through the analysis",
                record.starts_at + state::GENERATION_RUN_DURATION / 2,
                "IN_PROGRESS",
            ),
            (
                "one tick before it ends",
                record.ends_at - tick,
                "IN_PROGRESS",
            ),
            ("the instant it ends", record.ends_at, "COMPLETED"),
            (
                "long after it ended",
                record.ends_at + Duration::from_secs(3_600),
                "COMPLETED",
            ),
        ];

        for (phase, instant, expected) in phases {
            let case = format!("{}/{phase}", row.case);
            assert_eq!(
                record.status(instant),
                expected,
                "case {case:?}: expected status {expected:?}, got {:?}",
                record.status(instant)
            );
            assert_eq!(
                record.started_at(instant).is_some(),
                expected != "QUEUED",
                "case {case:?}: startedAt appears exactly once the analysis has started, \
                 got {:?}",
                record.started_at(instant)
            );
            assert_eq!(
                record.ended_at(instant).is_some(),
                expected == "COMPLETED",
                "case {case:?}: endedAt appears exactly once the analysis has ended, \
                 got {:?}",
                record.ended_at(instant)
            );
            let expected_modified = match expected {
                "QUEUED" => None,
                "IN_PROGRESS" => Some(record.starts_at),
                _ => Some(record.ends_at),
            };
            assert_eq!(
                record.last_modified(instant).map(|(_, at)| at),
                expected_modified,
                "case {case:?}: lastModifiedAt is the latest transition that happened, \
                 got {:?}",
                record.last_modified(instant)
            );

            let progress = record.progress(instant);
            assert_eq!(
                progress.total_steps, total,
                "case {case:?}: totalSteps counts the pillars the scope names, \
                 got {progress:?}"
            );
            let expected_steps = match expected {
                "QUEUED" => 0,
                "COMPLETED" => total,
                _ => progress.steps_completed,
            };
            assert_eq!(
                progress.steps_completed, expected_steps,
                "case {case:?}: expected {expected_steps} of {total} steps, \
                 got {progress:?}"
            );
            assert!(
                (progress.completion_percentage
                    - f64::from(progress.steps_completed) * 100.0 / f64::from(total))
                    .abs()
                    < 1e-9,
                "case {case:?}: completionPercentage is the share of steps done, \
                 got {progress:?}"
            );
        }

        // Every step of the scope comes due in its own slice of the run span, so the
        // progress a read reports grows one step at a time rather than jumping.
        for step in 0..total {
            let share = (f64::from(step) + 0.5) / f64::from(total);
            let instant = record.starts_at
                + Duration::from_secs_f64(state::GENERATION_RUN_DURATION.as_secs_f64() * share);
            let progress = record.progress(instant);
            assert_eq!(
                progress.steps_completed, step,
                "case {:?}/step {step} of {total}: expected {step} steps done partway \
                 through that step, got {progress:?}",
                row.case
            );
        }
    }
}

/// Rule 4: the read is a REPORT. Polling it repeatedly never changes the generation's
/// identity, scope or instants, and the status only moves forwards through the published
/// order.
#[tokio::test]
async fn polling_a_generation_changes_nothing_a_later_caller_sees() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "generation-poll", 1).await;
    let goals = goals_of(&client, &profile_arn).await;
    let row = rows()
        .into_iter()
        .find(|row| row.case == "two-pillars-named-object-context")
        .expect("the row the poll test uses");
    let body = row.body(&goals);
    let started = start_generation(&client, &profile_arn, body.clone()).await;
    let id = started_id(&started, row.case);

    let mut last_status = 0usize;
    let mut first: Option<Value> = None;
    for poll in 0..8 {
        let answer = get_generation(&client, &profile_arn, &id).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}/poll {poll}: expected 200, got {answer:?}",
            row.case
        );
        let status = answer
            .body
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let index = GENERATION_STATUSES
            .iter()
            .position(|published| *published == status)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}/poll {poll}: status must be one of {GENERATION_STATUSES:?}, \
                     got {status:?}",
                    row.case
                )
            });
        assert!(
            index >= last_status,
            "case {:?}/poll {poll}: a generation's status never moves backwards, went \
             from {:?} to {status:?}",
            row.case,
            GENERATION_STATUSES[last_status]
        );
        last_status = index;

        // The members a read must never change, compared against the FIRST read.
        let stable = json!({
            "id": answer.body.get("id"),
            "profileArn": answer.body.get("profileArn"),
            "name": answer.body.get("name"),
            "scope": answer.body.get("scope"),
            "additionalContext": answer.body.get("additionalContext"),
            "createdAt": answer.body.get("createdAt"),
            "createdBy": answer.body.get("createdBy"),
            "estimatedCompletionTime": answer.body.get("estimatedCompletionTime"),
        });
        match &first {
            None => first = Some(stable),
            Some(expected) => assert_eq!(
                &stable, expected,
                "case {:?}/poll {poll}: a read must report the same stored generation, \
                 got {answer:?}",
                row.case
            ),
        }
        tokio::time::sleep(state::GENERATION_QUEUE_DURATION / 2).await;
    }
    assert_eq!(
        GENERATION_STATUSES[last_status], "COMPLETED",
        "case {:?}: the polls must have outlasted the spans the clock owes",
        row.case
    );
}

/// Rule 5: a generation is reached only through the profile that holds it, and a
/// malformed identifier never reaches the lookup.
#[tokio::test]
async fn a_generation_is_reached_only_through_the_profile_that_holds_it() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "generation-owner", 1).await;
    let other_arn = profile_arn_of(&client, "generation-stranger", 1).await;
    let row = rows().into_iter().next().expect("the first row");
    let started = start_generation(&client, &profile_arn, row.body(&[])).await;
    let id = started_id(&started, row.case);

    // A well-formed UUID the stored profile does not hold. It carries hex LETTERS, so
    // upper-casing it is a value the modelled pattern refuses.
    let absent_id = "abcdefab-0000-4a00-8b00-00000000000c";
    let not_found: [(&str, String, String); 3] = [
        (
            "another profile's generation",
            other_arn.clone(),
            id.clone(),
        ),
        (
            "an id no profile holds",
            profile_arn.clone(),
            absent_id.to_string(),
        ),
        (
            "a profile that is not stored",
            absent_profile_arn("absent-generation-host"),
            id.clone(),
        ),
    ];
    for (case, arn, generation_id) in not_found {
        let answer = get_generation(&client, &arn, &generation_id).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("ResourceId").and_then(Value::as_str),
            Some(format!("{arn}.{generation_id}").as_str()),
            "case {case:?}: the ResourceId names the pair as sent, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("ResourceType").and_then(Value::as_str),
            Some("GENERATION"),
            "case {case:?}: the ResourceType names the missing resource, got {answer:?}"
        );
    }

    // Values the modelled patterns refuse are refused at the boundary, before any lookup.
    for (case, arn, generation_id) in [
        ("not a UUID", profile_arn.clone(), "not-a-uuid".to_string()),
        (
            "uppercase UUID",
            profile_arn.clone(),
            absent_id.to_uppercase(),
        ),
        ("not an ARN", "not-an-arn".to_string(), id.clone()),
    ] {
        let answer = get_generation(&client, &arn, &generation_id).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a value the modelled pattern refuses is a 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
    }
}

/// Rule 6: `@idempotent`. The same start request repeated while its generation is in
/// flight answers that generation; a request differing in any one member starts its own.
///
/// Whether the first generation is still in flight when the repeat arrives is up to the
/// machine, so the assertion is on the RULE either way: a repeat answers the generation
/// already running, and a second one appears only once the first had already completed.
#[tokio::test]
async fn repeating_a_start_while_it_is_in_flight_answers_the_same_generation() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "generation-idempotent", 1).await;
    let goals = goals_of(&client, &profile_arn).await;

    for row in rows() {
        let body = row.body(&goals);
        let first = start_generation(&client, &profile_arn, body.clone()).await;
        let id = started_id(&first, row.case);
        let first_ends = instant_seconds(
            first.body.get("estimatedCompletionTime"),
            row.case,
            "estimatedCompletionTime",
        );
        let repeat = start_generation(&client, &profile_arn, body.clone()).await;
        let repeat_id = started_id(&repeat, row.case);
        if repeat_id == id {
            // The repeat landed while the first analysis was still running, so it
            // answered that one rather than starting a second.
            assert!(
                instant_seconds(repeat.body.get("createdAt"), row.case, "createdAt")
                    < first_ends,
                "case {:?}: the generation the repeat answered must be the one still in \
                 flight, got {repeat:?}",
                row.case
            );
        } else {
            // A second generation appears ONLY because the first had already completed by
            // the time the repeat arrived, which is a fresh analysis, not a lost replay.
            let repeat_started =
                instant_seconds(repeat.body.get("createdAt"), row.case, "createdAt");
            assert!(
                repeat_started >= first_ends,
                "case {:?}: a repeat of a start still in flight must answer the \
                 generation it started (in flight until {first_ends}, repeat at \
                 {repeat_started}), got {repeat:?}",
                row.case
            );
        }

        // One member at a time differs: each variant is a DIFFERENT analysis, so it gets
        // its own generation.
        let mut variants: Vec<(&str, Value)> = vec![
            ("another name", {
                let mut other = body.clone();
                other["name"] = json!("a-different-name");
                other
            }),
            ("another type", {
                let mut other = body.clone();
                let next = RECOMMENDATION_TYPES
                    .iter()
                    .find(|published| **published != row.recommendation_type)
                    .expect("another published recommendation type");
                other["types"] = json!([next]);
                other
            }),
            ("another context", {
                let mut other = body.clone();
                other["additionalContext"] = json!({ "unlike": "the first" });
                other
            }),
            ("another pillar set", {
                let mut other = body.clone();
                let extra = PILLARS
                    .iter()
                    .find(|pillar| !row.pillars.contains(pillar))
                    .copied();
                let mut pillars = row.pillars.clone();
                match extra {
                    Some(pillar) => pillars.push(pillar),
                    // The row already names every published pillar, so drop one instead.
                    None => {
                        pillars.pop();
                    }
                }
                other["scope"]["pillars"] = json!(pillars);
                other
            }),
        ];
        if !goals.is_empty() {
            let mut other = body.clone();
            other["scope"]["goalIds"] = json!([goals[goals.len() - 1]]);
            variants.push(("another goal set", other));
        }

        for (variant, variant_body) in variants {
            if variant_body == body {
                continue;
            }
            let answer = start_generation(&client, &profile_arn, variant_body.clone()).await;
            let variant_case = format!("{}/{variant}", row.case);
            assert_ne!(
                started_id(&answer, &variant_case),
                id,
                "case {variant_case:?}: a start differing in one member is its own \
                 generation, got {answer:?}"
            );
        }
    }
}

/// Rule 7: a start the service cannot serve is refused and records nothing.
#[tokio::test]
async fn a_start_the_service_cannot_serve_is_refused_as_a_validation_exception() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "generation-refusals", 1).await;
    let ineligible_arn = profile_arn_of(&client, "generation-no-scope", 0).await;
    let goals = goals_of(&client, &profile_arn).await;
    let good_body = json!({ "types": ["RESOURCE"], "scope": { "pillars": ["SECURITY"] } });

    let refusals: Vec<(&str, String, Value)> = vec![
        (
            "a profile that is not stored",
            absent_profile_arn("absent-start-host"),
            good_body.clone(),
        ),
        (
            "a profile with no aggregation scope to analyse",
            ineligible_arn.clone(),
            good_body.clone(),
        ),
        (
            "a goal id the profile does not hold",
            profile_arn.clone(),
            json!({
                "types": ["RESOURCE"],
                "scope": {
                    "pillars": ["SECURITY"],
                    "goalIds": ["abcdefab-0000-4a00-8b00-00000000000c"],
                },
            }),
        ),
        (
            "a recommendation type the enum does not publish",
            profile_arn.clone(),
            json!({ "types": ["TELEPATHY"], "scope": { "pillars": ["SECURITY"] } }),
        ),
        (
            "a pillar the enum does not publish",
            profile_arn.clone(),
            json!({ "types": ["RESOURCE"], "scope": { "pillars": ["ASTROLOGY"] } }),
        ),
        (
            "a scope naming no pillar at all",
            profile_arn.clone(),
            json!({ "types": ["RESOURCE"], "scope": { "pillars": [] } }),
        ),
        (
            "more recommendation types than the published maximum",
            profile_arn.clone(),
            json!({
                "types": ["RESOURCE", "ARCHITECTURE"],
                "scope": { "pillars": ["SECURITY"] },
            }),
        ),
        (
            "an additionalContext nested one level past the ceiling",
            profile_arn.clone(),
            json!({
                "types": ["RESOURCE"],
                "scope": { "pillars": ["SECURITY"] },
                "additionalContext": nested_document(state::MAX_DOCUMENT_DEPTH),
            }),
        ),
        (
            "an additionalContext nested far past the ceiling",
            profile_arn.clone(),
            json!({
                "types": ["RESOURCE"],
                "scope": { "pillars": ["SECURITY"] },
                "additionalContext": nested_document(state::MAX_DOCUMENT_DEPTH * 2),
            }),
        ),
    ];

    for (case, arn, body) in refusals {
        let answer = start_generation(&client, &arn, body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
    }

    // The document ceiling is a BOUND: the deepest document it allows is served, and only
    // one past it is refused (the two rows above).
    let at_the_ceiling = json!({
        "types": ["RESOURCE"],
        "scope": { "pillars": ["SECURITY"] },
        "additionalContext": nested_document(state::MAX_DOCUMENT_DEPTH - 1),
    });
    let answer = start_generation(&client, &profile_arn, at_the_ceiling.clone()).await;
    let deep_id = started_id(&answer, "an additionalContext nested to the ceiling");
    let fetched = get_generation(&client, &profile_arn, &deep_id).await;
    assert_eq!(
        fetched.body.get("additionalContext"),
        at_the_ceiling.get("additionalContext"),
        "a document nested to the ceiling round-trips as sent, got {fetched:?}"
    );

    // The refusal that names the profile's own missing scope says which member leaves it
    // ineligible — the sentence `GetAgentProfile` reports in `fieldErrors`.
    let answer = start_generation(&client, &ineligible_arn, good_body.clone()).await;
    let message = answer
        .body
        .get("Message")
        .or_else(|| answer.body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        message.contains("At least one aggregation configuration is required to generate \
                          recommendations."),
        "an ineligible profile must say what leaves it so, got {answer:?}"
    );

    // Nothing was recorded by any refusal: the eligible profile's only generation is the
    // one a successful start records now.
    let started = start_generation(&client, &profile_arn, good_body).await;
    let id = started_id(&started, "after the refusals");
    let fetched = get_generation(&client, &profile_arn, &id).await;
    assert_eq!(
        fetched.status, 200,
        "a start after the refusals must still record its generation, got {fetched:?}"
    );
    assert_eq!(
        client.store().lock().agent_generation_count(&profile_arn),
        2,
        "the refusals must have recorded no generation beside the two starts that were \
         served, got {} for {profile_arn}",
        client.store().lock().agent_generation_count(&profile_arn)
    );
    assert_eq!(
        client.store().lock().agent_generation_count(&ineligible_arn),
        0,
        "an ineligible profile holds no generation, got {}",
        client.store().lock().agent_generation_count(&ineligible_arn)
    );
    assert!(
        !goals.is_empty(),
        "the suite's goals must have been recorded for the scope rows"
    );
}

/// Rule 9: a generation does not outlive the profile it is reached through.
#[tokio::test]
async fn deleting_the_profile_takes_its_generations_with_it() {
    let client = Client::new();
    // `deletionProtection` is `@default(true)`, so a profile this test can delete has to
    // clear it on the create.
    let mut body = super::agent_goals::profile_body("generation-deleted-host", &["SECURITY"], 1);
    body["deletionProtection"] = json!(false);
    let created = super::agent_goals::create_profile(&client, body).await;
    assert_eq!(
        created.status, 200,
        "expected CreateAgentProfile 200 for the profile under test, got {created:?}"
    );
    let profile_arn = created
        .body
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let goals = goals_of(&client, &profile_arn).await;

    let mut ids: Vec<(&str, String)> = Vec::new();
    for row in rows() {
        let answer = start_generation(&client, &profile_arn, row.body(&goals)).await;
        ids.push((row.case, started_id(&answer, row.case)));
    }
    assert_eq!(
        client.store().lock().agent_generation_count(&profile_arn),
        ids.len(),
        "every row's generation must be recorded under the profile before the delete"
    );

    let deleted = delete_profile(&client, &profile_arn).await;
    assert_eq!(
        deleted.status, 200,
        "expected DeleteAgentProfile 200, got {deleted:?}"
    );

    for (case, id) in &ids {
        let answer = get_generation(&client, &profile_arn, id).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: a generation of a deleted profile must not resolve, \
             got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
    }
    let store = client.store();
    let guard = store.lock();
    assert!(
        !guard.agent_generations.contains_key(&profile_arn),
        "a deleted profile leaves no generations behind, got {} under {profile_arn}",
        guard.agent_generation_count(&profile_arn)
    );
}

/// Rule 6's foundation: the fingerprint an `@idempotent` repeat is matched on is a
/// function of the document's MEMBERS, not of a `HashMap`'s iteration order.
///
/// `Document::Object` is a `HashMap`, so two instances holding the same keys can iterate
/// in different orders; two documents built in opposite orders must still fingerprint
/// alike, or an identical repeat would quietly start a second analysis.
#[test]
fn a_document_fingerprints_by_its_members_not_by_its_map_order() {
    use ::aws_smithy_types::Document;

    /// Member names enough to make two `HashMap`s of them iterate differently.
    const NAMES: [&str; 8] = [
        "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta",
    ];

    let build = |reverse: bool| -> Document {
        let mut names: Vec<&str> = NAMES.to_vec();
        if reverse {
            names.reverse();
        }
        let mut members = std::collections::HashMap::new();
        for name in names {
            members.insert(
                name.to_string(),
                Document::Object(std::collections::HashMap::from([
                    (
                        "value".to_string(),
                        Document::String(format!("{name}-value")),
                    ),
                    ("flag".to_string(), Document::Bool(name.len() % 2 == 0)),
                ])),
            );
        }
        Document::Object(members)
    };

    let forwards = build(false);
    let backwards = build(true);
    assert_eq!(
        state::document_fingerprint(Some(&forwards)),
        state::document_fingerprint(Some(&backwards)),
        "case \"same members, opposite insertion order\": the rendering must not depend \
         on map order, got {:?} and {:?}",
        state::document_fingerprint(Some(&forwards)),
        state::document_fingerprint(Some(&backwards))
    );
    assert_eq!(
        state::parameter_fingerprint(&state::document_fingerprint(Some(&forwards))),
        state::parameter_fingerprint(&state::document_fingerprint(Some(&backwards))),
        "case \"same members, opposite insertion order\": the fingerprints must match"
    );

    // A document that differs in one member fingerprints differently, so the match is not
    // vacuous.
    let mut different = std::collections::HashMap::new();
    different.insert("alpha".to_string(), Document::String("other".to_string()));
    let different = Document::Object(different);
    assert_ne!(
        state::parameter_fingerprint(&state::document_fingerprint(Some(&forwards))),
        state::parameter_fingerprint(&state::document_fingerprint(Some(&different))),
        "case \"one member differs\": the fingerprints must differ"
    );
    assert_ne!(
        state::parameter_fingerprint(&state::document_fingerprint(Some(&forwards))),
        state::parameter_fingerprint(&state::document_fingerprint(None)),
        "case \"no document at all\": a document and no document must not fingerprint alike"
    );
}

/// Rule 8 at the state layer: the document-depth ceiling is a BOUND — the deepest
/// document it allows passes and one level past it is refused, however much deeper the
/// document goes, and the check itself costs no stack.
#[test]
fn the_document_depth_ceiling_is_a_bound() {
    use ::aws_smithy_types::Document;

    /// A document whose deepest node sits at `depth`: `depth - 1` arrays around a leaf.
    fn nested(depth: usize) -> Document {
        let mut document = Document::String("leaf".to_string());
        for _ in 1..depth {
            document = Document::Array(vec![document]);
        }
        document
    }

    for (case, depth, exceeded) in [
        ("one level", 1, false),
        ("one short of the ceiling", state::MAX_DOCUMENT_DEPTH - 1, false),
        ("exactly the ceiling", state::MAX_DOCUMENT_DEPTH, false),
        ("one past the ceiling", state::MAX_DOCUMENT_DEPTH + 1, true),
        ("far past the ceiling", state::MAX_DOCUMENT_DEPTH * 16, true),
    ] {
        let document = nested(depth);
        assert_eq!(
            state::document_depth_exceeded(Some(&document)),
            exceeded,
            "case {case:?}: a document {depth} levels deep against a ceiling of {} must \
             {}be refused",
            state::MAX_DOCUMENT_DEPTH,
            if exceeded { "" } else { "not " }
        );
    }
    assert!(
        !state::document_depth_exceeded(None),
        "case \"no document at all\": a request that carries none is never too deep"
    );
}
