// SPDX-License-Identifier: CC0-1.0

//! The asynchronous data-automation invocation slice, end to end over the wire.
//!
//! Every assertion here goes through `MultiServiceRouter::dispatch`, so it exercises the
//! same awsJson1_1 path a real SDK takes: `POST /` with
//! `X-Amz-Target: AmazonBedrockKeystoneRuntimeService.<Op>`.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! * `InvokeDataAutomationAsync` mints and PERSISTS one invocation per accepted
//!   submission and answers with that record's own ARN; `@idempotent` means a
//!   `clientToken` the service already accepted replays the first ARN.
//! * `GetDataAutomationStatus` computes its answer from that record, and the status
//!   progression is owed by the clock, so repeated reads never disagree.
//! * The three tagging operations edit and report the stored tag map of the invocation
//!   their `resourceARN` names.
//! * An ARN naming no stored invocation is `ResourceNotFoundException` (404); an ARN
//!   outside the modelled pattern or length is `ValidationException` (400).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomationruntime_amazonbedrockkeystoneruntimeservice as bda;
use smithy_server_runtime::MultiServiceRouter;

use bda::state::{AutomationJobStatus, Invocation, CREATED_FOR, IN_PROGRESS_FOR};

/// This service's sigv4 signing name (`OPERATIONS.md`): the credential scope the router
/// routes on.
const SIGNING_NAME: &str = "bedrock";

/// The region every request below signs for. The handler reads it out of the SigV4
/// credential scope, so it is the region in every ARN these tests expect.
const REGION: &str = "us-west-2";

/// A legal `dataAutomationProfileArn` (pattern from `smithy_query` shape
/// `DataAutomationProfileArn`, whose account segment accepts the literal `aws`).
const PROFILE_ARN: &str = "arn:aws:bedrock:us-west-2:aws:data-automation-profile/us.data-automation-v1";

/// The modelled `AutomationJobStatus` values, as `AUTOMATION_JOB_STATUS_VALUES` lists
/// them.
const JOB_STATUS_VALUES: &[&str] = &[
    "ClientError",
    "Created",
    "InProgress",
    "ServiceError",
    "Success",
];

/// One row of the submission table: the case name, the request body, the output S3 URI
/// the readers must report back, and the tags the resource must then carry.
type SubmissionCase = (
    &'static str,
    Value,
    &'static str,
    Vec<(&'static str, &'static str)>,
);

/// One row of the tag-edit script: the case name, the operation to call, its request
/// body, and the whole tag map the resource must report afterwards.
type TagEditCase = (
    &'static str,
    &'static str,
    Value,
    Vec<(&'static str, &'static str)>,
);

/// One ARN-keyed operation under test: its name, and how it carries the ARN.
type ArnKeyedOperation = (&'static str, fn(&str) -> Value);

/// A router serving only this service over one shared engine — what the production
/// router does for this crate, minus every other service.
fn service() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(bda::EmulatedService::new(engine));
    router
}

/// One wire answer: the HTTP status and the parsed JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The awsJson error code: the body's `__type`, local name only.
    fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .split('#')
            .next_back()
            .unwrap_or_default()
    }
}

/// Call one operation on `router` the way an SDK would.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "x-amz-target",
            format!("AmazonBedrockKeystoneRuntimeService.{operation}"),
        )
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("serialize the request body"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("a JSON response body")
    };
    Answer { status, body }
}

/// Submit one invocation and return its `invocationArn`.
async fn submit(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let answer = call(router, "InvokeDataAutomationAsync", body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from InvokeDataAutomationAsync, got {} / {}",
        answer.status, answer.body
    );
    answer.body["invocationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no invocationArn in {}", answer.body))
        .to_string()
}

/// The smallest legal submission body, parameterised by the output prefix.
fn minimal_submission(output_s3_uri: &str) -> Value {
    json!({
        "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/doc.pdf"},
        "outputConfiguration": {"s3Uri": output_s3_uri},
        "dataAutomationProfileArn": PROFILE_ARN,
    })
}

/// Check `arn` against the modelled `InvocationArn` shape — pattern
/// `^arn:aws(|-cn|-iso|-iso-[a-z]|-us-gov):bedrock:[a-zA-Z0-9-]*:[0-9]{12}:data-automation-invocation/[a-zA-Z0-9-_]+$`
/// and `@length(1..128)`, from `smithy_query` shape `InvocationArn`. Checked
/// structurally so the test needs no regex engine.
fn assert_invocation_arn(case: &str, arn: &str) {
    let prefix = format!(
        "arn:aws:bedrock:{REGION}:{}:data-automation-invocation/",
        emulator_server::EMULATOR_ACCOUNT_ID
    );
    let Some(id) = arn.strip_prefix(prefix.as_str()) else {
        panic!("case {case:?}: expected an ARN starting {prefix:?}, got {arn:?}");
    };
    assert!(
        !id.is_empty()
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "case {case:?}: the resource id must match [a-zA-Z0-9-_]+, got {id:?}"
    );
    assert!(
        !arn.is_empty() && arn.len() <= 128,
        "case {case:?}: InvocationArn is @length(1..128), got {} chars in {arn:?}",
        arn.len()
    );
}

