// SPDX-License-Identifier: CC0-1.0

//! How EMR Serverless words the requests it refuses, end to end over HTTP.
//!
//! Every case goes through the real router (restJson1, SigV4 signing name
//! `emr-serverless`), so each one exercises routing, deserialization, the boundary
//! validator, this service's refusal wording and serialization — the same path an SDK
//! client takes.
//!
//! Three rules are under test, each asserted over a SET of inputs rather than one
//! recorded request:
//!
//! 1. A refusal states the condition it refuses on; it never echoes the value the
//!    request carried. A not-found refusal names the RESOURCE CLASS that does not
//!    exist, the same sentence whichever operation refused and whatever id was asked
//!    for.
//! 2. A `@range` violation names the member and the bound it broke, and does not quote
//!    the submitted value.
//! 3. A resource ARN is checked for its VENDOR: an ARN of another service is refused by
//!    that check, naming the vendor that was expected rather than the caller's ARN or
//!    the member's regex, and carrying the front door's generic bad-request identity
//!    rather than one of this service's modelled error shapes.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_emrserverless_awstoledowebservice::state::LIFECYCLE_TRANSITION;
use service_com_amazonaws_emrserverless_awstoledowebservice::EmulatedService;

/// A SigV4 credential scope naming this service's signing name (`emr-serverless`,
/// from `smithy_query(query="service_info")`). It is what routes a request here;
/// request authentication is off, so the signature itself is never checked.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/emr-serverless/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The fixed phrase a `ResourceNotFoundException` carries for an application that does
/// not exist, whichever operation refused.
const APPLICATION_NOT_FOUND: &str = "Application does not exist";

/// The same, for a job run.
const JOB_RUN_NOT_FOUND: &str = "Job Run with given ID doesn't exist";

/// The same again, for an attempt a job run that DOES exist does not have: a not-found
/// refusal states which part of the request was not found, so this is its own phrase.
const JOB_RUN_ATTEMPT_NOT_FOUND: &str = "Job Run with given attempt doesn't exist";

/// The message the resource-ARN vendor check states. The vendor is this service's ARN
/// namespace (`smithy_query(query="service_info")` → `arnNamespace` /
/// `sigv4Name` `emr-serverless`).
const WRONG_VENDOR: &str = "Invalid input resource arn: expected vendor emr-serverless";

/// The error code that refusal carries: the front door's generic bad-request identity,
/// which no operation of this service declares — the ARN names a resource of another
/// vendor, so the request is refused ahead of this service's own input validation.
const WRONG_VENDOR_CODE: &str = "BadRequestException";

/// An execution role ARN satisfying `IAMRoleArn`'s pattern.
const EXECUTION_ROLE: &str = "arn:aws:iam::000000000000:role/EMRServerlessRole";

