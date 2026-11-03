// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the PRECEDENCE of the two refusals `GetBackendJob` answers for an
//! address that resolves to no job.
//!
//! The rule under test: `AppId` names an IDENTITY, so it is resolved against the Amplify
//! app registry before anything else — an app id no registered app exists for is a
//! permanent client-input error (HTTP 400, error code `ValidationException`, the prefixed
//! "invalid appId" sentence) whatever the rest of the request names. Only under an app
//! that DOES exist is the rest of the address state, and there an address resolving to no
//! job is answered as ABSENT STATE — HTTP 404 with the error code
//! `ResourceNotFoundException` and this service's prefixed "no matching job" sentence —
//! whether the environment or the job id is the unknown one.
//!
//! The rows below differ along what the rule generalises over: which SIDE of the gate the
//! case falls on, the form and the size of the unregistered app id, and WHICH part of the
//! address below the gate is unknown. Each case drives the real wire path
//! (`MultiServiceRouter::dispatch` with this one service registered), so the route
//! templates, the `restJson1` codec and the handler are exercised as a served request
//! exercises them.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_amplifybackend_amplifybackend::state::{
    invalid_app_id_message, ComAmazonawsAmplifybackendAmplifybackendState, ERROR_MESSAGE_PREFIX,
    MAX_IDENTIFIER_CHARS, UNRESOLVABLE_JOB_SENTENCE,
};
use service_com_amazonaws_amplifybackend_amplifybackend::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The endpoint host an AmplifyBackend request addresses; its first label is the routing
/// key the router claims the request by.
const HOST: &str = "amplifybackend.us-east-1.amazonaws.com";

/// The wire error code a job read answers for an address that resolves to no job.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The code the app-identity refusal carries, which an address under an app that exists
/// must never answer.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The app this suite registers a backend for, and its two environments.
const APP: &str = "dreadjob1a2b3c";
const ENVIRONMENT: &str = "staging";
const OTHER_ENVIRONMENT: &str = "preview";

/// A second registered app, so a job id can be addressed under the wrong app.
const OTHER_APP: &str = "dreadjob9z8y7x";

/// A router with just this service mounted, and the shared engine it was mounted on, with
/// an Amplify app registered for both app ids the cases address — the registry
/// `CreateBackend` settles an app's identity against (`crate::amplify_registry`).
fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    for app_id in [APP, OTHER_APP] {
        crate::amplify_registry::register_amplify_app(&engine, app_id, "job reads");
    }
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

/// Assert the app-identity refusal on one response.
fn assert_app_invalid(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} {body}"
    );
    assert_eq!(
        field(body, "code"),
        VALIDATION_EXCEPTION,
        "case {case:?}: expected code {VALIDATION_EXCEPTION:?}, got {body}"
    );
    assert_ne!(
        field(body, "code"),
        RESOURCE_NOT_FOUND,
        "case {case:?}: an app that does not exist is never reported as an absent job, got {body}"
    );
    assert!(
        field(body, "__type").ends_with(VALIDATION_EXCEPTION),
        "case {case:?}: the envelope's `__type` agrees with the code, got {body}"
    );
    assert_eq!(
        field(body, "message"),
        invalid_app_id_message(),
        "case {case:?}: expected message {:?}, got {body}",
        invalid_app_id_message()
    );
    assert!(
        field(body, "message").starts_with(ERROR_MESSAGE_PREFIX),
        "case {case:?}: the message carries this service's fixed prefix, got {body}"
    );
}

/// An `AppId` no registered Amplify app exists for is refused as the permanent
/// client-input error it is, BEFORE the job lookup — whatever form that id takes and
/// whatever the rest of the address names, up to and including a job id another app really
/// did mint.
#[tokio::test]
async fn an_unregistered_app_id_is_refused_before_the_job_lookup() {
    let (router, _engine) = router_with_engine();
    let staging_job = seed(&router, APP, ENVIRONMENT).await;
    let other_app_job = seed(&router, OTHER_APP, ENVIRONMENT).await;

    // (the case, the address)
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
            "the shortest app id Amplify's pattern allows",
            job_path("d0", ENVIRONMENT, &staging_job),
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
            "an unregistered app with an environment nothing ever created",
            job_path("dneveregistered", "nosuchenvironment", "999"),
        ),
        (
            "an unregistered app with a job id another app minted",
            job_path("dneveregistered", ENVIRONMENT, &other_app_job),
        ),
    ];

    for (case, path) in rows {
        let (status, body) = call(&router, "GET", &path, None).await;
        assert_app_invalid(case, status, &body);
    }
}

/// Under an app that DOES exist, every address that resolves to no job earns the SAME
/// job-absent refusal, whichever of the two remaining parts is unknown and whatever form
/// that part takes.
#[tokio::test]
async fn an_unresolvable_job_address_is_one_resource_not_found() {
    let (router, _engine) = router_with_engine();
    let staging_job = seed(&router, APP, ENVIRONMENT).await;
    let _preview_job = seed(&router, APP, OTHER_ENVIRONMENT).await;
    let _other_app_job = seed(&router, OTHER_APP, ENVIRONMENT).await;

    // (what is unknown about the address, the address)
    let rows: Vec<(&str, String)> = vec![
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
            "an environment name outside the form this service creates",
            job_path(APP, "NOT-A-VALID-ENV", "999"),
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

/// Either refusal is a REPORT: it stores nothing, registers no app, and leaves every job
/// that does resolve reading back exactly as it did before.
#[tokio::test]
async fn a_refused_job_read_changes_nothing() {
    let (router, engine) = router_with_engine();
    let staging_job = seed(&router, APP, ENVIRONMENT).await;

    // Both sides of the gate, each asked twice: the identity refusal and the job-absent
    // refusal answer the same body every time and leave the store as it was.
    let absent = job_path(APP, ENVIRONMENT, "999");
    let (first_status, first_body) = call(&router, "GET", &absent, None).await;
    assert_job_absent("the first job-absent refusal", first_status, &first_body);
    let (second_status, second_body) = call(&router, "GET", &absent, None).await;
    assert_job_absent("the repeated job-absent refusal", second_status, &second_body);
    assert_eq!(
        first_body, second_body,
        "case \"repeated job-absent refusal\": the same address answers the same body, got {first_body} vs {second_body}"
    );

    let unregistered = job_path("dnotregistered", ENVIRONMENT, "999");
    let (first_status, first_body) = call(&router, "GET", &unregistered, None).await;
    assert_app_invalid("the first identity refusal", first_status, &first_body);
    let (second_status, second_body) = call(&router, "GET", &unregistered, None).await;
    assert_app_invalid("the repeated identity refusal", second_status, &second_body);
    assert_eq!(
        first_body, second_body,
        "case \"repeated identity refusal\": the same address answers the same body, got {first_body} vs {second_body}"
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
        "case \"a resolvable address after four refusals\": expected 200, got {status} {body}"
    );
    assert_eq!(
        field(&body, "jobId"),
        staging_job,
        "case \"a resolvable address after four refusals\": the job is unchanged, got {body}"
    );
}
