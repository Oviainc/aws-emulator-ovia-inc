// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `GetRecommendation`
//! (GET `/backlog/agent-space/{agentSpaceId}/recommendations/{recommendationId}`).
//!
//! The rules under test:
//!
//! 1. A read reports the recommendation the store holds for the identifier it was
//!    given, rendered from that record — the fixed half from the recommendation, the
//!    changing half from the revision the read resolves.
//! 2. A read that names a `recommendationVersion` is served THAT revision; a read
//!    that names none is served the latest revision the recommendation has.
//! 3. A value that is not a recommendation identifier is refused with HTTP 400
//!    `ValidationException` whose message is one sentence naming the member it
//!    validated as a JSON pointer and the pattern that member must satisfy, echoing
//!    none of the rejected value — decided BEFORE anything is looked up.
//! 4. An identifier (or a version) that is well formed but names nothing in a space
//!    the caller reached is HTTP 404 `ResourceNotFoundException` naming the resource
//!    that was resolved.
//! 5. A read addressed INSIDE an agent space authorises the caller against that space
//!    first, so an `agentSpaceId` that resolves to no space fails CLOSED with HTTP 403
//!    `AccessDeniedException` and the wholly fixed message `Access denied`.
//! 6. A read changes nothing a later caller sees.
//!
//! Each table spans its class rather than one row: the identifier rows differ in
//! prefix, separator, character class, group count and group length; the served rows
//! differ in which optional members they carry, in status and priority (every value
//! the model declares), in how many revisions they have and in whether those versions
//! are contiguous; the version rows cover each stored version, each published bound of
//! a `Long` and values just outside the stored set.
//!
//! Nothing in this service's modelled surface CREATES a recommendation — the model
//! declares only Get, List and Update — so the served rows are seeded into the same
//! shared store the handler reads through, which is what makes the 200 path a read of
//! persisted state rather than a value the request carried.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_devopsagent_devopsagent::state::{
    service_state, RecommendationRecord, RecommendationRevision,
};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service's signing name (`aidevops`), which is
/// how the router resolves which service claims a request. Signature verification is
/// switched off for these tests, so the signature is a placeholder.
const AUTHORIZATION: &str = concat!(
    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-west-2/aidevops/aws4_request, ",
    "SignedHeaders=host, Signature=0123456789abcdef"
);

/// The region the credential scope above names, which is the region every ARN these
/// requests report is minted in.
const REGION: &str = "us-west-2";

/// The form a recommendation identifier takes, as the refusal states it. Written out
/// here rather than read from the service so the test pins the text itself.
const RECOMMENDATION_ID_PATTERN: &str =
    "^rec-[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$";

/// The JSON pointer the refusal names the validated member by.
const RECOMMENDATION_ID_PATH: &str = "/recommendationId";

/// The hexadecimal digits each dash-separated group of a UUID carries, which is what
/// [`RECOMMENDATION_ID_PATTERN`] states after the prefix.
const UUID_GROUP_LENGTHS: [usize; 5] = [8, 4, 4, 4, 12];

/// The values `RecommendationStatus` declares — `smithy_query(query="shape",
/// shape="RecommendationStatus")`.
const STATUS_VALUES: [&str; 6] = [
    "PROPOSED",
    "ACCEPTED",
    "REJECTED",
    "CLOSED",
    "COMPLETED",
    "UPDATE_IN_PROGRESS",
];

/// The values `RecommendationPriority` declares — `smithy_query(query="shape",
/// shape="RecommendationPriority")`.
const PRIORITY_VALUES: [&str; 3] = ["HIGH", "MEDIUM", "LOW"];

/// One emulator: a single shared engine behind the production router, handed back so a
/// test can seed the SAME store the handlers read through.
fn new_router() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_devopsagent_devopsagent::EmulatedService::new(engine.clone()),
    );
    (router, engine)
}

/// One answer as it came off the wire.
struct Answer {
    status: u16,
    /// The error shape `x-amzn-errortype` names (restJson1's error code), falling back
    /// to the body's `__type`; empty on a success.
    error_type: String,
    body: Value,
}

