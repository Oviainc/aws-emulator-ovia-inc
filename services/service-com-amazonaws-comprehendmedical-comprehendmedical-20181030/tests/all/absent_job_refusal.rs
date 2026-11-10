// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how this service refuses a request that names an asynchronous
//! job it has no record of.
//!
//! The rule under test: a per-id asynchronous-job operation handed a well-formed `JobId`
//! the family it addresses holds no job for is refused with the modelled
//! `ResourceNotFoundException`, answered with the status this service answers a
//! client-input refusal with (HTTP 400), and one fixed sentence for a `Message` — no
//! variable part, so the submitted identifier is never echoed back — while an id that
//! does name a job of that family is answered normally and a value outside the member's
//! modelled constraints is still refused at the boundary with its constraint report.
//!
//! The table spans the class the rule generalises over: every per-id job operation this
//! service implements (describe and stop, across the job families that have them), and
//! the ways an id can name no job of the family it is sent to — one nothing ever minted,
//! one at each end of the member's modelled length, one made of the exotic characters the
//! member's pattern admits, and one another family minted and still holds. A job whose
//! metadata has aged out resolves the same way, through the same family-store lookup, and
//! is covered where the store's retention is exercised rather than here: the modelled
//! `ExpirationTime` is months away, so no test can wait it out over the wire.
//!
//! Every request goes over the wire the service actually speaks (awsJson1_1, `POST /`
//! with `X-Amz-Target: ComprehendMedical_20181030.<Operation>`) through the same
//! `MultiServiceRouter` the emulator's front doors use, so these tests exercise routing,
//! validation, the handler and serialization together.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from the service's wire protocol.
const TARGET_PREFIX: &str = "ComprehendMedical_20181030";

/// What the service reports for a request that names a job it cannot find.
const JOB_NOT_FOUND: &str = "Could not find specified job.";

/// The per-id operations of the job families this service implements, each paired with
/// the start operation that mints an id it resolves
/// (`smithy_query(query="digest")` lists the family's Start/Describe/Stop trio).
const PER_ID_JOB_OPERATIONS: &[(&str, &str)] = &[
    ("DescribePHIDetectionJob", "StartPHIDetectionJob"),
    (
        "DescribeEntitiesDetectionV2Job",
        "StartEntitiesDetectionV2Job",
    ),
    (
        "StopEntitiesDetectionV2Job",
        "StartEntitiesDetectionV2Job",
    ),
];

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

/// A start body with everything the model requires for any asynchronous job family
/// (`InputDataConfig`, `OutputDataConfig`, `DataAccessRoleArn`, `LanguageCode`).
fn start_body() -> Value {
    json!({
        "InputDataConfig": {"S3Bucket": "in-bucket", "S3Key": "docs/"},
        "OutputDataConfig": {"S3Bucket": "out-bucket"},
        "DataAccessRoleArn": "arn:aws:iam::123456789012:role/CMRole",
        "LanguageCode": "en",
    })
}

/// Start a job through `operation` and return the `JobId` it minted.
async fn start_job(router: &MultiServiceRouter, operation: &str) -> String {
    let (status, answer) = call(router, operation, &start_body()).await;
    assert_eq!(
        status, 200,
        "case {operation:?}: starting a job must answer 200, got {status} {answer}"
    );
    answer["JobId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {operation:?}: expected a JobId, got {answer}"))
        .to_string()
}