/// A timestamp from a response as seconds since the epoch. The member carries
/// `timestampFormat: date-time`, but accept the epoch-seconds form too so the assertion
/// is about the instant, not the encoding.
fn epoch_seconds(case: &str, value: &Value) -> f64 {
    match value {
        Value::String(text) => aws_smithy_types::DateTime::from_str(
            text,
            aws_smithy_types::date_time::Format::DateTime,
        )
        .unwrap_or_else(|e| panic!("case {case:?}: {text:?} is not an RFC 3339 timestamp: {e}"))
        .as_secs_f64(),
        Value::Number(number) => number
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: {number} is not a number of seconds")),
        other => panic!("case {case:?}: expected a timestamp, got {other}"),
    }
}

/// The tags `ListTagsForResource` reports, as (key, value) pairs in the order answered.
async fn list_tags(router: &MultiServiceRouter, case: &str, arn: &str) -> Vec<(String, String)> {
    let answer = call(router, "ListTagsForResource", json!({"resourceARN": arn})).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from ListTagsForResource, got {} / {}",
        answer.status, answer.body
    );
    answer.body["tags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no tags array in {}", answer.body))
        .iter()
        .map(|tag| {
            (
                tag["key"].as_str().unwrap_or_default().to_string(),
                tag["value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Rank of a status in the progression, for the monotonicity assertion.
fn progression_rank(status: &str) -> usize {
    match status {
        "Created" => 0,
        "InProgress" => 1,
        "Success" => 2,
        other => panic!("{other:?} is not part of the success progression"),
    }
}

/// A fixed submission instant, so the clock table below is deterministic.
fn fixed_submission_time() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// An invocation submitted at [`fixed_submission_time`], for the pure clock tests.
fn fixed_invocation() -> Invocation {
    Invocation::new(
        "arn:aws:bedrock:us-west-2:000000000000:data-automation-invocation/fixture".to_string(),
        "fixture".to_string(),
        "s3://in-bucket/assets/doc.pdf".to_string(),
        "s3://out-bucket/results/".to_string(),
        PROFILE_ARN.to_string(),
        fixed_submission_time(),
    )
}

#[test]
fn advance_to_materialises_only_the_transitions_the_clock_owes() {
    let submitted = fixed_submission_time();
    let in_progress_due = CREATED_FOR;
    let success_due = CREATED_FOR + IN_PROGRESS_FOR;

    // Every row is "how long after the submission the job is observed" -> the status the
    // clock owes it by then, and whether a completion time is owed with it.
    let rows: &[(&str, Duration, AutomationJobStatus, bool)] = &[
        ("observed at the submission instant", Duration::ZERO, AutomationJobStatus::Created, false),
        ("one tick before InProgress is due", in_progress_due - Duration::from_millis(1), AutomationJobStatus::Created, false),
        ("exactly when InProgress is due", in_progress_due, AutomationJobStatus::InProgress, false),
        ("one tick before Success is due", success_due - Duration::from_millis(1), AutomationJobStatus::InProgress, false),
        ("exactly when Success is due", success_due, AutomationJobStatus::Success, true),
        ("long after Success is due", success_due + Duration::from_secs(3_600), AutomationJobStatus::Success, true),
    ];

    for (case, after, want_status, want_completion) in rows {
        let mut invocation = fixed_invocation();
        invocation.advance_to(submitted + *after);
        assert_eq!(
            invocation.status, *want_status,
            "case {case:?}: expected {want_status:?} {after:?} after submission, got {:?}",
            invocation.status
        );
        assert_eq!(
            invocation.job_completion_time.is_some(),
            *want_completion,
            "case {case:?}: completion time presence wrong, got {:?}",
            invocation.job_completion_time
        );
        if *want_completion {
            // The instant stored is the instant the transition became DUE, so it does
            // not depend on when a caller happened to look.
            assert_eq!(
                invocation.job_completion_time,
                Some(submitted + success_due),
                "case {case:?}: the completion instant must be when Success fell due, got {:?}",
                invocation.job_completion_time
            );
        }
    }

    // Idempotence of the materialisation: once a transition is recorded, re-applying the
    // clock — even at an earlier instant — never moves the job back.
    let mut invocation = fixed_invocation();
    invocation.advance_to(submitted + success_due);
    let completed_at = invocation.job_completion_time;
    for case in ["re-read at the submission instant", "re-read far in the future"] {
        let now = if case.contains("submission") {
            submitted
        } else {
            submitted + Duration::from_secs(86_400)
        };
        invocation.advance_to(now);
        assert_eq!(
            invocation.status,
            AutomationJobStatus::Success,
            "case {case:?}: a recorded transition must not be undone"
        );
        assert_eq!(
            invocation.job_completion_time, completed_at,
            "case {case:?}: the completion instant must stay put"
        );
    }
}

#[test]
fn job_duration_in_seconds_is_derived_from_the_two_stored_timestamps() {
    let submitted = fixed_submission_time();

    // Whole seconds between submission and completion, truncated — the duration is never
    // stored, so each row only sets the two timestamps.
    let rows: &[(&str, Option<Duration>, Option<i32>)] = &[
        ("still running", None, None),
        ("completed inside the first second", Some(Duration::from_millis(600)), Some(0)),
        ("completed just past one second", Some(Duration::from_millis(1_500)), Some(1)),
        ("completed after a minute and a half", Some(Duration::from_secs(90)), Some(90)),
        ("completed after a day", Some(Duration::from_secs(86_400)), Some(86_400)),
    ];

    for (case, elapsed, want) in rows {
        let mut invocation = fixed_invocation();
        invocation.job_completion_time = elapsed.map(|elapsed| submitted + elapsed);
        assert_eq!(
            invocation.job_duration_in_seconds(),
            *want,
            "case {case:?}: expected {want:?} seconds for an elapsed {elapsed:?}, got {:?}",
            invocation.job_duration_in_seconds()
        );
    }
}

#[tokio::test]
async fn each_submission_is_persisted_and_reported_back_from_state() {
    let router = service();

    // One row per shape of submission the model allows: the bare required members, and
    // then each optional member group added. What differs between rows is what the
    // operation must persist, which is what the two readers below report.
    let rows: Vec<SubmissionCase> = vec![
        (
            "only the three required members",
            minimal_submission("s3://out-bucket/plain/"),
            "s3://out-bucket/plain/",
            vec![],
        ),
        (
            "a project configuration and one tag",
            json!({
                "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/scan.tiff"},
                "outputConfiguration": {"s3Uri": "s3://out-bucket/project/"},
                "dataAutomationProfileArn": PROFILE_ARN,
                "dataAutomationConfiguration": {
                    "dataAutomationProjectArn": "arn:aws:bedrock:us-west-2:000000000000:data-automation-project/invoices",
                    "stage": "LIVE",
                },
                "tags": [{"key": "env", "value": "dev"}],
            }),
            "s3://out-bucket/project/",
            vec![("env", "dev")],
        ),
        (
            "blueprints, encryption, notification and three tags",
            json!({
                "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/clip.mp4"},
                "outputConfiguration": {"s3Uri": "s3://out-bucket/deep/nested/prefix/"},
                "dataAutomationProfileArn": PROFILE_ARN,
                "blueprints": [{
                    "blueprintArn": "arn:aws:bedrock:us-west-2:aws:blueprint/bedrock-data-automation-public-video",
                    "stage": "LIVE",
                }],
                "encryptionConfiguration": {"kmsKeyId": "alias/data-automation"},
                "notificationConfiguration": {"eventBridgeConfiguration": {"eventBridgeEnabled": true}},
                "tags": [
                    {"key": "team", "value": "search"},
                    {"key": "env", "value": "prod"},
                    {"key": "cost-center", "value": "4711"},
                ],
            }),
            "s3://out-bucket/deep/nested/prefix/",
            vec![("cost-center", "4711"), ("env", "prod"), ("team", "search")],
        ),
        (
            "an output uri that is a bucket root",
            minimal_submission("s3://out-bucket"),
            "s3://out-bucket",
            vec![],
        ),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, body, want_output_uri, want_tags) in &rows {
        let before = SystemTime::now();
        let arn = submit(&router, case, body.clone()).await;
        let after = SystemTime::now();
        assert_invocation_arn(case, &arn);
        minted.push(arn.clone());

        let answer = call(&router, "GetDataAutomationStatus", json!({"invocationArn": arn})).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 from GetDataAutomationStatus, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["outputConfiguration"]["s3Uri"].as_str(),
            Some(*want_output_uri),
            "case {case:?}: the reported outputConfiguration must be the one submitted, got {}",
            answer.body
        );
        let status = answer.body["status"].as_str().unwrap_or_default();
        assert!(
            JOB_STATUS_VALUES.contains(&status),
            "case {case:?}: {status:?} is not a modelled AutomationJobStatus"
        );

        // The submission time is the instant this call was accepted, so it falls inside
        // the window the call itself spanned.
        let submitted = epoch_seconds(case, &answer.body["jobSubmissionTime"]);
        let low = before
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("after the epoch")
            .as_secs_f64();
        let high = after
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("after the epoch")
            .as_secs_f64();
        assert!(
            submitted >= low - 1.0 && submitted <= high + 1.0,
            "case {case:?}: jobSubmissionTime {submitted} is outside the submit window {low}..{high}"
        );

        let tags = list_tags(&router, case, &arn).await;
        let want: Vec<(String, String)> = want_tags
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        assert_eq!(
            tags, want,
            "case {case:?}: ListTagsForResource must report exactly the submitted tags"
        );
    }

    // Distinct submissions are distinct jobs: no two share an ARN.
    let mut sorted = minted.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        minted.len(),
        "each submission must mint its own invocationArn, got {minted:?}"
    );
}