impl Answer {
    /// The `message` of an error answer.
    fn message(&self) -> &str {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or("<no message>")
    }
}

/// Send one request and return what came back.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body
        .as_ref()
        .map(|b| Bytes::from(serde_json::to_vec(b).expect("serialize request body")))
        .unwrap_or_default();
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "aidevops.us-west-2.amazonaws.com")
        .header("authorization", AUTHORIZATION);
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let parsed: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "response body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    let error_type = if header.is_empty() {
        parsed
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    } else {
        header
    };
    // Either source may carry a `namespace#Shape` or a trailing `:` — the shape name is
    // what names the error.
    let error_type = error_type
        .split(['#', ':'])
        .find(|part| !part.is_empty() && part.contains("Exception"))
        .unwrap_or_default()
        .to_string();
    Answer {
        status,
        error_type,
        body: parsed,
    }
}

/// Create an agent space and return its minted id.
async fn create_space(router: &MultiServiceRouter, name: &str) -> String {
    let answer = call(router, "POST", "/v1/agentspaces", Some(json!({"name": name}))).await;
    assert_eq!(
        answer.status, 201,
        "case {name:?}: expected 201, got {} with {}",
        answer.status, answer.body
    );
    answer.body["agentSpace"]["agentSpaceId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no agentSpaceId in {}", answer.body))
        .to_string()
}

/// `GET .../recommendations/{recommendation_id}`, naming `version` when given.
async fn get_recommendation(
    router: &MultiServiceRouter,
    space: &str,
    recommendation_id: &str,
    version: Option<i64>,
) -> Answer {
    let uri = match version {
        Some(version) => format!(
            "/backlog/agent-space/{space}/recommendations/{recommendation_id}\
             ?recommendationVersion={version}"
        ),
        None => format!("/backlog/agent-space/{space}/recommendations/{recommendation_id}"),
    };
    call(router, "GET", &uri, None).await
}

/// One seeded recommendation: what it is, and the revisions it has.
struct Seed {
    case: &'static str,
    recommendation_id: &'static str,
    task_id: &'static str,
    goal_id: Option<&'static str>,
    goal_version: Option<i64>,
    revisions: Vec<SeedRevision>,
}

/// One revision of a seeded recommendation: the half a later revision can change.
struct SeedRevision {
    version: i64,
    status: &'static str,
    priority: &'static str,
    spec: Option<&'static str>,
    additional_context: Option<&'static str>,
    rank_position: Option<i32>,
}