/// Long enough for a clock-owed transition to have fallen due.
fn wait_for_transition() {
    std::thread::sleep(LIFECYCLE_TRANSITION + Duration::from_millis(250));
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        let raw = self.body["__type"].as_str().unwrap_or_default();
        raw.rsplit('#').next().unwrap_or_default()
    }

    /// The `message` member of an error body, or the empty string when there is none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    /// The value at a JSON pointer, or `Value::Null`.
    fn at(&self, pointer: &str) -> Value {
        self.body.pointer(pointer).cloned().unwrap_or(Value::Null)
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "emr-serverless.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// Percent-encode a path segment, so an ARN can travel in one.
fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// A `StartJobRun` body carrying only the required members, plus whatever `extra`
/// members a case adds.
fn job_run_body(token: &str, extra: Value) -> Value {
    let mut body = json!({
        "clientToken": token,
        "executionRoleArn": EXECUTION_ROLE,
        "jobDriver": { "sparkSubmit": { "entryPoint": "s3://bucket/app.py" } },
    });
    if let (Some(target), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
        for (name, value) in extra {
            target.insert(name.clone(), value.clone());
        }
    }
    body
}

/// Create an application, bring it to `STARTED`, and return its id.
async fn started_application(router: &MultiServiceRouter, token: &str) -> String {
    let created = call(
        router,
        "POST",
        "/applications",
        Some(json!({ "clientToken": token, "releaseLabel": "emr-7.1.0", "type": "SPARK" })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {token:?}: CreateApplication expected 200, got {} with {:?}",
        created.status, created.body
    );
    let id = created
        .at("/applicationId")
        .as_str()
        .unwrap_or_default()
        .to_string();
    wait_for_transition();
    let started = call(router, "POST", &format!("/applications/{id}/start"), None).await;
    assert_eq!(
        started.status, 200,
        "case {token:?}: StartApplication expected 200, got {} with {:?}",
        started.status, started.body
    );
    wait_for_transition();
    id
}

/// Rule 1: a resource that does not exist is refused with the fixed phrase for its
/// RESOURCE CLASS, on every operation that reports one, and the id the request asked
/// for appears nowhere in the body.
///
/// The rows differ along the identifier — its length (1, 12 and the modelled maximum of
/// 64: `ApplicationId` / `JobRunId` are `length 1..64`, `pattern ^[0-9a-z]+$`), its
/// characters (letters, digits, both) — because the rule generalises over exactly that:
/// the message does not depend on the value.
#[tokio::test]
async fn a_not_found_refusal_states_the_resource_class_and_never_the_requested_id() {
    let router = emulator();
    let application_id = started_application(&router, "tok-notfound").await;
    let submitted = call(
        &router,
        "POST",
        &format!("/applications/{application_id}/jobruns"),
        Some(job_run_body("job-notfound", json!({}))),
    )
    .await;
    assert_eq!(
        submitted.status, 200,
        "StartJobRun expected 200, got {} with {:?}",
        submitted.status, submitted.body
    );
    let job_run_id = submitted
        .at("/jobRunId")
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Four ids that name nothing, differing in length (1, 12, 32 and the modelled
    // maximum of 64) and in characters. None of them occurs inside either fixed phrase,
    // so a row can also assert the id is nowhere in the body.
    let unknown_ids = [
        "9",
        "doesnotexist",
        "00fakeidnonexistent0000000000000",
        "9".repeat(64).leak(),
    ];

    for unknown in unknown_ids {
        // One row per operation whose model declares `ResourceNotFoundException`, with
        // the resource class each one fails to find.
        let cases: Vec<(String, &str, String, Option<Value>, &str)> = vec![
            (
                format!("GetApplication({unknown})"),
                "GET",
                format!("/applications/{unknown}"),
                None,
                APPLICATION_NOT_FOUND,
            ),
            (
                format!("StartApplication({unknown})"),
                "POST",
                format!("/applications/{unknown}/start"),
                None,
                APPLICATION_NOT_FOUND,
            ),
            (
                format!("StopApplication({unknown})"),
                "POST",
                format!("/applications/{unknown}/stop"),
                None,
                APPLICATION_NOT_FOUND,
            ),
            (
                format!("DeleteApplication({unknown})"),
                "DELETE",
                format!("/applications/{unknown}"),
                None,
                APPLICATION_NOT_FOUND,
            ),
            (
                format!("StartJobRun({unknown})"),
                "POST",
                format!("/applications/{unknown}/jobruns"),
                Some(job_run_body(
                    &format!("tok-missing-{}", unknown.len()),
                    json!({}),
                )),
                APPLICATION_NOT_FOUND,
            ),
            (
                format!("GetJobRun(unknown application {unknown})"),
                "GET",
                format!("/applications/{unknown}/jobruns/{job_run_id}"),
                None,
                APPLICATION_NOT_FOUND,
            ),
            (
                format!("GetJobRun(unknown job run {unknown})"),
                "GET",
                format!("/applications/{application_id}/jobruns/{unknown}"),
                None,
                JOB_RUN_NOT_FOUND,
            ),
            (
                format!("CancelJobRun(unknown job run {unknown})"),
                "DELETE",
                format!("/applications/{application_id}/jobruns/{unknown}"),
                None,
                JOB_RUN_NOT_FOUND,
            ),
        ];

        for (case, method, uri, body, expected) in cases {
            let answer = call(&router, method, &uri, body).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected 404, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.message(),
                expected,
                "case {case:?}: expected the fixed phrase {expected:?}, got {:?}",
                answer.message()
            );
            assert!(
                !answer.body.to_string().contains(unknown),
                "case {case:?}: the requested id must not be echoed, got {:?}",
                answer.body
            );
        }
    }

    // An attempt that names none is refused for the ATTEMPT, not for the id: nothing
    // retries here, so the stored run has exactly its first attempt, and the id the
    // request named is valid.
    for attempt in [2, 7, 99] {
        let case = format!("GetJobRun(attempt {attempt})");
        let answer = call(
            &router,
            "GET",
            &format!("/applications/{application_id}/jobruns/{job_run_id}?attempt={attempt}"),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.message(),
            JOB_RUN_ATTEMPT_NOT_FOUND,
            "case {case:?}: expected the fixed phrase {JOB_RUN_ATTEMPT_NOT_FOUND:?}, got {:?}",
            answer.message()
        );
        assert!(
            !answer.message().contains(&attempt.to_string()),
            "case {case:?}: the requested attempt must not be echoed, got {:?}",
            answer.message()
        );
    }

    // The attempt the run DOES have is not a not-found at all.
    let found = call(
        &router,
        "GET",
        &format!("/applications/{application_id}/jobruns/{job_run_id}?attempt=1"),
        None,
    )
    .await;
    assert_eq!(
        found.status, 200,
        "case GetJobRun(attempt 1): the run's own attempt must be reported, got {} with {:?}",
        found.status, found.body
    );
}

/// Rule 2: a `@range` violation names the member and the bound and does NOT quote the
/// submitted value.
///
/// The rows span every numeric member this service's model bounds, each on BOTH
/// published bounds and one past each: `maxResults` is `@range(min: 1, max: 50)` on
/// `ListApplications`, `ListJobRuns` and `ListJobRunAttempts`, and
/// `StartJobRun$executionTimeoutMinutes` is a `long` with `@range(min: 0, max:
/// 1000000)` carried in the request BODY rather than the query string
/// (`smithy_query(query="operation",operation="ListApplications,ListJobRuns,ListJobRunAttempts,StartJobRun")`).
#[tokio::test]
async fn a_range_violation_names_the_member_and_the_bound_but_not_the_submitted_value() {
    let router = emulator();
    let application_id = started_application(&router, "tok-range").await;

    // (case, method, uri, body, the clause the refusal must state)
    let refused: Vec<(String, &str, String, Option<Value>, String)> = vec![
        (
            "ListApplications(maxResults=0)".to_string(),
            "GET",
            "/applications?maxResults=0".to_string(),
            None,
            "Value at 'maxResults' failed to satisfy constraint: Member must have value \
             greater than or equal to 1"
                .to_string(),
        ),
        (
            "ListApplications(maxResults=51)".to_string(),
            "GET",
            "/applications?maxResults=51".to_string(),
            None,
            "Value at 'maxResults' failed to satisfy constraint: Member must have value less \
             than or equal to 50"
                .to_string(),
        ),
        (
            "ListJobRuns(maxResults=0)".to_string(),
            "GET",
            format!("/applications/{application_id}/jobruns?maxResults=0"),
            None,
            "Value at 'maxResults' failed to satisfy constraint: Member must have value \
             greater than or equal to 1"
                .to_string(),
        ),
        (
            "ListJobRuns(maxResults=1000)".to_string(),
            "GET",
            format!("/applications/{application_id}/jobruns?maxResults=1000"),
            None,
            "Value at 'maxResults' failed to satisfy constraint: Member must have value less \
             than or equal to 50"
                .to_string(),
        ),
        (
            // Still a stub (HTTP 501) once the input is admissible: the range refusal is
            // the boundary's, so it does not depend on the handler existing.
            "ListJobRunAttempts(maxResults=0)".to_string(),
            "GET",
            format!("/applications/{application_id}/jobruns/0000000000000001/attempts?maxResults=0"),
            None,
            "Value at 'maxResults' failed to satisfy constraint: Member must have value \
             greater than or equal to 1"
                .to_string(),
        ),
        (
            "StartJobRun(executionTimeoutMinutes=-1)".to_string(),
            "POST",
            format!("/applications/{application_id}/jobruns"),
            Some(job_run_body(
                "tok-timeout-low",
                json!({ "executionTimeoutMinutes": -1 }),
            )),
            "Value at 'executionTimeoutMinutes' failed to satisfy constraint: Member must have \
             value greater than or equal to 0"
                .to_string(),
        ),
        (
            "StartJobRun(executionTimeoutMinutes=1000001)".to_string(),
            "POST",
            format!("/applications/{application_id}/jobruns"),
            Some(job_run_body(
                "tok-timeout-high",
                json!({ "executionTimeoutMinutes": 1_000_001i64 }),
            )),
            "Value at 'executionTimeoutMinutes' failed to satisfy constraint: Member must have \
             value less than or equal to 1000000"
                .to_string(),
        ),
    ];

    for (case, method, uri, body, clause) in refused {
        let answer = call(&router, method, &uri, body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.message(),
            format!("1 validation error detected: {clause}"),
            "case {case:?}: expected the member and the bound without the value, got {:?}",
            answer.message()
        );
    }

    // Each published bound is admissible: a refusal that quoted no value would be just as
    // wrong if it refused a value the model admits.
    let admitted: Vec<(String, &str, String, Option<Value>, u16)> = vec![
        (
            "ListApplications(maxResults=1)".to_string(),
            "GET",
            "/applications?maxResults=1".to_string(),
            None,
            200,
        ),
        (
            "ListApplications(maxResults=50)".to_string(),
            "GET",
            "/applications?maxResults=50".to_string(),
            None,
            200,
        ),
        (
            "ListJobRuns(maxResults=50)".to_string(),
            "GET",
            format!("/applications/{application_id}/jobruns?maxResults=50"),
            None,
            200,
        ),
        (
            "StartJobRun(executionTimeoutMinutes=0)".to_string(),
            "POST",
            format!("/applications/{application_id}/jobruns"),
            Some(job_run_body(
                "tok-timeout-min",
                json!({ "executionTimeoutMinutes": 0 }),
            )),
            200,
        ),
        (
            "StartJobRun(executionTimeoutMinutes=1000000)".to_string(),
            "POST",
            format!("/applications/{application_id}/jobruns"),
            Some(job_run_body(
                "tok-timeout-max",
                json!({ "executionTimeoutMinutes": 1_000_000i64 }),
            )),
            200,
        ),
        (
            // An admissible `maxResults` reaches the handler, which is still a stub.
            "ListJobRunAttempts(maxResults=50)".to_string(),
            "GET",
            format!(
                "/applications/{application_id}/jobruns/0000000000000001/attempts?maxResults=50"
            ),
            None,
            501,
        ),
    ];

    for (case, method, uri, body, expected) in admitted {
        let answer = call(&router, method, &uri, body).await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected {expected}, got {} with {:?}",
            answer.status, answer.body
        );
    }
}

/// Rule 3: a resource ARN is checked for its vendor, and an ARN of another vendor is
/// refused by that check — naming the vendor that was expected, not the caller's ARN and
/// not the member's regex.
///
/// The rows span the three operations that take a `resourceArn` (`TagResource`,
/// `UntagResource` and `ListTagsForResource` all bind it to `/tags/{resourceArn}`) and
/// ARNs of several other vendors, in several partitions, both long enough to satisfy
/// `ResourceArn`'s `length 60..1024` and shorter than it — the vendor check is ahead of
/// the member's own constraints either way.
#[tokio::test]
async fn a_resource_arn_of_another_vendor_is_refused_by_the_vendor_check() {
    let router = emulator();

    // Each row is an ARN whose vendor (the third segment of
    // `arn:<partition>:<vendor>:<region>:<account>:<resource>`) is not `emr-serverless`.
    let foreign_arns = [
        // Over the modelled minimum length, which is the shape of the recorded case.
        "arn:aws:s3:::emr-serverless-artifacts-bucket-000000000000000",
        "arn:aws:iam::000000000000:role/EMRServerlessExecutionRoleForTests",
        // `emr` is a different vendor from `emr-serverless`, prefix or not.
        "arn:aws:emr:eu-west-1:000000000000:cluster/j-000000000000000000001",
        // Another partition, and a resource segment that uses `:` rather than `/`.
        "arn:aws-cn:lambda:cn-north-1:000000000000:function:emr-serverless-trigger",
        // Under the modelled minimum length: the vendor is still readable.
        "arn:aws:s3:::b",
    ];

    for arn in foreign_arns {
        let encoded = urlencode(arn);
        let cases: Vec<(String, &str, String, Option<Value>)> = vec![
            (
                format!("ListTagsForResource({arn})"),
                "GET",
                format!("/tags/{encoded}"),
                None,
            ),
            (
                format!("TagResource({arn})"),
                "POST",
                format!("/tags/{encoded}"),
                Some(json!({ "tags": { "owner": "team" } })),
            ),
            (
                format!("UntagResource({arn})"),
                "DELETE",
                format!("/tags/{encoded}?tagKeys=owner"),
                None,
            ),
        ];
        for (case, method, uri, body) in cases {
            let answer = call(&router, method, &uri, body).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                WRONG_VENDOR_CODE,
                "case {case:?}: expected {WRONG_VENDOR_CODE}, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.message(),
                WRONG_VENDOR,
                "case {case:?}: expected the vendor check's message, got {:?}",
                answer.message()
            );
            assert!(
                !answer.body.to_string().contains(arn),
                "case {case:?}: the caller's ARN must not be echoed, got {:?}",
                answer.body
            );
            assert!(
                !answer.body.to_string().contains("regular expression"),
                "case {case:?}: the member's regex must not be stated, got {:?}",
                answer.body
            );
        }
    }

    // An ARN of THIS vendor is not the vendor check's business. One that satisfies
    // `ResourceArn` reaches the handler, which answers on the resource it names (no
    // application was created here, so the modelled `ResourceNotFoundException`); one
    // that does not keeps the framework's constraint wording, which names the member.
    let application_arn =
        "arn:aws:emr-serverless:eu-west-1:000000000000:/applications/0000000000000001";
    let own_vendor_but_not_a_resource =
        "arn:aws:emr-serverless:eu-west-1:000000000000:/clusters/0000000000000001";
    // Not an ARN at all: there is no vendor segment to read, so the same wording stands.
    let not_an_arn = "this-is-not-an-arn-but-it-is-long-enough-to-pass-the-length-check";

    let listed = call(
        &router,
        "GET",
        &format!("/tags/{}", urlencode(application_arn)),
        None,
    )
    .await;
    assert_eq!(
        listed.status, 404,
        "case ListTagsForResource(own vendor): an admissible ARN must reach the handler, got {} \
         with {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.error_code(),
        "ResourceNotFoundException",
        "case ListTagsForResource(own vendor): expected ResourceNotFoundException, got {:?}",
        listed.body
    );
    assert_eq!(
        listed.message(),
        APPLICATION_NOT_FOUND,
        "case ListTagsForResource(own vendor): expected the class phrase {APPLICATION_NOT_FOUND:?}, \
         got {:?}",
        listed.message()
    );

    for (case, value) in [
        ("own vendor, unknown resource type", own_vendor_but_not_a_resource),
        ("not an ARN", not_an_arn),
    ] {
        let answer = call(&router, "GET", &format!("/tags/{}", urlencode(value)), None).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_ne!(
            answer.message(),
            WRONG_VENDOR,
            "case {case:?}: the vendor check must not claim a wrong vendor, got {:?}",
            answer.message()
        );
        assert!(
            answer.message().contains("'resourceArn'"),
            "case {case:?}: the refusal must name the member, got {:?}",
            answer.message()
        );
    }
}
