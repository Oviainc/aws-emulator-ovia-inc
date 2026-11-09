// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AgentCore batch-evaluation surface —
//! `StartBatchEvaluation` and `GetBatchEvaluation`.
//!
//! The rules under test:
//!
//! * a started batch evaluation is a RESOURCE of this service, so every member of the
//!   starting request reads back through `GetBatchEvaluation` — whatever shape of job was
//!   asked for — under an id and an ARN this emulator minted for it;
//! * `BatchEvaluationStatus` is read off the CLOCK: `PENDING` until the job starts
//!   running, `IN_PROGRESS` while it is and `COMPLETED` once it has run to its end, so
//!   reading the job advances nothing and `updatedAt` names the instant of the transition
//!   that was last due;
//! * no rating is reported for a job: a rating is an evaluator's output and no judge
//!   model runs in this emulator, so `evaluationResults`, `insightResults`,
//!   `failureAnalysisResult`, `userIntentResult`, `executionSummaryResult` and
//!   `errorDetails` are absent from every read;
//! * `clientToken` is an `@idempotencyToken`: a replayed token answers with the job it
//!   already started, while two starts carrying different tokens are two jobs even under
//!   one `batchEvaluationName`;
//! * a `batchEvaluationId` no job is held for is refused 404 `ResourceNotFoundException`
//!   with the sentence this service words every missed lookup with.
//!
//! Every case drives real HTTP through the same `MultiServiceRouter` the emulator serves
//! with, so the routes, status codes, `restJson1` bodies and modelled error codes are
//! exercised and not just the handler functions.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore as agentcore;

/// A `clientToken` of the modelled length (33..256) and pattern.
const CLIENT_TOKEN: &str = "batchevalclienttoken00000000000001";
/// A second `clientToken`, so a case can tell a replay from a fresh call.
const OTHER_CLIENT_TOKEN: &str = "batchevalclienttoken00000000000002";

/// An online evaluation configuration, in the form `OnlineEvaluationConfigArn` publishes
/// as its pattern (source: smithy_query query="shape" shape="DataSourceConfig").
const ONLINE_EVALUATION_CONFIG_ARN: &str =
    "arn:aws:bedrock-agentcore:us-east-1:123456789012:online-evaluation-config/quality-0a1b2c3d4e";

/// A KMS key, in the form `KmsKeyArn` publishes as its pattern (source: smithy_query
/// query="operation" operation="StartBatchEvaluation").
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:123456789012:key/0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d";

/// The emulator, with request authentication off so a test signs nothing, plus the shared
/// engine so a case can age a stored job's clock without sleeping.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(agentcore::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Send one request and return its status and parsed JSON body (`Value::Null` for an
/// empty body). The SigV4 credential scope is what routes the request to this service.
async fn send(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(bytes::Bytes::new, |v| bytes::Bytes::from(v.to_string()));
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://bedrock-agentcore.us-east-1.amazonaws.com{path}"
        ))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/bedrock-agentcore/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// `StartBatchEvaluation`: `POST /evaluations/batch-evaluate`.
async fn start_batch_evaluation(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    send(
        router,
        "POST",
        "/evaluations/batch-evaluate",
        Some(body),
    )
    .await
}

/// `GetBatchEvaluation`: `GET /evaluations/batch-evaluate/{batchEvaluationId}`. The
/// `@httpLabel` needs no escaping — `BatchEvaluationId` admits only letters, digits, `-`
/// and `_`.
async fn get_batch_evaluation(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    send(
        router,
        "GET",
        &format!("/evaluations/batch-evaluate/{id}"),
        None,
    )
    .await
}

/// One member of a JSON object, or `Value::Null` when it is absent.
fn member<'a>(body: &'a Value, name: &str) -> &'a Value {
    body.get(name).unwrap_or(&Value::Null)
}