/// A fixed instant, so a seeded record carries no value that varies between runs.
fn instant(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

impl Seed {
    /// The record this seed persists.
    fn record(&self, agent_space_id: &str) -> RecommendationRecord {
        let mut revisions = BTreeMap::new();
        for revision in &self.revisions {
            let version = revision.version;
            revisions.insert(
                version,
                RecommendationRevision {
                    title: format!("{} at v{version}", self.case),
                    summary: format!("summary of {} at v{version}", self.case),
                    spec: revision.spec.map(str::to_string),
                    status: revision.status.to_string(),
                    priority: revision.priority.to_string(),
                    additional_context: revision.additional_context.map(str::to_string),
                    rank_position: revision.rank_position,
                    ranked_at: revision
                        .rank_position
                        .map(|position| instant(2_000 + u64::try_from(position).unwrap_or(0))),
                    updated_at: instant(1_000 + u64::try_from(version).unwrap_or(0)),
                },
            );
        }
        RecommendationRecord {
            recommendation_id: self.recommendation_id.to_string(),
            agent_space_id: agent_space_id.to_string(),
            task_id: self.task_id.to_string(),
            goal_id: self.goal_id.map(str::to_string),
            goal_version: self.goal_version,
            created_at: instant(500),
            revisions,
        }
    }

    /// The highest version this seed holds, which a read naming no version resolves.
    fn latest_version(&self) -> i64 {
        self.revisions
            .iter()
            .map(|revision| revision.version)
            .max()
            .expect("a seed carries at least one revision")
    }

    /// The versions this seed holds, in order.
    fn versions(&self) -> Vec<i64> {
        self.revisions
            .iter()
            .map(|revision| revision.version)
            .collect()
    }

    /// The revision `version` names.
    fn revision(&self, version: i64) -> &SeedRevision {
        self.revisions
            .iter()
            .find(|revision| revision.version == version)
            .unwrap_or_else(|| panic!("case {}: no seeded revision v{version}", self.case))
    }
}

/// One revision row, written compactly: the table below differs along what each row
/// carries, so the helpers keep the rows readable.
fn revision(
    version: i64,
    status: &'static str,
    priority: &'static str,
    spec: Option<&'static str>,
    additional_context: Option<&'static str>,
    rank_position: Option<i32>,
) -> SeedRevision {
    SeedRevision {
        version,
        status,
        priority,
        spec,
        additional_context,
        rank_position,
    }
}

/// The seeded recommendations: rows differing in which optional members they carry, in
/// how many revisions they have, and in whether those versions are contiguous.
fn seeds() -> Vec<Seed> {
    let mut rows = vec![
        Seed {
            case: "every optional member, three contiguous revisions",
            recommendation_id: "rec-0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0",
            task_id: "task-rollout-guardrails",
            goal_id: Some("goal-00000000-0000-0000-0000-000000000001"),
            goal_version: Some(7),
            revisions: vec![
                revision(
                    1,
                    "PROPOSED",
                    "HIGH",
                    Some("step one"),
                    Some("from the oncall review"),
                    Some(1),
                ),
                revision(2, "ACCEPTED", "MEDIUM", Some("step one, revised"), None, Some(3)),
                revision(3, "COMPLETED", "LOW", None, Some("closed out"), None),
            ],
        },
        Seed {
            case: "no optional member at all, one revision",
            recommendation_id: "rec-ABCDEF01-2345-6789-abcd-EF0123456789",
            task_id: "t",
            goal_id: None,
            goal_version: None,
            revisions: vec![revision(1, "REJECTED", "LOW", None, None, None)],
        },
        Seed {
            case: "non-contiguous versions, so the latest is the highest and not a count",
            recommendation_id: "rec-ffffffff-ffff-ffff-ffff-ffffffffffff",
            task_id: "task.with.dots_and-dashes",
            goal_id: Some("goal-ffffffff-ffff-ffff-ffff-ffffffffffff"),
            goal_version: Some(1),
            revisions: vec![
                revision(1, "PROPOSED", "HIGH", Some("first"), None, Some(9)),
                revision(5, "UPDATE_IN_PROGRESS", "MEDIUM", Some("fifth"), None, None),
                revision(9, "CLOSED", "HIGH", None, None, Some(2)),
            ],
        },
    ];
    // One row per declared status/priority pair, so the catalog is covered rather than
    // the two values the recorded case happened to carry.
    for (index, status) in STATUS_VALUES.iter().enumerate() {
        for (position, priority) in PRIORITY_VALUES.iter().enumerate() {
            rows.push(Seed {
                case: "one declared status and priority pair",
                // Leaked so every row's id is a `&'static str`; the suite builds a
                // bounded number of them (one per declared pair).
                recommendation_id: Box::leak(
                    format!("rec-00000000-0000-0000-{index:04x}-{position:012x}").into_boxed_str(),
                ),
                task_id: "task-catalog",
                goal_id: None,
                goal_version: None,
                revisions: vec![revision(1, status, priority, None, None, None)],
            });
        }
    }
    rows
}

/// Values that satisfy `ResourceId` (`^[a-zA-Z0-9_.-]+$`, 1..128) — so the boundary
/// validator passes them and this service's own guard is what refuses them — and are
/// not recommendation identifiers. The rows differ in prefix, separator, character
/// class, group count and group length, including each UUID group one digit short and
/// one long.
fn malformed_identifiers() -> Vec<String> {
    let mut rows = vec![
        // No prefix, a wrong prefix, the prefix alone, the prefix in the wrong case and
        // joined by the wrong separator, and the prefix not at the start.
        "00000000-0000-0000-0000-000000000000".to_string(),
        "recommendation-00000000-0000-0000-0000-000000000000".to_string(),
        "goal-00000000-0000-0000-0000-000000000000".to_string(),
        "rec".to_string(),
        "rec-".to_string(),
        "REC-00000000-0000-0000-0000-000000000000".to_string(),
        "rec_00000000-0000-0000-0000-000000000000".to_string(),
        "rec.00000000-0000-0000-0000-000000000000".to_string(),
        "my-rec-00000000-0000-0000-0000-000000000000".to_string(),
        // The prefix followed by something that is not a UUID at all, by a UUID with
        // its separators dropped, by a non-hexadecimal digit, and by too few or too
        // many groups.
        "rec-anything".to_string(),
        "rec-0000000000009999".to_string(),
        "rec-000000000000000000000000000000000000".to_string(),
        "rec-0000000z-0000-0000-0000-000000000000".to_string(),
        "rec-00000000-0000-0000-000000000000".to_string(),
        "rec-00000000-0000-0000-0000-0000-00000000000".to_string(),
    ];
    // Each group one digit short and one digit long.
    for (index, length) in UUID_GROUP_LENGTHS.iter().enumerate() {
        for adjusted in [length - 1, length + 1] {
            let groups: Vec<String> = UUID_GROUP_LENGTHS
                .iter()
                .enumerate()
                .map(|(position, group)| {
                    let width = if position == index { adjusted } else { *group };
                    "a".repeat(width)
                })
                .collect();
            rows.push(format!("rec-{}", groups.join("-")));
        }
    }
    rows
}

/// Recommendation identifiers that name no stored recommendation: the all-zero and
/// all-`f` UUIDs, a mixed-case one and a lower-case one — the pattern admits either
/// case.
fn absent_identifiers() -> Vec<String> {
    vec![
        "rec-00000000-0000-0000-0000-000000000000".to_string(),
        "rec-11111111-2222-3333-4444-555555555555".to_string(),
        "rec-AaBbCcDd-1234-5678-9abc-DEF012345678".to_string(),
        "rec-deadbeef-dead-beef-dead-beefdeadbeef".to_string(),
    ]
}

/// Identifiers that satisfy `AgentSpaceId`'s `^[a-zA-Z0-9-]{1,64}$` and name no stored
/// AgentSpace: the shortest admissible one, a digit-only one, one in the form this
/// service mints, a mixed-case hyphenated one, an all-hyphen one, and one at the
/// pattern's maximum length (from `smithy_query(query="shape", shape="AgentSpaceId")`).
fn absent_agent_space_ids() -> Vec<String> {
    vec![
        "a".to_string(),
        "7".to_string(),
        "as-0000000000000000".to_string(),
        "Mixed-Case-Agent-Space-42".to_string(),
        "---".to_string(),
        "z".repeat(64),
    ]
}

/// The refusal a value that is not a recommendation identifier gets: one sentence, the
/// validated member as a JSON pointer, and the pattern it must satisfy.
fn identifier_refusal() -> String {
    format!(
        "1 validation error detected. Value at '{RECOMMENDATION_ID_PATH}' failed to satisfy \
         constraint: Member must satisfy regular expression pattern: {RECOMMENDATION_ID_PATTERN}"
    )
}

/// The ARN a recommendation in `agent_space_id` reports its space by.
fn agent_space_arn(agent_space_id: &str) -> String {
    format!(
        "arn:aws:aidevops:{REGION}:{}:agent-space/{agent_space_id}",
        engine::emulator_account_id()
    )
}

/// Persist `seed` in `agent_space_id`, through the same store the handlers read.
fn seed_into(engine: &Arc<engine::InMemoryEngine>, agent_space_id: &str, seed: &Seed) {
    let state = service_state(&**engine).expect("the service store is reachable");
    let mut store = state.lock();
    let space = store
        .agent_spaces
        .get_mut(agent_space_id)
        .unwrap_or_else(|| panic!("agent space {agent_space_id} was created"));
    space.recommendations.insert(
        seed.recommendation_id.to_string(),
        seed.record(agent_space_id),
    );
}

/// Assert the answer reports `seed` at `version`, rendered from what was persisted.
fn assert_reports(answer: &Answer, seed: &Seed, space: &str, version: i64, case: &str) {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {}",
        answer.status, answer.body
    );
    let revision = seed.revision(version);
    let reported = &answer.body["recommendation"];
    let expected = json!({
        "agentSpaceArn": agent_space_arn(space),
        "recommendationId": seed.recommendation_id,
        "taskId": seed.task_id,
        "title": format!("{} at v{version}", seed.case),
        "status": revision.status,
        "priority": revision.priority,
        "version": version,
    });
    for (member, value) in expected.as_object().expect("an object of members") {
        assert_eq!(
            &reported[member], value,
            "case {case}: expected {member} {value}, got {:?}",
            reported[member]
        );
    }
    assert_eq!(
        reported["content"]["summary"].as_str(),
        Some(format!("summary of {} at v{version}", seed.case).as_str()),
        "case {case}: unexpected content.summary, got {:?}",
        reported["content"]
    );
    assert_eq!(
        reported["content"]["spec"].as_str(),
        revision.spec,
        "case {case}: unexpected content.spec, got {:?}",
        reported["content"]
    );
    assert_eq!(
        reported["goalId"].as_str(),
        seed.goal_id,
        "case {case}: unexpected goalId, got {:?}",
        reported["goalId"]
    );
    assert_eq!(
        reported["goalVersion"].as_i64(),
        seed.goal_version,
        "case {case}: unexpected goalVersion, got {:?}",
        reported["goalVersion"]
    );
    assert_eq!(
        reported["additionalContext"].as_str(),
        revision.additional_context,
        "case {case}: unexpected additionalContext, got {:?}",
        reported["additionalContext"]
    );
    assert_eq!(
        reported["rankPosition"].as_i64(),
        revision.rank_position.map(i64::from),
        "case {case}: unexpected rankPosition, got {:?}",
        reported["rankPosition"]
    );
    assert_eq!(
        reported["rankedAt"].is_null(),
        revision.rank_position.is_none(),
        "case {case}: rankedAt must be reported exactly when the revision is ranked, got {:?}",
        reported["rankedAt"]
    );
    assert!(
        reported["createdAt"].is_string() || reported["createdAt"].is_number(),
        "case {case}: expected a createdAt timestamp, got {:?}",
        reported["createdAt"]
    );
}

