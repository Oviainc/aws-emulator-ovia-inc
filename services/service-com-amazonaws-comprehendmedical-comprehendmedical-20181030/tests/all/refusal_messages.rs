// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two refusals this service words itself: how it states a
//! value outside a modelled constraint, and how it refuses a stop that names a job which
//! has already finished.
//!
//! The rules under test:
//!
//! * a value outside a modelled constraint is refused in AWS's validation envelope —
//!   `N validation error(s) detected: ` over one clause per violated constraint — and the
//!   clause names the offending MEMBER PATH and the CONSTRAINT it broke (the pattern, the
//!   length bound, the numeric bound, the enum's value set), never the submitted value;
//! * the error code is chosen per violation class: text past its modelled length maximum
//!   is `TextSizeLimitExceededException`, every other bad value
//!   `InvalidRequestException`;
//! * a data-access role ARN that was provided and does not fit the modelled IAM role ARN
//!   shape is refused with the service's own reason tag, which is what tells a caller to
//!   correct the string rather than the role;
//! * a stop naming a job in a terminal status is refused with `InvalidRequestException`
//!   and leaves the stored job exactly as it was.
//!
//! Each table spans the class the rule generalises over — the constraint kind, the member
//! and its nesting, the operation and job family, both ends of each published bound, each
//! terminal status a job can be driven to — rather than one recorded request.
//!
//! Every request goes over the wire the service actually speaks (awsJson1_1, `POST /`
//! with `X-Amz-Target: ComprehendMedical_20181030.<Operation>`) through the same
//! `MultiServiceRouter` the emulator's front doors use, so these tests exercise routing,
//! validation, the handler and serialization together.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from the service's wire protocol.
const TARGET_PREFIX: &str = "ComprehendMedical_20181030";

/// The expression the model declares for `JobId` and for `ComprehendMedicalAsyncJobFilter$JobName`
/// (`smithy_query(query="shape",shape="JobId,ComprehendMedicalAsyncJobFilter")`).
const NAME_PATTERN: &str = r"^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$";

/// What the service reports for a data-access role ARN outside that shape.
const ROLE_ARN_INVALID: &str =
    "DATA_ACCESS_ROLE_ARN_INVALID: The provided dataAccessRoleArn is invalid.";

/// What the service reports for a stop that names a job which has already finished.
const ALREADY_TERMINAL: &str = "Job is already in terminal state.";

/// A well-formed data-access role ARN, inside `IamRoleArn`'s length and pattern.
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/CMRole";

/// Wait out a transition the clock owes a job.
///
/// `std::thread::sleep` rather than `tokio::time::sleep`: the workspace pins tokio
/// without its `time` feature (see the root manifest), and each test awaits nothing else
/// while it waits.
fn wait(millis: u64) {
    std::thread::sleep(Duration::from_millis(millis));
}

/// A router carrying only this service, over its own engine, so each test observes
/// exactly the jobs it created.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_comprehendmedical_comprehendmedical_20181030::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Call `operation` with `body` and return the HTTP status and the parsed JSON answer.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header("content-type", "application/x-amz-json-1.1")
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("serialize the request body"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// The message an answer carries, whichever casing its envelope spells the member with:
/// the boundary refusal renders the protocol's own `message`, while a modelled exception
/// renders the `Message` member the model declares.
fn error_message(answer: &Value) -> Option<&str> {
    answer["message"]
        .as_str()
        .or_else(|| answer["Message"].as_str())
}

/// One clause in AWS's envelope, for a member whose value is withheld.
fn one_error(path: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value at '{path}' failed to satisfy constraint: \
         {requirement}"
    )
}

/// A start call for the entities-detection family with everything the model requires,
/// plus whatever `extra` overrides.
fn start_body(extra: Vec<(&str, Value)>) -> Value {
    let mut body = json!({
        "InputDataConfig": {"S3Bucket": "in-bucket", "S3Key": "docs/"},
        "OutputDataConfig": {"S3Bucket": "out-bucket"},
        "DataAccessRoleArn": ROLE_ARN,
        "LanguageCode": "en",
    });
    for (member, value) in extra {
        body[member] = value;
    }
    body
}