#[tokio::test]
async fn an_accepted_client_token_replays_the_one_invocation() {
    let router = service();

    // `clientToken` is `@idempotencyToken` on an `@idempotent` operation, so the token
    // identifies the submission. Rows differ in the token's shape, which the modelled
    // pattern `^[a-zA-Z0-9](-*[a-zA-Z0-9]){1,256}$` allows.
    let tokens = ["tok-1", "a1", "Zz9", "many-dashes-in-here"];

    let mut per_token: Vec<String> = Vec::new();
    for token in tokens {
        let body = json!({
            "clientToken": token,
            "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/doc.pdf"},
            "outputConfiguration": {"s3Uri": "s3://out-bucket/first/"},
            "dataAutomationProfileArn": PROFILE_ARN,
        });
        let first = submit(&router, token, body.clone()).await;
        let replay = submit(&router, token, body).await;
        assert_eq!(
            first, replay,
            "case {token:?}: an identical resubmission must replay the same invocationArn"
        );

        // A reused token names the job it already minted, so nothing the second request
        // carries can create a second one.
        let second_payload = json!({
            "clientToken": token,
            "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/other.pdf"},
            "outputConfiguration": {"s3Uri": "s3://out-bucket/second/"},
            "dataAutomationProfileArn": PROFILE_ARN,
        });
        let third = submit(&router, token, second_payload).await;
        assert_eq!(
            first, third,
            "case {token:?}: a reused token must not mint a second job"
        );

        let answer = call(
            &router,
            "GetDataAutomationStatus",
            json!({"invocationArn": first}),
        )
        .await;
        assert_eq!(
            answer.body["outputConfiguration"]["s3Uri"].as_str(),
            Some("s3://out-bucket/first/"),
            "case {token:?}: the stored job must stay the one the first call persisted, got {}",
            answer.body
        );
        per_token.push(first);
    }

    per_token.sort();
    per_token.dedup();
    assert_eq!(
        per_token.len(),
        tokens.len(),
        "distinct tokens are distinct jobs, got {per_token:?}"
    );

    // No token at all: the SDK would have filled one in, so two bare calls are two jobs.
    let untokened_a = submit(&router, "no token (a)", minimal_submission("s3://out-bucket/a/")).await;
    let untokened_b = submit(&router, "no token (b)", minimal_submission("s3://out-bucket/b/")).await;
    assert_ne!(
        untokened_a, untokened_b,
        "a submission without a clientToken must mint a new job each time"
    );
}