#[tokio::test]
async fn a_read_reports_the_stored_recommendation_at_the_version_it_names() {
    let (router, engine) = new_router();
    let space = create_space(&router, "recommendation-reads").await;
    for seed in seeds() {
        seed_into(&engine, &space, &seed);
    }

    for seed in seeds() {
        // Naming no version is served the LATEST revision, whatever the versions are.
        let latest = seed.latest_version();
        let answer = get_recommendation(&router, &space, seed.recommendation_id, None).await;
        assert_reports(
            &answer,
            &seed,
            &space,
            latest,
            &format!("{} ({}), no version named", seed.case, seed.recommendation_id),
        );

        // Naming a version is served THAT revision, including one an earlier revision
        // superseded.
        for version in seed.versions() {
            let answer =
                get_recommendation(&router, &space, seed.recommendation_id, Some(version)).await;
            assert_reports(
                &answer,
                &seed,
                &space,
                version,
                &format!("{} ({}), v{version}", seed.case, seed.recommendation_id),
            );
        }
    }
}

#[tokio::test]
async fn a_version_the_recommendation_does_not_have_is_not_found() {
    let (router, engine) = new_router();
    let space = create_space(&router, "recommendation-absent-versions").await;
    for seed in seeds() {
        seed_into(&engine, &space, &seed);
    }

    for seed in seeds() {
        let stored = seed.versions();
        // Zero and a negative version, each published bound of a `Long`, the version
        // just past the latest, and every gap in a non-contiguous history.
        let mut rows = vec![0, -1, i64::MIN, i64::MAX, seed.latest_version() + 1];
        rows.extend((1..=seed.latest_version()).filter(|version| !stored.contains(version)));
        for version in rows {
            let case = format!("{} ({}), v{version}", seed.case, seed.recommendation_id);
            let answer =
                get_recommendation(&router, &space, seed.recommendation_id, Some(version)).await;
            assert_eq!(
                answer.status, 404,
                "case {case}: expected 404, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_type, "ResourceNotFoundException",
                "case {case}: expected ResourceNotFoundException, got {:?} ({})",
                answer.error_type, answer.body
            );
            assert_eq!(
                answer.message(),
                format!(
                    "Recommendation not found: {} version {version}",
                    seed.recommendation_id
                ),
                "case {case}: expected the missing-version wording, got {:?}",
                answer.message()
            );
        }
    }
}