/// Start an entities-detection job and return its `JobId`.
async fn start_job(router: &MultiServiceRouter) -> String {
    let (status, answer) = call(router, "StartEntitiesDetectionV2Job", &start_body(vec![])).await;
    assert_eq!(status, 200, "start a job: expected 200, got {status} {answer}");
    answer["JobId"]
        .as_str()
        .unwrap_or_else(|| panic!("start a job: expected a JobId, got {answer}"))
        .to_string()
}

/// Describe an entities-detection job and return its properties.
async fn describe_job(router: &MultiServiceRouter, job_id: &str) -> Value {
    let (status, answer) = call(
        router,
        "DescribeEntitiesDetectionV2Job",
        &json!({"JobId": job_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "describe {job_id}: expected 200, got {status} {answer}"
    );
    answer["ComprehendMedicalAsyncJobProperties"].clone()
}

/// RULE: a refused value is stated in AWS's envelope, naming the member path and the
/// constraint it broke — whichever constraint class it was, wherever the member is nested
/// and whichever operation carried it — and the code follows the violation class.
#[tokio::test]
async fn a_constraint_refusal_names_the_member_path_and_the_constraint() {
    // (case, operation, body, the code expected, the whole message expected)
    let cases: Vec<(&str, &str, Value, &str, String)> = vec![
        (
            "a job id outside its pattern",
            "DescribeEntitiesDetectionV2Job",
            json!({"JobId": "not#a#valid#job#id!"}),
            "InvalidRequestException",
            one_error(
                "jobId",
                &format!("Member must satisfy regular expression pattern: {NAME_PATTERN}"),
            ),
        ),
        (
            "the same member of another job family",
            "DescribePHIDetectionJob",
            json!({"JobId": "not#a#valid#job#id!"}),
            "InvalidRequestException",
            one_error(
                "jobId",
                &format!("Member must satisfy regular expression pattern: {NAME_PATTERN}"),
            ),
        ),
        (
            "a job id past its length maximum",
            "DescribeEntitiesDetectionV2Job",
            json!({"JobId": "j".repeat(33)}),
            "InvalidRequestException",
            one_error("jobId", "Member must have length less than or equal to 32"),
        ),
        (
            "a required member left out",
            "DescribeEntitiesDetectionV2Job",
            json!({}),
            "InvalidRequestException",
            "1 validation error detected: Value null at 'jobId' failed to satisfy constraint: \
             Member must not be null"
                .to_string(),
        ),
        (
            "a filter member outside its pattern, nested one level down",
            "ListEntitiesDetectionV2Jobs",
            json!({"Filter": {"JobName": "bad#name#!"}}),
            "InvalidRequestException",
            one_error(
                "filter.jobName",
                &format!("Member must satisfy regular expression pattern: {NAME_PATTERN}"),
            ),
        ),
        (
            "the same filter member of another job family",
            "ListPHIDetectionJobs",
            json!({"Filter": {"JobName": "bad#name#!"}}),
            "InvalidRequestException",
            one_error(
                "filter.jobName",
                &format!("Member must satisfy regular expression pattern: {NAME_PATTERN}"),
            ),
        ),
        (
            "a filter member past its length maximum",
            "ListEntitiesDetectionV2Jobs",
            json!({"Filter": {"JobName": "n".repeat(257)}}),
            "InvalidRequestException",
            one_error(
                "filter.jobName",
                "Member must have length less than or equal to 256",
            ),
        ),
        (
            "a page size above its range maximum",
            "ListEntitiesDetectionV2Jobs",
            json!({"MaxResults": 501}),
            "InvalidRequestException",
            one_error(
                "maxResults",
                "Member must have value less than or equal to 500",
            ),
        ),
        (
            "a page size below its range minimum",
            "ListEntitiesDetectionV2Jobs",
            json!({"MaxResults": 0}),
            "InvalidRequestException",
            one_error(
                "maxResults",
                "Member must have value greater than or equal to 1",
            ),
        ),
        (
            "a page size far below its range minimum",
            "ListPHIDetectionJobs",
            json!({"MaxResults": -7}),
            "InvalidRequestException",
            one_error(
                "maxResults",
                "Member must have value greater than or equal to 1",
            ),
        ),
        (
            "text past the entity-detection length maximum",
            "DetectEntitiesV2",
            json!({"Text": "a".repeat(20_001)}),
            "TextSizeLimitExceededException",
            one_error("text", "Member must have length less than or equal to 20000"),
        ),
        (
            "text past the ontology-linking length maximum",
            "InferRxNorm",
            json!({"Text": "a".repeat(10_001)}),
            "TextSizeLimitExceededException",
            one_error("text", "Member must have length less than or equal to 10000"),
        ),
        (
            "text below its length minimum",
            "DetectEntitiesV2",
            json!({"Text": ""}),
            "InvalidRequestException",
            one_error("text", "Member must have length greater than or equal to 1"),
        ),
        (
            "a language code the enum does not carry",
            "StartEntitiesDetectionV2Job",
            start_body(vec![("LanguageCode", json!("fr"))]),
            "InvalidRequestException",
            one_error(
                "languageCode",
                "Member must satisfy enum value set: [en]",
            ),
        ),
        (
            "a request token outside its pattern",
            "StartEntitiesDetectionV2Job",
            start_body(vec![("ClientRequestToken", json!("bad token"))]),
            "InvalidRequestException",
            one_error(
                "clientRequestToken",
                r"Member must satisfy regular expression pattern: ^[a-zA-Z0-9-]+$",
            ),
        ),
    ];

    let router = router();
    for (case, operation, body, code, message) in cases {
        let (status, answer) = call(&router, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {answer}"
        );
        assert_eq!(
            answer["__type"].as_str(),
            Some(code),
            "case {case:?}: expected {code}, got {answer}"
        );
        assert_eq!(
            error_message(&answer),
            Some(message.as_str()),
            "case {case:?}: expected {message:?}, got {answer}"
        );
    }
}

/// RULE: the clause states the member and the constraint, and never the value the caller
/// submitted — whatever member it was and however distinctive the value.
#[tokio::test]
async fn a_refusal_never_echoes_the_submitted_value() {
    // (case, operation, body, the submitted value no message may carry)
    let cases: Vec<(&str, &str, Value, String)> = vec![
        (
            "a job id",
            "DescribeEntitiesDetectionV2Job",
            json!({"JobId": "patient#4711#mrn!"}),
            "patient#4711#mrn!".to_string(),
        ),
        (
            "a filter job name",
            "ListEntitiesDetectionV2Jobs",
            json!({"Filter": {"JobName": "ward#3#oncology!"}}),
            "ward#3#oncology!".to_string(),
        ),
        (
            "clinical text past its limit",
            "DetectEntitiesV2",
            json!({"Text": format!("{}{}", "Mr Nemo takes 50 mg Ibuprofen. ", "a".repeat(20_001))}),
            "Mr Nemo takes 50 mg Ibuprofen.".to_string(),
        ),
        (
            "a page size",
            "ListEntitiesDetectionV2Jobs",
            json!({"MaxResults": 99_999}),
            "99999".to_string(),
        ),
    ];

    let router = router();
    for (case, operation, body, submitted) in cases {
        let (status, answer) = call(&router, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {answer}"
        );
        let message = error_message(&answer).unwrap_or_default();
        assert!(
            !message.contains(&submitted),
            "case {case:?}: the refusal must not echo the submitted value, got {message:?}"
        );
        assert!(
            message.contains("failed to satisfy constraint: Member must"),
            "case {case:?}: the refusal must state the constraint, got {message:?}"
        );
    }
}

/// RULE: only a value OUTSIDE a published bound is refused — each bound's own extremes
/// are accepted and reach the operation, which answers whatever it answers for a valid
/// request.
#[tokio::test]
async fn a_value_at_a_published_bound_is_accepted() {
    // (case, operation, body, the status expected). 501 is the scaffold's own answer for
    // an operation nobody has implemented yet, and 400 the operation's OWN refusal of a
    // well-formed job id that names no job: the point of each row is that the request was
    // NOT refused at the boundary, which the message check below is what pins.
    let cases: Vec<(&str, &str, Value, u16)> = vec![
        (
            "the page-size minimum",
            "ListEntitiesDetectionV2Jobs",
            json!({"MaxResults": 1}),
            200,
        ),
        (
            "the page-size maximum",
            "ListEntitiesDetectionV2Jobs",
            json!({"MaxResults": 500}),
            200,
        ),
        (
            "a job id at its length maximum",
            "DescribeEntitiesDetectionV2Job",
            json!({"JobId": "j".repeat(32)}),
            400,
        ),
        (
            "a filter job name at its length maximum",
            "ListEntitiesDetectionV2Jobs",
            json!({"Filter": {"JobName": "n".repeat(256)}}),
            200,
        ),
        (
            "text at the entity-detection length maximum",
            "DetectEntitiesV2",
            json!({"Text": "a".repeat(20_000)}),
            501,
        ),
        (
            "text at the ontology-linking length maximum",
            "InferRxNorm",
            json!({"Text": "a".repeat(10_000)}),
            200,
        ),
        (
            "text at its length minimum",
            "InferRxNorm",
            json!({"Text": "a"}),
            200,
        ),
    ];

    let router = router();
    for (case, operation, body, expected) in cases {
        let (status, answer) = call(&router, operation, &body).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {answer}"
        );
        let message = error_message(&answer).unwrap_or_default();
        assert!(
            !message.contains("failed to satisfy constraint"),
            "case {case:?}: a value at a published bound must reach the operation rather \
             than be refused at the boundary, got {message:?}"
        );
    }
}

/// RULE: a data-access role ARN the caller provided outside the modelled IAM role ARN
/// shape is refused with the service's own reason tag — whichever of that member's
/// constraints it broke and whichever job family's start call carried it — while a
/// well-formed one is not refused and an absent one is stated as the absent member it is.
#[tokio::test]
async fn a_provided_data_access_role_arn_outside_its_shape_is_tagged() {
    // (case, operation, the ARN the request carried, the whole message expected)
    let malformed: Vec<(&str, &str, Value)> = vec![
        (
            "an ARN that is not an ARN at all",
            "StartEntitiesDetectionV2Job",
            json!("not-a-valid-arn"),
        ),
        (
            "an ARN of the right length outside the pattern",
            "StartEntitiesDetectionV2Job",
            json!("arn:aws:iam::123456789012:user/NotARole"),
        ),
        (
            "an ARN whose account id is not twelve digits",
            "StartEntitiesDetectionV2Job",
            json!("arn:aws:iam::12345:role/CMRole"),
        ),
        (
            "an ARN naming no role path",
            "StartEntitiesDetectionV2Job",
            json!("arn:aws:iam::123456789012:role/"),
        ),
        (
            "an ARN past the length maximum",
            "StartEntitiesDetectionV2Job",
            json!(format!("arn:aws:iam::123456789012:role/{}", "r".repeat(2048))),
        ),
        (
            "the same member of another job family",
            "StartPHIDetectionJob",
            json!("not-a-valid-arn"),
        ),
        (
            "the same member of the RxNorm job family",
            "StartRxNormInferenceJob",
            json!("short"),
        ),
    ];

    let router = router();
    for (case, operation, arn) in malformed {
        let body = start_body(vec![("DataAccessRoleArn", arn)]);
        let (status, answer) = call(&router, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {answer}"
        );
        assert_eq!(
            answer["__type"].as_str(),
            Some("InvalidRequestException"),
            "case {case:?}: expected InvalidRequestException, got {answer}"
        );
        assert_eq!(
            error_message(&answer),
            Some(ROLE_ARN_INVALID),
            "case {case:?}: expected {ROLE_ARN_INVALID:?}, got {answer}"
        );
        assert_eq!(
            error_message(&answer)
                .unwrap_or_default()
                .matches("dataAccessRoleArn")
                .count(),
            1,
            "case {case:?}: the member must be named once, got {answer}"
        );
    }

    // An absent ARN is a fact of the request's shape, not an ARN that was provided, so it
    // is stated as the absent member it is.
    let mut absent = start_body(vec![]);
    absent
        .as_object_mut()
        .expect("the start body is an object")
        .remove("DataAccessRoleArn");
    let (status, answer) = call(&router, "StartEntitiesDetectionV2Job", &absent).await;
    assert_eq!(
        status, 400,
        "case an absent role ARN: expected 400, got {status} {answer}"
    );
    assert_eq!(
        error_message(&answer),
        Some(
            "1 validation error detected: Value null at 'dataAccessRoleArn' failed to satisfy \
             constraint: Member must not be null"
        ),
        "case an absent role ARN: expected the absent-member clause, got {answer}"
    );

    // A well-formed ARN is not refused: an implemented job family starts the job, and a
    // family that is still a scaffold answers its own 501.
    for (operation, expected) in [
        ("StartEntitiesDetectionV2Job", 200_u16),
        ("StartPHIDetectionJob", 200),
        ("StartICD10CMInferenceJob", 501),
        ("StartRxNormInferenceJob", 501),
        ("StartSNOMEDCTInferenceJob", 501),
    ] {
        let (status, answer) = call(&router, operation, &start_body(vec![])).await;
        assert_eq!(
            status, expected,
            "case a well-formed role ARN on {operation:?}: expected {expected}, got {status} \
             {answer}"
        );
    }
}

/// RULE: a stop naming a job that has already reached a terminal status is refused with
/// the modelled client error, and the stored job is unchanged by the refused call — while
/// a job still running is stopped, and repeating the stop before it takes effect is the
/// same request again.
#[tokio::test]
async fn stopping_a_job_in_a_terminal_status_is_refused() {
    let router = router();

    // The two terminal statuses a job of this emulator can be driven to: the one the
    // clock owes a started job, and the one a stop that took effect leaves behind.
    let failed = start_job(&router).await;
    let stopped = start_job(&router).await;

    // A job still running is stopped, and the repeated stop before it takes effect is
    // accepted: `STOP_REQUESTED` is not terminal.
    for attempt in ["the first stop", "the stop repeated while it is pending"] {
        let (status, answer) = call(
            &router,
            "StopEntitiesDetectionV2Job",
            &json!({"JobId": stopped.clone()}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {attempt:?}: a job still running must be stoppable, got {status} {answer}"
        );
        assert_eq!(
            answer["JobId"].as_str(),
            Some(stopped.as_str()),
            "case {attempt:?}: expected the JobId back, got {answer}"
        );
    }

    // Both transitions are due on the clock: the stop's move to STOPPED, and the started
    // job's own move to its terminal status.
    wait(900);

    for (case, job_id, expected_status) in [
        ("a job the clock finished", &failed, "FAILED"),
        ("a job a stop finished", &stopped, "STOPPED"),
    ] {
        let before = describe_job(&router, job_id).await;
        assert_eq!(
            before["JobStatus"].as_str(),
            Some(expected_status),
            "case {case:?}: expected {expected_status}, got {before}"
        );

        let (status, answer) = call(
            &router,
            "StopEntitiesDetectionV2Job",
            &json!({"JobId": job_id.clone()}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {answer}"
        );
        assert_eq!(
            answer["__type"].as_str(),
            Some("InvalidRequestException"),
            "case {case:?}: expected InvalidRequestException, got {answer}"
        );
        assert_eq!(
            error_message(&answer),
            Some(ALREADY_TERMINAL),
            "case {case:?}: expected {ALREADY_TERMINAL:?}, got {answer}"
        );
        assert!(
            answer["JobId"].is_null(),
            "case {case:?}: a refused stop answers no JobId, got {answer}"
        );

        let after = describe_job(&router, job_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused stop must change nothing, got {after}"
        );
    }

    // A stop that names no stored job is the not-found refusal, not the terminal-state
    // one: both are answered with the same status, so the two failures are told apart by
    // their error code and their message.
    let (status, answer) = call(
        &router,
        "StopEntitiesDetectionV2Job",
        &json!({"JobId": "nosuchjob"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case an unknown job: expected 400, got {status} {answer}"
    );
    assert_eq!(
        answer["__type"].as_str(),
        Some("ResourceNotFoundException"),
        "case an unknown job: expected ResourceNotFoundException, got {answer}"
    );
    assert_ne!(
        error_message(&answer),
        Some(ALREADY_TERMINAL),
        "case an unknown job: expected its own message, got {answer}"
    );
}