#[tokio::test]
async fn status_reads_agree_with_each_other_and_reach_success() {
    let router = service();
    let case = "a single submission polled to completion";
    let arn = submit(&router, case, minimal_submission("s3://out-bucket/poll/")).await;

    let read = |arn: String| {
        let router = &router;
        async move {
            let answer = call(
                router,
                "GetDataAutomationStatus",
                json!({"invocationArn": arn}),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200 from GetDataAutomationStatus, got {} / {}",
                answer.status, answer.body
            );
            answer.body
        }
    };

    // The first read happens straight after the submit. That the job is NOT born
    // finished is pinned deterministically by
    // `advance_to_materialises_only_the_transitions_the_clock_owes`, with injected
    // instants; over the wire the assertion is the invariant that holds whatever the
    // machine's load did to the 300 ms window — a job without a terminal status reports
    // neither a completion time nor a duration.
    let first = read(arn.clone()).await;
    let first_status = first["status"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no status in {first}"))
        .to_string();
    assert!(
        JOB_STATUS_VALUES.contains(&first_status.as_str()),
        "case {case:?}: {first_status:?} is not a modelled AutomationJobStatus"
    );
    if first_status != "Success" {
        assert!(
            first.get("jobCompletionTime").is_none_or(Value::is_null),
            "case {case:?}: a job in {first_status:?} has no completion time, got {first}"
        );
        assert!(
            first.get("jobDurationInSeconds").is_none_or(Value::is_null),
            "case {case:?}: a job in {first_status:?} has no duration, got {first}"
        );
    }

    // Poll until the clock owes Success. The observed statuses may skip a step, but they
    // must never go backwards.
    let mut observed = vec![first_status];
    let deadline = SystemTime::now() + Duration::from_secs(10);
    let mut final_body = first;
    while SystemTime::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
        final_body = read(arn.clone()).await;
        let status = final_body["status"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no status in {final_body}"))
            .to_string();
        if observed.last() != Some(&status) {
            observed.push(status.clone());
        }
        if status == "Success" {
            break;
        }
    }
    assert!(
        observed
            .windows(2)
            .all(|pair| progression_rank(&pair[0]) < progression_rank(&pair[1])),
        "case {case:?}: the status must only move forwards, observed {observed:?}"
    );
    assert_eq!(
        final_body["status"].as_str(),
        Some("Success"),
        "case {case:?}: the job must reach Success, observed {observed:?}"
    );

    // Once terminal, the record is fixed: every later read answers identically.
    let submitted = epoch_seconds(case, &final_body["jobSubmissionTime"]);
    let completed = epoch_seconds(case, &final_body["jobCompletionTime"]);
    let duration = final_body["jobDurationInSeconds"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: no jobDurationInSeconds in {final_body}"));
    assert!(
        completed > submitted,
        "case {case:?}: completion {completed} must follow submission {submitted}"
    );
    #[allow(clippy::cast_possible_truncation)]
    let want_duration = (completed - submitted).floor() as i64;
    assert_eq!(
        duration, want_duration,
        "case {case:?}: jobDurationInSeconds must be the whole seconds between {submitted} and {completed}"
    );
    for round in 0..3 {
        tokio::time::sleep(Duration::from_millis(60)).await;
        let again = read(arn.clone()).await;
        assert_eq!(
            again["status"], final_body["status"],
            "case {case:?} re-read {round}: a read must not change what a later caller sees"
        );
        assert_eq!(
            again["jobSubmissionTime"], final_body["jobSubmissionTime"],
            "case {case:?} re-read {round}: the submission time must be stable"
        );
        assert_eq!(
            again["jobCompletionTime"], final_body["jobCompletionTime"],
            "case {case:?} re-read {round}: the completion time must be stable"
        );
        assert_eq!(
            again["jobDurationInSeconds"], final_body["jobDurationInSeconds"],
            "case {case:?} re-read {round}: the derived duration must be stable"
        );
    }
}

#[tokio::test]
async fn every_arn_keyed_operation_refuses_unknown_and_malformed_arns() {
    let router = service();
    let account = emulator_server::EMULATOR_ACCOUNT_ID;

    // The four operations that resolve a caller-supplied invocation ARN, with the member
    // each one carries it in.
    let operations: &[ArnKeyedOperation] = &[
        ("GetDataAutomationStatus", |arn| {
            json!({"invocationArn": arn})
        }),
        ("ListTagsForResource", |arn| json!({"resourceARN": arn})),
        ("TagResource", |arn| {
            json!({"resourceARN": arn, "tags": [{"key": "env", "value": "dev"}]})
        }),
        ("UntagResource", |arn| {
            json!({"resourceARN": arn, "tagKeys": ["env"]})
        }),
    ];

    // Well formed under the modelled pattern, but naming nothing this emulator minted.
    // Rows differ in region, partition and resource-id shape.
    let unknown = [
        format!("arn:aws:bedrock:{REGION}:{account}:data-automation-invocation/does-not-exist"),
        format!("arn:aws:bedrock:eu-west-1:{account}:data-automation-invocation/0"),
        format!("arn:aws-us-gov:bedrock:us-gov-west-1:{account}:data-automation-invocation/a_B-9"),
        format!("arn:aws-cn:bedrock:cn-north-1:{account}:data-automation-invocation/{}", "x".repeat(40)),
    ];
    // Outside the modelled pattern or length: refused by the boundary, before any
    // lookup. Rows differ in which part of the ARN is wrong.
    let malformed = [
        "not-an-arn".to_string(),
        format!("arn:aws:bedrock:{REGION}:{account}:data-automation-project/p"),
        format!("arn:aws:bedrock:{REGION}:12345:data-automation-invocation/x"),
        format!("arn:aws:bedrock:{REGION}:{account}:data-automation-invocation/"),
        format!("arn:aws:s3:{REGION}:{account}:data-automation-invocation/x"),
    ];

    for (operation, build) in operations {
        for arn in &unknown {
            let case = format!("{operation} on unknown {arn}");
            let answer = call(&router, operation, build(arn)).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected 404, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {}",
                answer.body
            );
        }
        for arn in &malformed {
            let case = format!("{operation} on malformed {arn}");
            let answer = call(&router, operation, build(arn)).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {}",
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn tag_and_untag_edit_the_stored_tag_map() {
    let router = service();
    let case = "tag edits on one invocation";
    let arn = submit(
        &router,
        case,
        json!({
            "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/doc.pdf"},
            "outputConfiguration": {"s3Uri": "s3://out-bucket/tags/"},
            "dataAutomationProfileArn": PROFILE_ARN,
            "tags": [{"key": "env", "value": "dev"}],
        }),
    )
    .await;

    // Each row is one edit applied to the tag map built by the rows before it, and the
    // whole map the resource must then report.
    let rows: Vec<TagEditCase> = vec![
        (
            "a new key is added and an existing key takes the new value",
            "TagResource",
            json!({"resourceARN": arn, "tags": [
                {"key": "team", "value": "search"},
                {"key": "env", "value": "prod"},
            ]}),
            vec![("env", "prod"), ("team", "search")],
        ),
        (
            "tagging with an empty list changes nothing",
            "TagResource",
            json!({"resourceARN": arn, "tags": []}),
            vec![("env", "prod"), ("team", "search")],
        ),
        (
            "an empty tag value is a value, not an absence",
            "TagResource",
            json!({"resourceARN": arn, "tags": [{"key": "note", "value": ""}]}),
            vec![("env", "prod"), ("note", ""), ("team", "search")],
        ),
        (
            "untagging removes exactly the named keys",
            "UntagResource",
            json!({"resourceARN": arn, "tagKeys": ["team", "note"]}),
            vec![("env", "prod")],
        ),
        (
            "untagging a key the resource does not carry is a no-op success",
            "UntagResource",
            json!({"resourceARN": arn, "tagKeys": ["never-set"]}),
            vec![("env", "prod")],
        ),
        (
            "untagging every key leaves an empty tag list",
            "UntagResource",
            json!({"resourceARN": arn, "tagKeys": ["env"]}),
            vec![],
        ),
        (
            "a key removed can be set again",
            "TagResource",
            json!({"resourceARN": arn, "tags": [{"key": "env", "value": "staging"}]}),
            vec![("env", "staging")],
        ),
    ];

    for (row, operation, body, want) in &rows {
        let answer = call(&router, operation, body.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {row:?}: expected 200 from {operation}, got {} / {}",
            answer.status, answer.body
        );
        let tags = list_tags(&router, row, &arn).await;
        let expected: Vec<(String, String)> = want
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        assert_eq!(
            tags, expected,
            "case {row:?}: ListTagsForResource must report the edited map"
        );
        let mut sorted = tags.clone();
        sorted.sort();
        assert_eq!(
            tags, sorted,
            "case {row:?}: tags must be reported in a stable key order"
        );
    }
}

#[tokio::test]
async fn tagging_past_the_modelled_tag_bound_is_refused_and_changes_nothing() {
    let router = service();
    let case = "a resource already carrying the maximum number of tags";

    // The modelled `TagList` bound is `@length(min: 0, max: 200)` (smithy_query shape
    // `TagList`), so a resource can hold 200 tags and no more.
    let full: Vec<Value> = (0..200)
        .map(|i| json!({"key": format!("key{i:03}"), "value": format!("value{i:03}")}))
        .collect();
    let arn = submit(
        &router,
        case,
        json!({
            "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/doc.pdf"},
            "outputConfiguration": {"s3Uri": "s3://out-bucket/quota/"},
            "dataAutomationProfileArn": PROFILE_ARN,
            "tags": full,
        }),
    )
    .await;
    assert_eq!(
        list_tags(&router, case, &arn).await.len(),
        200,
        "case {case:?}: the submission must persist all 200 tags"
    );

    // Overwriting an existing key keeps the count at the bound, so it is allowed.
    let overwrite = call(
        &router,
        "TagResource",
        json!({"resourceARN": arn, "tags": [{"key": "key000", "value": "rewritten"}]}),
    )
    .await;
    assert_eq!(
        overwrite.status, 200,
        "case {case:?}: overwriting a key does not grow the map, got {} / {}",
        overwrite.status, overwrite.body
    );
    let tags = list_tags(&router, case, &arn).await;
    assert_eq!(tags.len(), 200, "case {case:?}: the count must stay at 200");
    assert!(
        tags.contains(&("key000".to_string(), "rewritten".to_string())),
        "case {case:?}: the overwrite must be visible"
    );

    // One more distinct key would take it past the bound.
    let over = call(
        &router,
        "TagResource",
        json!({"resourceARN": arn, "tags": [{"key": "one-too-many", "value": "x"}]}),
    )
    .await;
    assert_eq!(
        over.status, 402,
        "case {case:?}: expected 402, got {} / {}",
        over.status, over.body
    );
    assert_eq!(
        over.error_code(),
        "ServiceQuotaExceededException",
        "case {case:?}: expected ServiceQuotaExceededException, got {}",
        over.body
    );
    let after = list_tags(&router, case, &arn).await;
    assert_eq!(
        after, tags,
        "case {case:?}: a refused TagResource must leave the stored tags untouched"
    );
}

#[test]
fn the_invocation_store_is_bounded_and_retires_finished_jobs_first() {
    use service_com_amazonaws_bedrockdataautomationruntime_amazonbedrockkeystoneruntimeservice::ComAmazonawsBedrockdataautomationruntimeAmazonbedrockkeystoneruntimeserviceState as Store;

    // `now` is an hour past the fixture epoch: a record submitted back then has long
    // since finished, while one submitted AT `now` is still `Created`.
    let epoch = fixed_submission_time();
    let now = epoch + Duration::from_secs(3_600);

    /// One invocation record with index `i`, submitted at `submitted`, carrying its own
    /// `clientToken` — the shape `InvokeDataAutomationAsync` admits.
    fn record(i: usize, submitted: SystemTime) -> Invocation {
        let arn =
            format!("arn:aws:bedrock:us-west-2:000000000000:data-automation-invocation/job{i:05}");
        let mut invocation = Invocation::new(
            arn,
            format!("job{i:05}"),
            "s3://in-bucket/assets/doc.pdf".to_string(),
            "s3://out-bucket/results/".to_string(),
            PROFILE_ARN.to_string(),
            submitted,
        );
        invocation.client_token = Some(format!("token{i:05}"));
        invocation
    }

    /// Fill `store` to capacity through the ONE admission path. The first `running`
    /// records are submitted at `now`, so they are still pending; the rest are submitted
    /// in the past in index order, so index `running` is the longest-submitted finished
    /// record. Returns the ARNs in index order.
    fn fill(store: &mut Store, running: usize, epoch: SystemTime, now: SystemTime) -> Vec<String> {
        let mut arns = Vec::new();
        for i in 0..bda::state::MAX_INVOCATIONS {
            let submitted = if i < running {
                now
            } else {
                epoch + Duration::from_millis(i as u64)
            };
            let invocation = record(i, submitted);
            let arn = invocation.invocation_arn.clone();
            assert!(
                store.admit_invocation(invocation, now),
                "filling below capacity must always find a slot (record {i})"
            );
            arns.push(arn);
        }
        arns
    }

    // Rows differ in how many of the records at capacity are still running: the policy
    // must retire the longest-submitted FINISHED record, and refuse only when there is
    // no finished record to retire.
    let rows: &[(&str, usize, bool)] = &[
        ("every record finished", 0, true),
        ("the first record still running", 1, true),
        ("all but one still running", bda::state::MAX_INVOCATIONS - 1, true),
        ("every record still running", bda::state::MAX_INVOCATIONS, false),
    ];

    for (case, running, want_room) in rows {
        let mut store = Store::default();
        let arns = fill(&mut store, *running, epoch, now);
        assert_eq!(
            store.invocation_count(),
            bda::state::MAX_INVOCATIONS,
            "case {case:?}: the fixture must start at capacity"
        );

        // One more submission, at capacity: it is admitted only if a finished record can
        // be retired to make room for it.
        let newcomer = record(bda::state::MAX_INVOCATIONS, now);
        let newcomer_arn = newcomer.invocation_arn.clone();
        let admitted = store.admit_invocation(newcomer, now);
        assert_eq!(
            admitted, *want_room,
            "case {case:?}: expected admit_invocation to answer {want_room}"
        );

        // Whatever the answer, NEITHER map may exceed the cap: the token map is admitted
        // with its record and retired with it, so a stream of distinct tokens cannot
        // allocate past the bound.
        assert!(
            store.invocation_count() <= bda::state::MAX_INVOCATIONS,
            "case {case:?}: invocations must stay bounded, got {}",
            store.invocation_count()
        );
        assert!(
            store.client_token_count() <= store.invocation_count(),
            "case {case:?}: client_tokens must never outgrow invocations, got {} vs {}",
            store.client_token_count(),
            store.invocation_count()
        );

        if *want_room {
            assert_eq!(
                store.invocation_count(),
                bda::state::MAX_INVOCATIONS,
                "case {case:?}: exactly one record must be retired to seat the newcomer"
            );
            assert!(
                store.invocation(&newcomer_arn).is_some(),
                "case {case:?}: the admitted record must be retained"
            );
            // The retired record is the longest-submitted finished one — the first
            // record that was not left running.
            let retired = &arns[*running];
            assert!(
                store.invocation(retired).is_none(),
                "case {case:?}: {retired:?} was the longest-submitted finished record"
            );
            // Its `clientToken` went with it: a token never outlives the job it minted,
            // which is what keeps the token map from accumulating dead entries.
            assert!(
                store
                    .replay_for_client_token(&format!("token{running:05}"))
                    .is_none(),
                "case {case:?}: the retired record's clientToken must not replay"
            );
        } else {
            assert_eq!(
                store.invocation_count(),
                bda::state::MAX_INVOCATIONS,
                "case {case:?}: a job still running must never be dropped"
            );
            assert!(
                store.invocation(&newcomer_arn).is_none(),
                "case {case:?}: a refused submission must insert nothing"
            );
            assert!(
                store
                    .replay_for_client_token(&format!("token{:05}", bda::state::MAX_INVOCATIONS))
                    .is_none(),
                "case {case:?}: a refused submission must leave no clientToken behind"
            );
        }
    }
}

#[test]
fn a_stream_of_distinct_client_tokens_cannot_grow_the_store_past_its_bound() {
    use service_com_amazonaws_bedrockdataautomationruntime_amazonbedrockkeystoneruntimeservice::ComAmazonawsBedrockdataautomationruntimeAmazonbedrockkeystoneruntimeserviceState as Store;

    // The adversarial pattern the bound exists for: every request carries a FRESH
    // `clientToken`, so none of them replays and each one admits a new record. Both maps
    // must stay at or below the cap however long the stream runs, and no token may
    // survive the record it named.
    let epoch = fixed_submission_time();
    let submissions = bda::state::MAX_INVOCATIONS + 250;
    let mut store = Store::default();
    let mut tokens = Vec::new();

    for i in 0..submissions {
        // Each record is submitted far enough in the past to have finished, so the
        // retirement policy always has a candidate; `now` keeps pace with the stream.
        let submitted = epoch + Duration::from_millis(i as u64);
        let now = submitted + CREATED_FOR + IN_PROGRESS_FOR;
        let token = format!("stream-token-{i:06}");
        let arn = format!(
            "arn:aws:bedrock:us-west-2:000000000000:data-automation-invocation/stream{i:06}"
        );
        let mut invocation = Invocation::new(
            arn,
            format!("stream{i:06}"),
            "s3://in-bucket/assets/doc.pdf".to_string(),
            "s3://out-bucket/results/".to_string(),
            PROFILE_ARN.to_string(),
            submitted,
        );
        invocation.client_token = Some(token.clone());
        assert!(
            store.admit_invocation(invocation, now),
            "submission {i}: a finished record was available to retire"
        );
        tokens.push(token);

        assert!(
            store.invocation_count() <= bda::state::MAX_INVOCATIONS,
            "submission {i}: invocations grew past the bound, got {}",
            store.invocation_count()
        );
        assert!(
            store.client_token_count() <= store.invocation_count(),
            "submission {i}: client_tokens grew past invocations, got {} vs {}",
            store.client_token_count(),
            store.invocation_count()
        );
    }

    assert_eq!(
        store.invocation_count(),
        bda::state::MAX_INVOCATIONS,
        "the store must sit exactly at its bound after {submissions} submissions, got {}",
        store.invocation_count()
    );
    // Every token that is still retained names a record that is still retained, so a
    // replay can never hand back an ARN a reader cannot resolve.
    let mut replayable = 0usize;
    for token in &tokens {
        if let Some(arn) = store.replay_for_client_token(token) {
            replayable += 1;
            assert!(
                store.invocation(&arn).is_some(),
                "token {token:?} replayed {arn:?}, which is no longer stored"
            );
        }
    }
    assert_eq!(
        replayable,
        bda::state::MAX_INVOCATIONS,
        "exactly the retained records' tokens may replay, got {replayable}"
    );
}

#[tokio::test]
async fn the_synchronous_invoke_data_automation_is_still_a_stub() {
    // `InvokeDataAutomation` returns the analysis of the asset itself
    // (`semanticModality` plus `outputSegments`), which no engine in this emulator can
    // produce. It therefore keeps the scaffold's 501 rather than answering with
    // synthesised output; this pins that it is a 501 and not an empty 200.
    let router = service();
    let answer = call(
        &router,
        "InvokeDataAutomation",
        json!({
            "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/doc.pdf"},
            "dataAutomationProfileArn": PROFILE_ARN,
        }),
    )
    .await;
    assert_eq!(
        answer.status, 501,
        "expected 501 NotImplemented from the synchronous invoke, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "NotImplemented",
        "expected the scaffold's NotImplemented, got {}",
        answer.body
    );
}