#[tokio::test]
async fn a_value_that_is_not_a_recommendation_identifier_is_refused_without_being_echoed() {
    let (router, engine) = new_router();
    let space = create_space(&router, "recommendation-identifier-validation").await;
    let seed = &seeds()[0];
    seed_into(&engine, &space, seed);

    for case in malformed_identifiers() {
        for version in [None, Some(1)] {
            let answer = get_recommendation(&router, &space, &case, version).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?} (version {version:?}): expected 400, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_type, "ValidationException",
                "case {case:?} (version {version:?}): expected ValidationException, got {:?} ({})",
                answer.error_type, answer.body
            );
            assert_eq!(
                answer.message(),
                identifier_refusal(),
                "case {case:?} (version {version:?}): expected the validation wording naming \
                 {RECOMMENDATION_ID_PATH} and the recommendation pattern, got {:?}",
                answer.message()
            );
            // No echo of the rejected value: the class states the member and the
            // pattern only. (A row that is itself a substring of the pattern text
            // cannot be told apart from the pattern, so it is exempt — the equality
            // above already pins the whole message for it.)
            if !RECOMMENDATION_ID_PATTERN.contains(case.as_str()) {
                assert!(
                    !answer.message().contains(case.as_str()),
                    "case {case:?}: the refusal must not echo the rejected value, got {:?}",
                    answer.message()
                );
            }
            let field_path = answer.body["fieldList"][0]["path"].as_str();
            assert_eq!(
                field_path,
                Some(RECOMMENDATION_ID_PATH),
                "case {case:?}: expected the field path {RECOMMENDATION_ID_PATH}, got {:?}",
                answer.body["fieldList"]
            );
        }
    }

    // The identifier form is decided before the agent space is resolved: the same
    // refusal whether or not the space exists.
    for case in malformed_identifiers() {
        for space_id in absent_agent_space_ids() {
            let answer = get_recommendation(&router, &space_id, &case, None).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?} in absent space {space_id:?}: expected 400, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.message(),
                identifier_refusal(),
                "case {case:?} in absent space {space_id:?}: unexpected refusal message, got {:?}",
                answer.message()
            );
        }
    }
}

