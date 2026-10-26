// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the refusal `GetBackendJob` answers when the job it addresses
//! does not resolve.
//!
//! The rule under test: a job is addressed by (`AppId`, `BackendEnvironmentName`,
//! `JobId`), every part of that address is STATE, and an address resolving to no job is
//! answered as ABSENT STATE — HTTP 404 with the error code `ResourceNotFoundException`
//! and this service's prefixed "no matching job" sentence — whichever part of it is the
//! unknown one. There is exactly one such refusal: the read does not distinguish an app
//! it holds no backend for from an environment or a job id that was never recorded, and
//! it never answers a request-validation refusal, which would tell a caller its request
//! was malformed when all that happened is that the job does not exist (yet).
//!
//! The rows below differ along what the rule generalises over: WHICH part of the address
//! is unknown, and the form and the size of the unknown value. Each case drives the real
//! wire path (`MultiServiceRouter::dispatch` with this one service registered), so the
//! route templates, the `restJson1` codec and the handler are exercised as a served
//! request exercises them.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_amplifybackend_amplifybackend::state::{
    ComAmazonawsAmplifybackendAmplifybackendState, ERROR_MESSAGE_PREFIX, MAX_IDENTIFIER_CHARS,
    UNRESOLVABLE_JOB_SENTENCE,
};
use service_com_amazonaws_amplifybackend_amplifybackend::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The endpoint host an AmplifyBackend request addresses; its first label is the routing
/// key the router claims the request by.
const HOST: &str = "amplifybackend.us-east-1.amazonaws.com";

/// The wire error code a job read answers for an address that resolves to no job.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The code a request-validation refusal carries, which this read must never answer.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The app this suite registers a backend for, and its two environments.
const APP: &str = "dreadjob1a2b3c";
const ENVIRONMENT: &str = "staging";
const OTHER_ENVIRONMENT: &str = "preview";

/// A second registered app, so a job id can be addressed under the wrong app.
const OTHER_APP: &str = "dreadjob9z8y7x";

/// A router with just this service mounted, and the shared engine it was mounted on.
fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These tests exercise the service, not SigV4: requests carry no signature.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Send one request and read back `(status, body)`. `body` is `Value::Null` when the
/// response carries no JSON document.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let bytes = match &body {
        Some(document) => {
            bytes::Bytes::from(serde_json::to_vec(document).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{HOST}{path}"))
        .header("host", HOST)
        .header("content-type", "application/json")
        .body(bytes)
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let document = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, document)
}

/// A string member of a response, for an assertion that names the case.
fn field<'a>(body: &'a Value, name: &str) -> &'a str {
    body.get(name).and_then(Value::as_str).unwrap_or_default()
}