/// One member of a JSON object as a string, or `""`.
fn text<'a>(body: &'a Value, name: &str) -> &'a str {
    member(body, name).as_str().unwrap_or("")
}

/// The wire error code of a refusal (`__type` on `restJson1`).
fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

/// `BatchEvaluationId`'s modelled pattern,
/// `^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$`, checked without a regex dependency:
/// the discriminator is the last ten characters after the final `-`, and everything
/// before it is an identifier starting with a letter.
fn is_modelled_batch_evaluation_id(id: &str) -> bool {
    let Some((prefix, suffix)) = id.rsplit_once('-') else {
        return false;
    };
    suffix.len() == 10
        && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
        && (1..=100).contains(&prefix.len())
        && prefix.starts_with(|c: char| c.is_ascii_alphabetic())
        && prefix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A batch evaluation ARN as this service spells one: the partition, this service's
/// namespace, the region the request was signed for, the twelve-digit account and the
/// `batch-evaluate/<batchEvaluationId>` resource segment.
fn is_batch_evaluation_arn(arn: &str, id: &str) -> bool {
    let segments: Vec<&str> = arn.splitn(6, ':').collect();
    segments.len() == 6
        && segments[0] == "arn"
        && segments[1].starts_with("aws")
        && segments[2] == "bedrock-agentcore"
        && segments[3] == "us-east-1"
        && segments[4].len() == 12
        && segments[4].bytes().all(|b| b.is_ascii_digit())
        && segments[5] == format!("batch-evaluate/{id}")
}

/// Move a stored job's start instant `by` further into the past, so a case can reach the
/// far side of a lifecycle span without sleeping for it. This is the test reaching into
/// the store the handlers share, not an emulator behaviour.
fn age_batch_evaluation(engine: &engine::InMemoryEngine, id: &str, by: Duration) {
    let state = agentcore::state::service_state(engine).expect("the service state");
    let mut store = state.lock();
    let job = store
        .batch_evaluations
        .get_mut(id)
        .expect("the batch evaluation that was just started");
    job.created_at -= by;
}

/// How many batch evaluation jobs this emulator holds — the check that a replayed start
/// wrote nothing new.
fn batch_evaluation_count(engine: &engine::InMemoryEngine) -> usize {
    let state = agentcore::state::service_state(engine).expect("the service state");
    let store = state.lock();
    store.batch_evaluations.len()
}

/// A `cloudWatchLogs` data source naming the services whose traces are evaluated.
fn cloudwatch_source(service_names: &[&str], log_group_names: &[&str]) -> Value {
    json!({"cloudWatchLogs": {
        "serviceNames": service_names,
        "logGroupNames": log_group_names,
    }})
}

/// The smallest `StartBatchEvaluation` request the model admits: every `@required`
/// member and nothing else.
fn minimal_request(name: &str) -> Value {
    json!({
        "batchEvaluationName": name,
        "dataSourceConfig": cloudwatch_source(&["checkout-agent"], &["/aws/bedrock-agentcore/checkout"]),
    })
}

/// `name`, `evaluators` and `insights` as a request carries them, for a table row that
/// asks for `count` of each.
fn evaluators(count: usize) -> Value {
    // `EvaluatorId`'s three pattern alternatives: a built-in, a third-party and a custom
    // evaluator minted by the control plane (source: smithy_query query="shape"
    // shape="Evaluator").
    let ids = [
        "Builtin.Correctness",
        "Builtin.Helpfulness",
        "ThirdParty.Ragas.Faithfulness",
        "quality-0a1b2c3d4e",
        "Builtin.Completeness",
        "Builtin.Coherence",
        "Builtin.Relevance",
        "Builtin.Groundedness",
        "Builtin.ToolUse",
        "Builtin.Goal.Accuracy",
    ];
    Value::Array(
        ids.iter()
            .take(count)
            .map(|id| json!({"evaluatorId": id}))
            .collect(),
    )
}

/// `insights` as a request carries them, up to the modelled maximum of ten.
fn insights(count: usize) -> Value {
    // `InsightId`'s two pattern alternatives: a built-in insight and a custom one
    // (source: smithy_query query="shape" shape="Insight").
    let ids = [
        "Builtin.FailureAnalysis",
        "Builtin.UserIntent",
        "Builtin.ExecutionSummary",
        "custom-1a2b3c4d5e",
        "Builtin.ToolErrors",
        "Builtin.LatencyOutliers",
        "Builtin.Hallucination",
        "Builtin.Refusals",
        "Builtin.Escalations",
        "Builtin.Loops",
    ];
    Value::Array(
        ids.iter()
            .take(count)
            .map(|id| json!({"insightId": id}))
            .collect(),
    )
}

/// The round-trip table: a row per shape of batch evaluation a caller can ask for. Rows
/// differ along the name's length (`BatchEvaluationName` is
/// `^[a-zA-Z][a-zA-Z0-9_]{0,47}$`, so one character and 48 are its bounds), the
/// description (absent and both bounds of `BatchEvaluationDescription` len 1..200), the
/// member of the `DataSourceConfig` union and how much of it is filled in, the number of
/// evaluators and insights (none, one, the empty list the model admits, and the documented
/// maximum of ten), whether an output configuration, tags, a KMS key and per-session
/// evaluation metadata were supplied.
fn round_trip_cases() -> Vec<(&'static str, Value)> {
    let longest_name = "a".repeat(48);
    let longest_description = "d".repeat(200);
    vec![
        ("required-members-only", minimal_request("checkout_flow")),
        (
            "shortest-name-shortest-description-one-evaluator",
            json!({
                "batchEvaluationName": "a",
                "description": "d",
                "dataSourceConfig": cloudwatch_source(&["agent-a"], &["/aws/a"]),
                "evaluators": evaluators(1),
            }),
        ),
        (
            "longest-name-longest-description-ten-evaluators-ten-insights",
            json!({
                "batchEvaluationName": longest_name,
                "description": longest_description,
                "dataSourceConfig": cloudwatch_source(
                    &["checkout-agent"],
                    &["/aws/bedrock-agentcore/checkout", "/aws/bedrock-agentcore/search"],
                ),
                "evaluators": evaluators(10),
                "insights": insights(10),
            }),
        ),
        (
            "empty-insight-list-and-prefix-filtered-cloudwatch-source",
            json!({
                "batchEvaluationName": "support_triage",
                "dataSourceConfig": {"cloudWatchLogs": {
                    "serviceNames": ["support-agent"],
                    "logGroupNamePrefixes": ["/aws/bedrock-agentcore/support"],
                    "filterConfig": {
                        "sessionIds": ["session-01", "session-02"],
                        "timeRange": {"startTime": 1_767_225_600, "endTime": 1_767_312_000},
                        "sessionTraceIds": [{
                            "sessionId": "session-01",
                            "traceIds": ["4bf92f3577b34da6a3ce929d0e0e4736"],
                        }],
                    },
                }},
                "insights": insights(0),
                "evaluators": evaluators(2),
            }),
        ),
        (
            "online-evaluation-config-source-output-tags-kms-and-metadata",
            json!({
                "batchEvaluationName": "nightly_regression",
                "description": "re-score yesterday's sessions",
                "dataSourceConfig": {"onlineEvaluationConfigSource": {
                    "onlineEvaluationConfigArn": ONLINE_EVALUATION_CONFIG_ARN,
                    "timeRange": {"startTime": 1_767_225_600, "endTime": 1_767_312_000},
                }},
                "evaluators": evaluators(3),
                "insights": insights(1),
                "outputConfig": {"cloudWatchConfig": {
                    // `resultDestination` is an enum of DEDICATED_LOG_GROUP |
                    // SOURCE_LOG_GROUP (source: smithy_query query="shape"
                    // shape="CloudWatchOutputConfig").
                    "resultDestination": "DEDICATED_LOG_GROUP",
                    "logGroupName": "/aws/bedrock-agentcore/evaluations",
                    "logStreamName": "nightly",
                    "metricsNamespace": "AgentCore/Evaluations",
                }},
                "tags": {"team": "quality", "env": "staging"},
                "kmsKeyArn": KMS_KEY_ARN,
                "evaluationMetadata": {"sessionMetadata": [{
                    "sessionId": "session-01",
                    "testScenarioId": "refund-happy-path",
                    "groundTruth": {"inline": {
                        "assertions": [{"text": "the refund amount is stated"}],
                        "expectedTrajectory": {"toolNames": ["lookupOrder", "issueRefund"]},
                    }},
                    "metadata": {"locale": "en-US"},
                }]},
            }),
        ),
    ]
}

/// A started batch evaluation is reported from what was stored, and `GetBatchEvaluation`
/// reports the same record back — for every shape of request the model admits.
#[tokio::test]
async fn a_started_batch_evaluation_round_trips_through_get_batch_evaluation() {
    let (router, _engine) = emulator();
    let mut minted: BTreeSet<String> = BTreeSet::new();

    for (case, request) in round_trip_cases() {
        let (status, started) = start_batch_evaluation(&router, request.clone()).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from StartBatchEvaluation, got {status} with {started:?}"
        );

        let id = text(&started, "batchEvaluationId").to_string();
        assert!(
            is_modelled_batch_evaluation_id(&id),
            "case {case:?}: expected a batchEvaluationId matching ^[a-zA-Z][a-zA-Z0-9-_]{{0,99}}-[a-zA-Z0-9]{{10}}$, got {id:?}"
        );
        let requested_name = request
            .get("batchEvaluationName")
            .and_then(Value::as_str)
            .expect("every case names its job");
        assert!(
            id.starts_with(&format!("{requested_name}-")),
            "case {case:?}: expected the id to be minted from the name {requested_name:?}, got {id:?}"
        );
        assert!(
            minted.insert(id.clone()),
            "case {case:?}: expected a batchEvaluationId no earlier start minted, got {id:?}"
        );
        let arn = text(&started, "batchEvaluationArn").to_string();
        assert!(
            is_batch_evaluation_arn(&arn, &id),
            "case {case:?}: expected an ARN addressing {id:?} in the signed region, got {arn:?}"
        );
        assert_eq!(
            text(&started, "batchEvaluationName"),
            requested_name,
            "case {case:?}: expected the name the request asked for, got {started:?}"
        );
        assert_eq!(
            text(&started, "status"),
            "PENDING",
            "case {case:?}: expected a just-started job to be PENDING, got {started:?}"
        );
        let created_at = member(&started, "createdAt").clone();
        assert!(
            created_at.is_number(),
            "case {case:?}: expected createdAt as epoch seconds (smithy.api#Timestamp on restJson1), got {created_at:?}"
        );
        // Every optional member the start echoes is the one the request asked for.
        for name in ["description", "evaluators", "insights", "outputConfig", "tags", "kmsKeyArn"] {
            assert_eq!(
                member(&started, name),
                request.get(name).unwrap_or(&Value::Null),
                "case {case:?}: expected StartBatchEvaluation to echo {name} as the request asked for it, got {started:?}"
            );
        }

        // The read reports the record that was just stored.
        let (status, read) = get_batch_evaluation(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetBatchEvaluation, got {status} with {read:?}"
        );
        assert_eq!(
            text(&read, "batchEvaluationId"),
            id,
            "case {case:?}: expected the id the start minted, got {read:?}"
        );
        assert_eq!(
            text(&read, "batchEvaluationArn"),
            arn,
            "case {case:?}: expected the ARN the start minted, got {read:?}"
        );
        assert_eq!(
            text(&read, "batchEvaluationName"),
            requested_name,
            "case {case:?}: expected the stored name, got {read:?}"
        );
        assert_eq!(
            member(&read, "createdAt"),
            &created_at,
            "case {case:?}: expected the createdAt the start reported, got {read:?}"
        );
        assert_eq!(
            text(&read, "status"),
            "PENDING",
            "case {case:?}: expected the read to report the job as PENDING, got {read:?}"
        );
        for name in [
            "description",
            "evaluators",
            "insights",
            "dataSourceConfig",
            "outputConfig",
            "kmsKeyArn",
        ] {
            assert_eq!(
                member(&read, name),
                request.get(name).unwrap_or(&Value::Null),
                "case {case:?}: expected GetBatchEvaluation to report {name} as the request asked for it, got {read:?}"
            );
        }
        // No judge model and no insight analysis run here, so no rating, cluster or
        // failure is reported for any job.
        for name in [
            "evaluationResults",
            "insightResults",
            "failureAnalysisResult",
            "userIntentResult",
            "executionSummaryResult",
            "errorDetails",
        ] {
            assert_eq!(
                member(&read, name),
                &Value::Null,
                "case {case:?}: expected no {name} for a job nothing scored, got {read:?}"
            );
        }
    }
}