#[tokio::test]
async fn an_identifier_that_names_nothing_in_this_space_reports_the_missing_recommendation() {
    let (router, engine) = new_router();
    let space = create_space(&router, "recommendation-absent").await;
    let other_space = create_space(&router, "recommendation-absent-other").await;
    // Stored in ANOTHER space, so the refusal is decided from what THIS space holds
    // rather than from the store being empty.
    let elsewhere = &seeds()[0];
    seed_into(&engine, &other_space, elsewhere);

    let mut rows = absent_identifiers();
    rows.push(elsewhere.recommendation_id.to_string());
    for case in rows {
        let answer = get_recommendation(&router, &space, &case, None).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?} ({})",
            answer.error_type, answer.body
        );
        assert_eq!(
            answer.message(),
            format!("Recommendation not found: {case}"),
            "case {case:?}: expected the missing-recommendation wording, got {:?}",
            answer.message()
        );
    }

    // The same identifier is served in the space that holds it, so the arm above is a
    // lookup in the store and not a blanket refusal.
    let served =
        get_recommendation(&router, &other_space, elsewhere.recommendation_id, None).await;
    assert_reports(
        &served,
        elsewhere,
        &other_space,
        elsewhere.latest_version(),
        "the recommendation in its own space",
    );
}

#[tokio::test]
async fn an_agent_space_that_resolves_to_nothing_fails_closed() {
    let (router, engine) = new_router();
    let present = create_space(&router, "recommendation-fail-closed").await;
    let stored = &seeds()[0];
    seed_into(&engine, &present, stored);

    for space_id in absent_agent_space_ids() {
        assert_ne!(
            space_id, present,
            "the table must not name the stored AgentSpace"
        );
        // Both a recommendation that exists elsewhere and one that exists nowhere, with
        // and without a version: inside an unresolvable space nothing about its
        // contents is reported.
        for recommendation_id in [
            stored.recommendation_id,
            "rec-00000000-0000-0000-0000-000000000000",
        ] {
            for version in [None, Some(1), Some(99)] {
                let case =
                    format!("{recommendation_id:?} in {space_id:?} (version {version:?})");
                let answer =
                    get_recommendation(&router, &space_id, recommendation_id, version).await;
                assert_eq!(
                    answer.status, 403,
                    "case {case}: expected 403, got {} with {}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_type, "AccessDeniedException",
                    "case {case}: expected AccessDeniedException, got {:?} ({})",
                    answer.error_type, answer.body
                );
                assert_eq!(
                    answer.message(),
                    "Access denied",
                    "case {case}: expected the fixed access-denied wording, got {:?}",
                    answer.message()
                );
                assert!(
                    !answer.message().contains(space_id.as_str()),
                    "case {case}: the refusal must disclose no identifier, got {:?}",
                    answer.message()
                );
            }
        }
    }

    // The space that does exist is still served, so the arm is decided from the store.
    let served = get_recommendation(&router, &present, stored.recommendation_id, None).await;
    assert_reports(
        &served,
        stored,
        &present,
        stored.latest_version(),
        "the stored recommendation in the space that exists",
    );
}

