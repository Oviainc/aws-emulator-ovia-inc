// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribePHIDetectionJob`.
//!
//! The rule under test: a job-family describe answers with the stored job the `JobId`
//! names — every member its start call was given, plus the status the clock has moved it
//! to — and refuses with `ResourceNotFoundException` any `JobId` that names no job of
//! THAT family. The tables therefore span a class of starts (each optional member of the
//! start call present and absent, several at once), both ends of the status progression,
//! and the ids that name no PHI job: never created, created by another family, and one
//! past a job's `ExpirationTime` is covered by the family store's own expiry.
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

/// Wait out a transition the clock owes a job.
///
/// `std::thread::sleep` rather than `tokio::time::sleep`: the workspace pins tokio
/// without its `time` feature, and each test awaits nothing else while it waits.
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

/// Start a job of `operation`'s family with `body` and return its `JobId`.
async fn start_job(router: &MultiServiceRouter, operation: &str, body: &Value) -> String {
    let (status, answer) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation:?} {body}: starting a job must answer 200, got {status} {answer}"
    );
    answer["JobId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {operation:?} {body}: expected a JobId, got {answer}"))
        .to_string()
}

/// Describe a PHI job and return its `ComprehendMedicalAsyncJobProperties`.
async fn describe_phi_job(router: &MultiServiceRouter, job_id: &str) -> Value {
    let (status, answer) = call(
        router,
        "DescribePHIDetectionJob",
        &json!({"JobId": job_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {job_id:?}: describing a started PHI job must answer 200, got {status} {answer}"
    );
    answer["ComprehendMedicalAsyncJobProperties"].clone()
}

/// A start body with everything the model requires, plus whatever `extra` adds.
///
/// The required members are the model's own: `InputDataConfig`, `OutputDataConfig`,
/// `DataAccessRoleArn` and `LanguageCode`
/// (`smithy_query(query="operation",operation="StartPHIDetectionJob")`).
fn start_body(extra: Vec<(&str, Value)>) -> Value {
    let mut body = json!({
        "InputDataConfig": {"S3Bucket": "in-bucket", "S3Key": "docs/"},
        "OutputDataConfig": {"S3Bucket": "out-bucket"},
        "DataAccessRoleArn": "arn:aws:iam::123456789012:role/CMRole",
        "LanguageCode": "en",
    });
    for (member, value) in extra {
        body[member] = value;
    }
    body
}

/// One row of the describe table: the case it names, the members it adds to the start
/// call's required ones, and the members the describe must then report.
type DescribeCase = (
    &'static str,
    Vec<(&'static str, Value)>,
    Vec<(&'static str, Value)>,
);

/// `DescribePHIDetectionJob` reports the stored job, member for member, whichever of the
/// start call's optional members it was given.
#[tokio::test]
async fn describing_a_phi_job_reports_what_start_was_given() {
    let router = router();

    // (case, the members added to the required ones, the members describe must report)
    let cases: Vec<DescribeCase> = vec![
        (
            "only the required members",
            vec![],
            vec![
                ("JobName", Value::Null),
                ("KMSKey", Value::Null),
                ("LanguageCode", json!("en")),
                (
                    "InputDataConfig",
                    json!({"S3Bucket": "in-bucket", "S3Key": "docs/"}),
                ),
                ("OutputDataConfig", json!({"S3Bucket": "out-bucket"})),
            ],
        ),
        (
            "a job name as well",
            vec![("JobName", json!("phi-named"))],
            vec![("JobName", json!("phi-named")), ("KMSKey", Value::Null)],
        ),
        (
            "an output encryption key as well",
            vec![("KMSKey", json!("alias/cm"))],
            vec![("KMSKey", json!("alias/cm")), ("JobName", Value::Null)],
        ),
        (
            "every optional member at once",
            vec![
                ("JobName", json!("phi.full-1_2:3/4=5+6%7@8")),
                ("ClientRequestToken", json!("token-full")),
                ("KMSKey", json!("arn:aws:kms:us-east-1:123456789012:key/k")),
                (
                    "OutputDataConfig",
                    json!({"S3Bucket": "other-out", "S3Key": "results/phi/"}),
                ),
                ("InputDataConfig", json!({"S3Bucket": "other-in"})),
            ],
            vec![
                ("JobName", json!("phi.full-1_2:3/4=5+6%7@8")),
                ("KMSKey", json!("arn:aws:kms:us-east-1:123456789012:key/k")),
                (
                    "OutputDataConfig",
                    json!({"S3Bucket": "other-out", "S3Key": "results/phi/"}),
                ),
                ("InputDataConfig", json!({"S3Bucket": "other-in"})),
            ],
        ),
        (
            "a different data access role",
            vec![(
                "DataAccessRoleArn",
                json!("arn:aws:iam::210987654321:role/service-role/Other"),
            )],
            vec![(
                "DataAccessRoleArn",
                json!("arn:aws:iam::210987654321:role/service-role/Other"),
            )],
        ),
    ];

    for (case, extra, expected) in cases {
        let job_id = start_job(&router, "StartPHIDetectionJob", &start_body(extra)).await;
        let properties = describe_phi_job(&router, &job_id).await;
        assert_eq!(
            properties["JobId"].as_str(),
            Some(job_id.as_str()),
            "case {case:?}: describe must answer about the job it was asked for, got {properties}"
        );
        for (member, want) in expected {
            assert_eq!(
                properties[member], want,
                "case {case:?}: expected {member} {want}, got {properties}"
            );
        }
        assert!(
            properties["SubmitTime"].as_f64().is_some_and(|at| at > 0.0),
            "case {case:?}: expected a SubmitTime after the epoch, got {properties}"
        );
        assert!(
            properties["ExpirationTime"].as_f64() > properties["SubmitTime"].as_f64(),
            "case {case:?}: expected job metadata to expire after it was submitted, got \
             {properties}"
        );
        assert_eq!(
            properties["ModelVersion"].as_str().map(str::is_empty),
            Some(false),
            "case {case:?}: expected the model version behind the job, got {properties}"
        );
        // The list of the same family reports the same record, so the describe reads the
        // store rather than answering from the request.
        let (status, listed) = call(&router, "ListPHIDetectionJobs", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: listing the family must answer 200, got {status} {listed}"
        );
        let from_list = listed["ComprehendMedicalAsyncJobPropertiesList"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|job| job["JobId"].as_str() == Some(job_id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: the started job must be listed, got {listed}"));
        assert_eq!(
            from_list, properties,
            "case {case:?}: describe and list must report one record, got {properties} and \
             {from_list}"
        );
    }
}

/// A `JobId` that names no job of the PHI family is a `ResourceNotFoundException`, and a
/// describe never creates the job it was asked about.
#[tokio::test]
async fn describing_a_job_the_phi_family_does_not_hold_is_not_found() {
    let router = router();
    // A job of ANOTHER family: its id is well formed and stored, but not here.
    let other_family = start_job(
        &router,
        "StartEntitiesDetectionV2Job",
        &start_body(vec![("JobName", json!("entities"))]),
    )
    .await;

    // (case, the JobId asked about) — every value inside the modelled 1..32 length and
    // pattern, so the boundary validator passes the request to the handler.
    let cases: Vec<(&str, String)> = vec![
        ("a job id nothing ever minted", "doesnotexist".to_string()),
        ("the shortest possible job id", "j".to_string()),
        ("a job id at the length maximum", "j".repeat(32)),
        ("a job id of another family", other_family.clone()),
    ];

    for (case, job_id) in cases {
        let (status, answer) = call(
            &router,
            "DescribePHIDetectionJob",
            &json!({"JobId": job_id}),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {answer}"
        );
        assert_eq!(
            answer["__type"].as_str(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {answer}"
        );
        assert!(
            answer["ComprehendMedicalAsyncJobProperties"].is_null(),
            "case {case:?}: a refusal reports no job properties, got {answer}"
        );
    }

    // The describes above neither created a PHI job nor disturbed the other family's.
    let (status, listed) = call(&router, "ListPHIDetectionJobs", &json!({})).await;
    assert_eq!(
        status, 200,
        "listing the PHI family must answer 200, got {status} {listed}"
    );
    assert_eq!(
        listed["ComprehendMedicalAsyncJobPropertiesList"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "case a describe of an unknown job: the family must still hold no job, got {listed}"
    );
    let (status, answer) = call(
        &router,
        "DescribeEntitiesDetectionV2Job",
        &json!({"JobId": other_family.clone()}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {other_family:?}: the other family still holds its job, got {status} {answer}"
    );
}

/// A described PHI job carries the status the clock has moved it to: non-terminal
/// straight after the start, then the terminal status with an `EndTime` and a `Message`
/// saying why — and describing it repeatedly shows a later caller nothing different.
#[tokio::test]
async fn describing_a_phi_job_reports_the_status_the_clock_owes_it() {
    let router = router();
    let job_id = start_job(
        &router,
        "StartPHIDetectionJob",
        &start_body(vec![("JobName", json!("phi-progress"))]),
    )
    .await;

    let fresh = describe_phi_job(&router, &job_id).await;
    let fresh_status = fresh["JobStatus"].as_str().unwrap_or_default().to_string();
    assert!(
        ["SUBMITTED", "IN_PROGRESS"].contains(&fresh_status.as_str()),
        "case a job just started: expected a non-terminal status, got {fresh_status:?}"
    );
    assert!(
        fresh["EndTime"].is_null(),
        "case a job just started: a running job has no EndTime, got {fresh}"
    );
    // Two describes in a row are reads: the second shows what the first did, bar the
    // transitions the clock itself owed in between.
    let again = describe_phi_job(&router, &job_id).await;
    assert_eq!(
        again["SubmitTime"], fresh["SubmitTime"],
        "case a repeated describe: the submit time never changes, got {again} after {fresh}"
    );
    assert_eq!(
        again["ExpirationTime"], fresh["ExpirationTime"],
        "case a repeated describe: the expiration time never changes, got {again} after {fresh}"
    );

    wait(900);
    let settled = describe_phi_job(&router, &job_id).await;
    assert_eq!(
        settled["JobStatus"].as_str(),
        Some("FAILED"),
        "case a job past its running span: expected its terminal status, got {settled}"
    );
    assert!(
        settled["EndTime"].as_f64() > settled["SubmitTime"].as_f64(),
        "case a finished job: expected an EndTime after the SubmitTime, got {settled}"
    );
    assert!(
        settled["Message"]
            .as_str()
            .is_some_and(|message| message.contains("in-bucket")),
        "case a FAILED job: expected a Message naming the input it could not read, got {settled}"
    );
    assert!(
        settled["ManifestFilePath"].is_null(),
        "case a job no run analysed: expected no manifest, got {settled}"
    );

    // A terminal status is terminal: a later describe reports exactly the same record.
    wait(100);
    let still = describe_phi_job(&router, &job_id).await;
    assert_eq!(
        still, settled,
        "case a describe after the terminal status: expected the same record, got {still}"
    );
}