/// `BatchEvaluationStatus` is read off the clock, and reading a job advances nothing: a
/// row per point of the lifecycle, each asserted on two consecutive reads.
#[tokio::test]
async fn a_batch_evaluation_s_status_follows_the_clock() {
    // How far past the start instant each row reads the job, and what the lifecycle says
    // it is then: the span is 3 s per step, so the bounds of each step and one point past
    // the last are all exercised.
    let cases: [(&str, u64, &str, u64); 6] = [
        ("just started", 0, "PENDING", 0),
        ("one second in, still pending", 1, "PENDING", 0),
        ("at the first span, running", 3, "IN_PROGRESS", 3),
        ("inside the run", 5, "IN_PROGRESS", 3),
        ("at the second span, finished", 6, "COMPLETED", 6),
        ("long past the run", 600, "COMPLETED", 6),
    ];

    for (case, aged_seconds, expected_status, expected_update_offset) in cases {
        let (router, engine) = emulator();
        let (status, started) =
            start_batch_evaluation(&router, minimal_request("clock_probe")).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from StartBatchEvaluation, got {status} with {started:?}"
        );
        let id = text(&started, "batchEvaluationId").to_string();
        let created_at = member(&started, "createdAt").as_f64().unwrap_or_default();

        age_batch_evaluation(&engine, &id, Duration::from_secs(aged_seconds));

        for read_number in 1..=2 {
            let (status, read) = get_batch_evaluation(&router, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from read {read_number}, got {status} with {read:?}"
            );
            assert_eq!(
                text(&read, "status"),
                expected_status,
                "case {case:?}: expected status {expected_status} on read {read_number} at {aged_seconds}s past the start, got {read:?}"
            );
            // `createdAt` is the instant the job was started, whatever the clock has
            // since made due; the aged record's start instant is the reported one.
            let reported_created = member(&read, "createdAt").as_f64().unwrap_or_default();
            let expected_created = created_at - aged_seconds as f64;
            assert!(
                (reported_created - expected_created).abs() < 1.0,
                "case {case:?}: expected createdAt near {expected_created} on read {read_number}, got {reported_created}"
            );
            let reported_updated = member(&read, "updatedAt").as_f64().unwrap_or_default();
            let expected_updated = expected_created + expected_update_offset as f64;
            assert!(
                (reported_updated - expected_updated).abs() < 1.0,
                "case {case:?}: expected updatedAt near {expected_updated} (the last transition the clock made due) on read {read_number}, got {reported_updated}"
            );
        }
    }
}