#[tokio::test]
async fn reading_a_recommendation_changes_nothing_a_later_caller_sees() {
    let (router, engine) = new_router();
    let space = create_space(&router, "recommendation-reads-are-reads").await;
    let seed = &seeds()[0];
    seed_into(&engine, &space, seed);

    let first = get_recommendation(&router, &space, seed.recommendation_id, None).await;
    // Reads of other versions, of absent versions and of absent identifiers in between:
    // none of them may move what the next read of the latest version reports.
    for version in [Some(1), Some(2), Some(99), None] {
        let _ = get_recommendation(&router, &space, seed.recommendation_id, version).await;
    }
    for absent in absent_identifiers() {
        let _ = get_recommendation(&router, &space, &absent, None).await;
    }
    let second = get_recommendation(&router, &space, seed.recommendation_id, None).await;
    assert_eq!(
        first.body, second.body,
        "case repeated read: the second read must report what the first did, got {} then {}",
        first.body, second.body
    );

    // And the store still holds exactly the revisions that were seeded.
    let state = service_state(&*engine).expect("the service store is reachable");
    let store = state.lock();
    let kept: Vec<i64> = store.agent_spaces[&space].recommendations[seed.recommendation_id]
        .revisions
        .keys()
        .copied()
        .collect();
    let seeded: Vec<i64> = seed.versions();
    assert_eq!(
        kept, seeded,
        "case revisions after reads: expected {seeded:?}, got {kept:?}"
    );
}