/// The ids that name no job of the family they are sent to, beside the one a sibling
/// family minted (which each test adds for itself).
///
/// Every value is inside `JobId`'s modelled length 1..32 and pattern
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$`
/// (`smithy_query(query="shape",shape="JobId")`), so the boundary validator hands each
/// request to the operation.
fn absent_ids() -> Vec<(&'static str, String)> {
    vec![
        ("a job id nothing ever minted", "doesnotexist".to_string()),
        ("a job id at the length minimum", "j".to_string()),
        ("a job id at the length maximum", "j".repeat(32)),
        (
            "a job id of the characters the pattern admits",
            "a.b_c:d/e=f+g%h@i 1".to_string(),
        ),
        (
            "a job id of letters outside ASCII",
            "patiëntenzorg-ß".to_string(),
        ),
    ]
}

/// A per-id job operation handed an id its family holds no job for is refused with the
/// modelled not-found error shape, the service's refusal status and one fixed sentence.
#[tokio::test]
async fn an_id_naming_no_job_of_the_family_is_refused() {
    let router = router();

    for (operation, start) in PER_ID_JOB_OPERATIONS {
        // A job of ANOTHER family: its id is well formed and stored, but not where this
        // operation looks for it.
        let other_family = if start.contains("PHIDetection") {
            start_job(&router, "StartEntitiesDetectionV2Job").await
        } else {
            start_job(&router, "StartPHIDetectionJob").await
        };
        let mut cases = absent_ids();
        cases.push(("a job id another family minted", other_family));

        for (case, job_id) in cases {
            let (status, answer) = call(&router, operation, &json!({"JobId": job_id})).await;
            assert_eq!(
                status, 400,
                "case {operation:?} / {case:?}: expected 400, got {status} {answer}"
            );
            assert_eq!(
                answer["__type"].as_str(),
                Some("ResourceNotFoundException"),
                "case {operation:?} / {case:?}: expected ResourceNotFoundException, got {answer}"
            );
            assert_eq!(
                answer["Message"].as_str(),
                Some(JOB_NOT_FOUND),
                "case {operation:?} / {case:?}: expected {JOB_NOT_FOUND:?}, got {answer}"
            );
            assert!(
                answer["ComprehendMedicalAsyncJobProperties"].is_null()
                    && answer["JobId"].is_null(),
                "case {operation:?} / {case:?}: a refusal reports no job, got {answer}"
            );
        }
    }
}

/// The refusal carries no part of the request: two different absent ids, on any per-id
/// job operation, produce the same answer byte for byte.
#[tokio::test]
async fn the_refusal_echoes_nothing_from_the_request() {
    let router = router();

    for (operation, _) in PER_ID_JOB_OPERATIONS {
        let mut answers: Vec<(&str, Value)> = Vec::new();
        for (case, job_id) in absent_ids() {
            let (_, answer) = call(&router, operation, &json!({"JobId": job_id.clone()})).await;
            let message = answer["Message"].as_str().unwrap_or_default();
            // A containment test only means something for an id no single word of the
            // sentence could hold, so it runs on the multi-character ids; the
            // byte-identity comparison below is what covers every row.
            if job_id.chars().count() > 3 {
                assert!(
                    !message.contains(job_id.as_str()),
                    "case {operation:?} / {case:?}: the refusal must not echo the id, got \
                     {message:?}"
                );
            }
            answers.push((case, answer));
        }
        let (first_case, first) = answers
            .first()
            .cloned()
            .expect("the absent-id table is not empty");
        for (case, answer) in &answers {
            assert_eq!(
                answer, &first,
                "case {operation:?} / {case:?}: expected the same answer as {first_case:?} \
                 ({first}), got {answer}"
            );
        }
    }
}

/// The refusal is for an id that names NO job: an id that does name one of the family is
/// answered normally, and the refusals above left the stored jobs alone.
#[tokio::test]
async fn an_id_naming_a_job_of_the_family_is_still_answered() {
    let router = router();

    for (operation, start) in PER_ID_JOB_OPERATIONS {
        let job_id = start_job(&router, start).await;
        // One refusal first, so the success below cannot be explained by an untouched
        // store.
        let (refused, _) = call(&router, operation, &json!({"JobId": "nosuchjob"})).await;
        assert_eq!(
            refused, 400,
            "case {operation:?}: an absent id is refused with 400, got {refused}"
        );

        let (status, answer) = call(&router, operation, &json!({"JobId": job_id.clone()})).await;
        assert_eq!(
            status, 200,
            "case {operation:?} / {job_id:?}: expected 200, got {status} {answer}"
        );
        let reported = answer["ComprehendMedicalAsyncJobProperties"]["JobId"]
            .as_str()
            .or_else(|| answer["JobId"].as_str());
        assert_eq!(
            reported,
            Some(job_id.as_str()),
            "case {operation:?} / {job_id:?}: expected the job it was asked about, got {answer}"
        );
    }
}

/// A `JobId` outside the member's modelled constraints is still refused at the boundary,
/// with the constraint report rather than the absent-job sentence: the two negative
/// classes stay distinguishable by message, as they are on AWS.
#[tokio::test]
async fn a_job_id_outside_its_constraints_keeps_the_constraint_refusal() {
    let router = router();

    // (case, the value sent) — one value per constraint `JobId` declares: the pattern,
    // and the length minimum of 1.
    let malformed: Vec<(&str, &str)> = vec![
        ("a value the pattern rejects", "not#a#valid#job#id!"),
        ("a value under the length minimum", ""),
    ];

    for (operation, _) in PER_ID_JOB_OPERATIONS {
        for (case, job_id) in &malformed {
            let (status, answer) = call(&router, operation, &json!({"JobId": job_id})).await;
            assert_eq!(
                status, 400,
                "case {operation:?} / {case:?}: expected 400, got {status} {answer}"
            );
            let message = answer["message"]
                .as_str()
                .or_else(|| answer["Message"].as_str())
                .unwrap_or_default();
            assert!(
                message.contains("failed to satisfy constraint"),
                "case {operation:?} / {case:?}: expected the constraint report, got {answer}"
            );
            assert_ne!(
                message, JOB_NOT_FOUND,
                "case {operation:?} / {case:?}: a bad value is not an absent job, got {answer}"
            );
        }
    }
}