/// A replayed `clientToken` answers with the job it already started, and two starts that
/// carry different tokens are two jobs even under one name.
#[tokio::test]
async fn a_replayed_client_token_answers_with_the_job_it_already_started() {
    let (router, engine) = emulator();

    let mut request = minimal_request("duplicate_name");
    request["clientToken"] = json!(CLIENT_TOKEN);
    let (status, first) = start_batch_evaluation(&router, request.clone()).await;
    assert_eq!(
        status, 202,
        "first start: expected HTTP 202, got {status} with {first:?}"
    );
    let first_id = text(&first, "batchEvaluationId").to_string();

    // The SAME token: the service ignores the request rather than starting a second job.
    let (status, replayed) = start_batch_evaluation(&router, request.clone()).await;
    assert_eq!(
        status, 202,
        "replayed token: expected HTTP 202, got {status} with {replayed:?}"
    );
    assert_eq!(
        text(&replayed, "batchEvaluationId"),
        first_id,
        "replayed token: expected the job the first start made, got {replayed:?}"
    );
    assert_eq!(
        batch_evaluation_count(&engine),
        1,
        "replayed token: expected the replay to have started no second job"
    );

    // A DIFFERENT token under the same name: its own job, with its own id, and both read
    // back.
    request["clientToken"] = json!(OTHER_CLIENT_TOKEN);
    let (status, second) = start_batch_evaluation(&router, request).await;
    assert_eq!(
        status, 202,
        "second token: expected HTTP 202, got {status} with {second:?}"
    );
    let second_id = text(&second, "batchEvaluationId").to_string();
    assert_ne!(
        second_id, first_id,
        "second token: expected a distinct batchEvaluationId, got {second_id:?}"
    );
    assert_eq!(
        batch_evaluation_count(&engine),
        2,
        "second token: expected two jobs to be held"
    );
    for (case, id) in [("first job", &first_id), ("second job", &second_id)] {
        let (status, read) = get_batch_evaluation(&router, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 reading {id:?}, got {status} with {read:?}"
        );
        assert_eq!(
            text(&read, "batchEvaluationName"),
            "duplicate_name",
            "case {case:?}: expected the name both starts asked for, got {read:?}"
        );
    }
}