/// Register a backend environment for `app_id` through `CreateBackend` and hand back the
/// `JobId` the call recorded — the one address that resolves for that environment.
async fn seed(router: &MultiServiceRouter, app_id: &str, environment: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        "/backend",
        Some(json!({
            "appId": app_id,
            "appName": "job reads",
            "backendEnvironmentName": environment,
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "seeding CreateBackend for {app_id:?}/{environment:?} expected 200, got {status} {body}"
    );
    let job_id = field(&body, "jobId").to_string();
    assert!(
        !job_id.is_empty(),
        "seeding CreateBackend for {app_id:?}/{environment:?} recorded no jobId, got {body}"
    );
    job_id
}

/// `GET /backend/{app_id}/job/{environment}/{job_id}`.
fn job_path(app_id: &str, environment: &str, job_id: &str) -> String {
    format!("/backend/{app_id}/job/{environment}/{job_id}")
}

/// The whole message the refusal carries: this service's fixed prefix plus the sentence
/// that says the parameters match no job. It has NO variable part — the refusal names no
/// part of the address, so every row expects exactly this text.
fn expected_message() -> String {
    format!("{ERROR_MESSAGE_PREFIX}{UNRESOLVABLE_JOB_SENTENCE}")
}

/// Assert the one refusal on one response.
fn assert_job_absent(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected 404, got {status} {body}"
    );
    assert_eq!(
        field(body, "code"),
        RESOURCE_NOT_FOUND,
        "case {case:?}: expected code {RESOURCE_NOT_FOUND:?}, got {body}"
    );
    assert_ne!(
        field(body, "code"),
        VALIDATION_EXCEPTION,
        "case {case:?}: absent state is never a request-validation refusal, got {body}"
    );
    assert!(
        field(body, "__type").ends_with(RESOURCE_NOT_FOUND),
        "case {case:?}: the envelope's `__type` agrees with the code, got {body}"
    );
    assert_eq!(
        field(body, "message"),
        expected_message(),
        "case {case:?}: expected message {:?}, got {body}",
        expected_message()
    );
    assert!(
        body.get("resourceType").is_none(),
        "case {case:?}: this refusal carries `message` only, got {body}"
    );
}

/// Every address that resolves to no job earns the SAME refusal, whichever of its three
/// parts is unknown and whatever form that part takes.
#[tokio::test]
async fn an_unresolvable_job_address_is_one_resource_not_found() {
    let (router, _engine) = router_with_engine();
    let staging_job = seed(&router, APP, ENVIRONMENT).await;
    let _preview_job = seed(&router, APP, OTHER_ENVIRONMENT).await;
    let other_app_job = seed(&router, OTHER_APP, ENVIRONMENT).await;

    // (what is unknown about the address, the address)
    //
    // The app ids no registry holds vary in FORM and SIZE: AmplifyBackend models `AppId`
    // as a bare `__string` `@httpLabel` with no pattern or length constraint
    // (`smithy_query(query="operation",operation="GetBackendJob")`), so each of these is a
    // well-formed request that simply names no app.
    let rows: Vec<(&str, String)> = vec![
        (
            "an app id no app ever had",
            job_path("d12440e142099", ENVIRONMENT, &staging_job),
        ),
        (
            "an app id one character past a registered one",
            job_path(&format!("{APP}9"), ENVIRONMENT, &staging_job),
        ),
        (
            "a registered app id with its last character dropped",
            job_path(&APP[..APP.len() - 1], ENVIRONMENT, &staging_job),
        ),
        (
            "an app id outside the pattern Amplify mints ids under",
            job_path("not-an-app-id", ENVIRONMENT, &staging_job),
        ),
        (
            "an app id at this emulator's identifier length bound",
            job_path(
                &format!("d{}", "9".repeat(MAX_IDENTIFIER_CHARS - 1)),
                ENVIRONMENT,
                &staging_job,
            ),
        ),
        (
            "a registered app with no such environment",
            job_path(APP, "nosuchenvironment", &staging_job),
        ),
        (
            "a registered app and environment with a job id never minted",
            job_path(APP, ENVIRONMENT, "999"),
        ),
        (
            "a job id that is not a number at all",
            job_path(APP, ENVIRONMENT, "not-a-job-id"),
        ),
        (
            "a job id at this emulator's identifier length bound",
            job_path(APP, ENVIRONMENT, &"9".repeat(MAX_IDENTIFIER_CHARS)),
        ),
        (
            "a job of this app addressed under its other environment",
            job_path(APP, OTHER_ENVIRONMENT, &staging_job),
        ),
        (
            "a job id one digit longer than one this app minted",
            job_path(APP, ENVIRONMENT, &format!("{staging_job}0")),
        ),
        (
            "an unregistered app with a job id another app minted",
            job_path("dneveregistered", ENVIRONMENT, &other_app_job),
        ),
    ];

    for (case, path) in rows {
        let (status, body) = call(&router, "GET", &path, None).await;
        assert_job_absent(case, status, &body);
    }
}

/// The refusal narrows nothing it should not: every address that DOES resolve is served,
/// including one whose environment has since been removed — a job record outlives the
/// environment it ran against.
#[tokio::test]
async fn an_address_that_resolves_is_still_served() {
    let (router, _engine) = router_with_engine();
    let staging_job = seed(&router, APP, ENVIRONMENT).await;
    let preview_job = seed(&router, APP, OTHER_ENVIRONMENT).await;
    let other_app_job = seed(&router, OTHER_APP, ENVIRONMENT).await;

    let rows = [
        ("the app's first environment", APP, ENVIRONMENT, &staging_job),
        (
            "the app's second environment",
            APP,
            OTHER_ENVIRONMENT,
            &preview_job,
        ),
        (
            "another app's environment",
            OTHER_APP,
            ENVIRONMENT,
            &other_app_job,
        ),
    ];
    for (case, app_id, environment, job_id) in rows {
        let (status, body) = call(&router, "GET", &job_path(app_id, environment, job_id), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            field(&body, "jobId"),
            job_id.as_str(),
            "case {case:?}: the read reports the job asked for, got {body}"
        );
        assert_eq!(
            field(&body, "appId"),
            app_id,
            "case {case:?}: the read reports the app asked for, got {body}"
        );
        assert_eq!(
            field(&body, "backendEnvironmentName"),
            environment,
            "case {case:?}: the read reports the environment asked for, got {body}"
        );
    }

    let (status, body) = call(
        &router,
        "POST",
        &format!("/backend/{APP}/environments/{ENVIRONMENT}/remove"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"remove the environment\": DeleteBackend expected 200, got {status} {body}"
    );
    let (status, body) = call(
        &router,
        "GET",
        &job_path(APP, ENVIRONMENT, &staging_job),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a job whose environment was removed\": expected 200, got {status} {body}"
    );
}

/// The refusal is a REPORT: it stores nothing, registers no app, and leaves every job
/// that does resolve reading back exactly as it did before.
#[tokio::test]
async fn a_refused_job_read_changes_nothing() {
    let (router, engine) = router_with_engine();
    let staging_job = seed(&router, APP, ENVIRONMENT).await;

    let unknown = job_path("dnotregistered", ENVIRONMENT, "999");
    let (first_status, first_body) = call(&router, "GET", &unknown, None).await;
    assert_job_absent("the first refusal", first_status, &first_body);
    let (second_status, second_body) = call(&router, "GET", &unknown, None).await;
    assert_job_absent("the repeated refusal", second_status, &second_body);
    assert_eq!(
        first_body, second_body,
        "case \"repeated refusal\": the same address answers the same body, got {first_body} vs {second_body}"
    );

    {
        let store = engine.state::<ComAmazonawsAmplifybackendAmplifybackendState>();
        let held = store.lock();
        assert!(
            !held.apps.contains_key("dnotregistered"),
            "case \"repeated refusal\": a refused read registers no app, got {:?}",
            held.apps.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            held.apps.len(),
            1,
            "case \"repeated refusal\": the store still holds only the seeded app, got {:?}",
            held.apps.keys().collect::<Vec<_>>()
        );
    }

    let (status, body) = call(
        &router,
        "GET",
        &job_path(APP, ENVIRONMENT, &staging_job),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a resolvable address after two refusals\": expected 200, got {status} {body}"
    );
    assert_eq!(
        field(&body, "jobId"),
        staging_job,
        "case \"a resolvable address after two refusals\": the job is unchanged, got {body}"
    );
}