/// A `batchEvaluationId` no job is held for is refused 404 `ResourceNotFoundException`,
/// naming the resource and the identifier that did not resolve.
#[tokio::test]
async fn a_batch_evaluation_id_no_job_is_held_for_is_refused() {
    let (router, _engine) = emulator();

    // One job exists, so the refusals below are about the id and not about an empty
    // store.
    let (status, started) = start_batch_evaluation(&router, minimal_request("held_job")).await;
    assert_eq!(
        status, 202,
        "expected HTTP 202 from StartBatchEvaluation, got {status} with {started:?}"
    );
    let held = text(&started, "batchEvaluationId").to_string();

    // Each row is an id the model admits that addresses no job: a plausible one, one
    // differing from the held job only in its discriminator, and the longest the pattern
    // allows.
    let unknown = [
        ("never started", "nightly_regression-0a1b2c3d4e".to_string()),
        (
            "same name, other discriminator",
            format!("{}-ffffffffff", held.rsplit_once('-').expect("a minted id").0),
        ),
        ("longest admissible id", format!("{}-0a1b2c3d4e", "j".repeat(89))),
    ];

    for (case, id) in unknown {
        let (status, body) = get_batch_evaluation(&router, &id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 for {id:?}, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected the modelled error code for {id:?}, got {body:?}"
        );
        assert_eq!(
            text(&body, "message"),
            format!("Batch Evaluation not found: {id}"),
            "case {case:?}: expected the service's not-found sentence for {id:?}, got {body:?}"
        );
    }

    // The job that IS held still reads back, so the refusals above removed nothing.
    let (status, read) = get_batch_evaluation(&router, &held).await;
    assert_eq!(
        status, 200,
        "expected the held job to still read back, got {status} with {read:?}"
    );
}
