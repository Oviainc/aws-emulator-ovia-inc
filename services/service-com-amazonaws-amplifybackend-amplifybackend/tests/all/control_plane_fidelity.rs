// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AmplifyBackend control plane's recorded refusals, message
//! form and job vocabulary.
//!
//! Each case drives the real wire path — `MultiServiceRouter::dispatch` with this one
//! service registered — so the route templates, the `restJson1` codec, the modelled
//! constraints and the handlers are exercised exactly as a served request does. The rules
//! under test, each over a SET of inputs rather than one:
//!
//! 1. a `BackendEnvironmentName` a create would bring into being is accepted only inside
//!    the form the service creates — 2 to 10 lowercase letters — and refused outside it
//!    with HTTP 400 `ValidationException` and one fixed message, storing nothing;
//! 2. an `AppId` Amplify holds no app for is refused on the create path as the missing app
//!    it names (404 `NotFoundException`), before any environment work, and no backend
//!    comes into being for it;
//! 3. an operation AWS has withdrawn from SDK and AWS CLI callers refuses EVERY call with
//!    HTTP 400 `ValidationException` and its fixed message, whatever the request carries,
//!    and leaves no resource and no job behind;
//! 4. a resource READ that does not resolve answers HTTP 404 with the error code
//!    `ResourceNotFoundException` and ONE sentence, naming no part of the address, so a
//!    caller cannot tell which link of it was missing;
//! 5. every error message this service renders begins with the service's fixed prefix, and
//!    where it names a resource it names it from the request;
//! 6. a job reports the service's own status vocabulary: `PENDING` or `RUNNING` while it
//!    is in flight and `COMPLETED` once the instant its work was due has passed.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifybackend_amplifybackend::state::{
    amplify_cli_only_message, environment_missing_sentence, environment_name_message,
    studio_only_message, ComAmazonawsAmplifybackendAmplifybackendState, AUTH_MISSING_SENTENCE,
    ERROR_MESSAGE_PREFIX, JOB_COMPLETION_DELAY, JOB_STATUS_COMPLETED, JOB_STATUS_FAILED,
    JOB_STATUS_PENDING, JOB_STATUS_RUNNING, UNRESOLVABLE_JOB_SENTENCE,
};
use service_com_amazonaws_amplifybackend_amplifybackend::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The endpoint host an AmplifyBackend request addresses; its first label is the routing
/// key the router claims the request by.
const HOST: &str = "amplifybackend.us-east-1.amazonaws.com";

/// The app every case addresses, and the environment most of them start from.
const APP: &str = "d1fidelity0001";
const ENVIRONMENT: &str = "staging";

/// The wire error codes under test.
const VALIDATION_EXCEPTION: &str = "ValidationException";
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";
const NOT_FOUND_EXCEPTION: &str = "NotFoundException";

// The recorded messages, written out here as literals rather than imported, so this suite
// pins the WORDING independently of the builders the handlers use: the rows below compare a
// response against the builder (which carries the variable parts), and
// `the_recorded_messages_are_worded_as_the_service_words_them` compares each builder
// against the literal, so a change to either side is caught.

/// What a request whose `BackendEnvironmentName` is outside the form is refused with.
const ENVIRONMENT_NAME_REFUSAL: &str = "An error occurred while processing your request: \
     Environment name must be between 2 and 10 characters, and lowercase only.";

/// What an operation AWS serves only through Amplify Studio refuses every call with.
const STUDIO_ONLY_REFUSAL: &str = "An error occurred while processing your request: \
     Operation not supported through SDK/AWS CLI. Please use Amplify Studio";

/// What `CloneBackend` refuses every call with — fixed text, including the placeholder and
/// the sample environment name AWS puts in its own command hint.
const AMPLIFY_CLI_ONLY_REFUSAL: &str = "An error occurred while processing your request: \
     Operation not supported through SDK/AWS CLI. Please use \"amplify pull --appId \
     <appId> --envName staging && amplify add env && amplify push\" from Amplify CLI";

/// What a lookup of a backend environment that does not exist answers, for `name`.
fn environment_missing_refusal(name: &str) -> String {
    format!(
        "An error occurred while processing your request: \
         BackendEnvironment {name} does not exist."
    )
}

/// What a job lookup that does not resolve answers.
const JOB_MISSING_REFUSAL: &str = "An error occurred while processing your request: \
     Unable to find matching job with provided parameters.";

/// What an auth-resource read that does not resolve answers.
const AUTH_MISSING_REFUSAL: &str = "An error occurred while processing your request: \
     Amplify auth parameters file not found.";

/// What a request naming an app no Amplify app exists for is refused with, for `app_id`.
fn app_missing_refusal(app_id: &str) -> String {
    format!("An error occurred while processing your request: App {app_id} not found.")
}

/// Every message this service renders is worded as the recordings word it: each builder the
/// handlers use answers the literal this suite holds, so the rows that compare a response
/// against a builder pin the wording too.
#[test]
fn the_recorded_messages_are_worded_as_the_service_words_them() {
    for (case, rendered, recorded) in [
        (
            "the environment-name rule",
            environment_name_message(),
            ENVIRONMENT_NAME_REFUSAL.to_string(),
        ),
        (
            "an operation AWS serves only through Studio",
            studio_only_message(),
            STUDIO_ONLY_REFUSAL.to_string(),
        ),
        (
            "an operation AWS serves through the Amplify CLI",
            amplify_cli_only_message(),
            AMPLIFY_CLI_ONLY_REFUSAL.to_string(),
        ),
        (
            "a backend environment that does not exist",
            format!(
                "{ERROR_MESSAGE_PREFIX}{}",
                environment_missing_sentence(ENVIRONMENT)
            ),
            environment_missing_refusal(ENVIRONMENT),
        ),
        (
            "a job lookup that does not resolve",
            format!("{ERROR_MESSAGE_PREFIX}{UNRESOLVABLE_JOB_SENTENCE}"),
            JOB_MISSING_REFUSAL.to_string(),
        ),
        (
            "an auth read that does not resolve",
            format!("{ERROR_MESSAGE_PREFIX}{AUTH_MISSING_SENTENCE}"),
            AUTH_MISSING_REFUSAL.to_string(),
        ),
    ] {
        assert_eq!(
            rendered, recorded,
            "case {case:?}: expected {recorded:?}, got {rendered:?}"
        );
    }
}

/// A router with just this service mounted, and the shared engine it was mounted on, with
/// an Amplify app registered for every id in `app_ids` — the registry this service settles
/// an app's identity against (`crate::amplify_registry`).
fn router_with_apps<S: AsRef<str>>(
    app_ids: &[S],
) -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    for app_id in app_ids {
        crate::amplify_registry::register_amplify_app(&engine, app_id.as_ref(), "fidelity");
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
            Bytes::from(serde_json::to_vec(document).expect("serialize the request body"))
        }
        None => Bytes::new(),
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

/// `CreateBackend` for `(app_id, environment)`.
async fn create_backend(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
) -> (u16, Value) {
    call(
        router,
        "POST",
        "/backend",
        Some(json!({
            "appId": app_id,
            "appName": "fidelity",
            "backendEnvironmentName": environment,
        })),
    )
    .await
}

/// `DeleteBackend` for `(app_id, environment)`.
async fn delete_backend(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/backend/{app_id}/environments/{environment}/remove"),
        Some(json!({})),
    )
    .await
}

/// `GetBackendJob` for `(app_id, environment, job_id)`.
async fn get_backend_job(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    job_id: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/backend/{app_id}/job/{environment}/{job_id}"),
        None,
    )
    .await
}

/// The environments `GetBackend` reports for `app_id`.
async fn environments(router: &MultiServiceRouter, app_id: &str) -> Vec<String> {
    let (_, body) = call(
        router,
        "POST",
        &format!("/backend/{app_id}/details"),
        Some(json!({})),
    )
    .await;
    body.get("backendEnvironmentList")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// How many job records the app holds, read from the store rather than over the wire, so a
/// refusal that recorded a job cannot hide behind a filter.
fn recorded_jobs(engine: &engine::InMemoryEngine, app_id: &str) -> usize {
    let state = engine.state::<ComAmazonawsAmplifybackendAmplifybackendState>();
    let held = state.lock();
    held.app(app_id).map_or(0, |app| app.jobs.len())
}

/// A minimal valid `CreateBackendAuthResourceConfig`, as the wire document. The enum
/// values come from the model's own value lists (`AuthResources`, `Service`,
/// `SignInMethod`, `RequiredSignUpAttributesElement` in `src/ops/types/shared_types.rs`).
fn minimal_auth_config() -> Value {
    json!({
        "authResources": "USER_POOL_ONLY",
        "service": "COGNITO",
        "userPoolConfigs": {
            "requiredSignUpAttributes": ["EMAIL"],
            "signInMethod": "EMAIL",
            "userPoolName": "fidelityPool"
        }
    })
}

/// A deeply nested `CreateBackendAuthResourceConfig`, so the withdrawal is exercised on a
/// request whose SHAPE is as large as the model allows, not only on a minimal one.
fn nested_auth_config() -> Value {
    json!({
        "authResources": "IDENTITY_POOL_AND_USER_POOL",
        "service": "COGNITO",
        "identityPoolConfigs": {
            "identityPoolName": "fidelityIdPool",
            "unauthenticatedLogin": true
        },
        "userPoolConfigs": {
            "requiredSignUpAttributes": ["ADDRESS", "PHONE_NUMBER"],
            "signInMethod": "EMAIL_AND_PHONE_NUMBER",
            "userPoolName": "fidelityPool",
            "mfa": {
                "MFAMode": "OPTIONAL",
                "settings": { "mfaTypes": ["SMS"], "smsMessage": "code {####}" }
            },
            "passwordPolicy": {
                "minimumLength": 12.0,
                "additionalConstraints": ["REQUIRE_DIGIT"]
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Rule 1 — the form of a backend environment name a create brings into being
// ---------------------------------------------------------------------------

/// Every name inside the form is created and reported; every name outside it is refused
/// with HTTP 400 `ValidationException` and the fixed message, and nothing is stored for
/// it.
///
/// The rows differ along what the rule generalises over — the LENGTH of the name (one
/// character below the minimum, the minimum itself, the maximum itself, one past the
/// maximum) and the ALPHABET it is drawn from (letters, an uppercase letter, a digit, a
/// hyphen, an underscore, a space, a non-ASCII letter).
#[tokio::test]
async fn an_environment_name_outside_the_form_the_service_creates_is_refused() {
    let (router, engine) = router_with_apps(&[APP]);

    // (case, name, inside the form)
    let rows: Vec<(&str, &str, bool)> = vec![
        ("the shortest name the form allows", "ab", true),
        ("a name between the bounds", "dev", true),
        ("the name most apps start with", ENVIRONMENT, true),
        ("the longest name the form allows", "abcdefghij", true),
        ("one character short of the minimum", "a", false),
        ("one character past the maximum", "abcdefghijk", false),
        ("a name carrying an uppercase letter", "Staging", false),
        ("a name carrying a digit", "env1", false),
        ("a name carrying a hyphen", "env-name", false),
        ("a name carrying an underscore", "env_name", false),
        ("a name carrying a space", "env name", false),
        ("a name outside ASCII", "énvir", false),
        // Far past the maximum and outside the alphabet at once.
        ("a long hyphenated name", "env-name-too-long-to-be-valid", false),
    ];

    let mut created: Vec<String> = Vec::new();
    for (case, name, inside) in rows {
        let (status, body) = create_backend(&router, APP, name).await;
        if inside {
            assert_eq!(
                status, 200,
                "case {case:?} ({name:?}): a name inside the form is created, got {status} {body}"
            );
            assert_eq!(
                field(&body, "backendEnvironmentName"),
                name,
                "case {case:?}: the answer names the environment created, got {body}"
            );
            created.push(name.to_string());
            created.sort();
            assert_eq!(
                environments(&router, APP).await,
                created,
                "case {case:?}: GetBackend reports the environments created, got {created:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?} ({name:?}): a name outside the form is refused, got {status} {body}"
            );
            assert_eq!(
                field(&body, "code"),
                VALIDATION_EXCEPTION,
                "case {case:?}: expected code {VALIDATION_EXCEPTION:?}, got {body}"
            );
            assert_eq!(
                field(&body, "message"),
                environment_name_message(),
                "case {case:?}: the refusal states the rule that was broken, got {body}"
            );
            for member in ["jobId", "status", "backendEnvironmentName"] {
                assert!(
                    body.get(member).is_none(),
                    "case {case:?}: a refused create reports no {member}, got {body}"
                );
            }
            assert_eq!(
                environments(&router, APP).await,
                created,
                "case {case:?}: a refused create stores no environment, got {body}"
            );
        }
    }

    // The refusals recorded no job either: only the accepted creates did.
    assert_eq!(
        recorded_jobs(&engine, APP),
        created.len(),
        "a refused create records no job: expected one per accepted create ({})",
        created.len()
    );
}

/// A name outside the form is refused where it would be CREATED, and nowhere else: a read
/// that merely ADDRESSES such a name answers the same not-found as any other environment
/// that does not exist, so a caller is never told its request is malformed for addressing
/// an environment the service itself could not have created.
#[tokio::test]
async fn a_name_outside_the_form_still_addresses_a_missing_environment_on_a_read() {
    let (router, _engine) = router_with_apps(&[APP]);
    let (status, body) = create_backend(&router, APP, ENVIRONMENT).await;
    assert_eq!(status, 200, "seeding CreateBackend expected 200, got {body}");

    for name in ["env-name-too-long-to-be-valid", "A", "env_1"] {
        let (status, body) = get_backend_job(&router, APP, name, "1").await;
        assert_eq!(
            status, 404,
            "case {name:?}: a read of an environment that does not exist is a not-found, got {status} {body}"
        );
        assert_eq!(
            field(&body, "code"),
            RESOURCE_NOT_FOUND_EXCEPTION,
            "case {name:?}: expected code {RESOURCE_NOT_FOUND_EXCEPTION:?}, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2 — app identity on the create path
// ---------------------------------------------------------------------------

/// `CreateBackend` for an `AppId` Amplify holds no app for answers the missing app it
/// names (404 `NotFoundException`), before any environment work, and brings no backend
/// into being; the same call for an app Amplify DID mint succeeds.
///
/// The rows differ in the FORM and SIZE of the id, and one of them pairs the unknown id
/// with a name outside the form of rule 1 — identity is settled first, so that row answers
/// the missing app and not the name.
#[tokio::test]
async fn create_backend_refuses_an_app_id_amplify_holds_no_app_for() {
    let (router, engine) = router_with_apps(&[APP]);

    // (case, app id, environment name)
    let rows: Vec<(&str, String, &str)> = vec![
        ("an id no app ever had", "d12440e142099".to_string(), "dev"),
        (
            "one character past the registered id",
            format!("{APP}9"),
            "dev",
        ),
        (
            "the registered id with its last character dropped",
            APP[..APP.len() - 1].to_string(),
            "dev",
        ),
        (
            "the shortest id Amplify's pattern allows",
            "d0".to_string(),
            "dev",
        ),
        (
            "an id outside Amplify's pattern altogether",
            "not-an-app-id".to_string(),
            "dev",
        ),
        (
            "an unknown id and a name outside the form at once",
            "d12440e142099".to_string(),
            "env-name-too-long-to-be-valid",
        ),
    ];

    for (case, app_id, environment) in rows {
        let (status, body) = create_backend(&router, &app_id, environment).await;
        assert_eq!(
            status, 404,
            "case {case:?} ({app_id:?}): expected 404, got {status} {body}"
        );
        assert_eq!(
            field(&body, "code"),
            NOT_FOUND_EXCEPTION,
            "case {case:?}: expected code {NOT_FOUND_EXCEPTION:?}, got {body}"
        );
        assert_eq!(
            field(&body, "message"),
            app_missing_refusal(&app_id),
            "case {case:?}: the refusal names the app the request asked for, got {body}"
        );
        assert!(
            !field(&body, "resourceType").is_empty(),
            "case {case:?}: the modelled refusal names the resource type, got {body}"
        );
        assert_eq!(
            recorded_jobs(&engine, &app_id),
            0,
            "case {case:?}: a refused create records no job for an app that does not exist"
        );
        let state = engine.state::<ComAmazonawsAmplifybackendAmplifybackendState>();
        assert!(
            !state.lock().apps.contains_key(&app_id),
            "case {case:?}: a refused create brings no backend into being"
        );
    }

    // The app Amplify minted is served.
    let (status, body) = create_backend(&router, APP, ENVIRONMENT).await;
    assert_eq!(
        status, 200,
        "case \"an app Amplify minted\": expected 200, got {status} {body}"
    );
    assert_eq!(
        environments(&router, APP).await,
        vec![ENVIRONMENT.to_string()],
        "case \"an app Amplify minted\": its environment is created"
    );
}

// ---------------------------------------------------------------------------
// Rule 3 — the operations AWS has withdrawn from this channel
// ---------------------------------------------------------------------------

/// `CreateBackendAuth` refuses EVERY call with HTTP 400 `ValidationException` and its
/// fixed message, whatever the request carries, and stores neither an auth resource nor a
/// job — so the read of that resource answers the recorded not-found afterwards.
///
/// The rows differ in the SHAPE of the configuration, the resource name and whether the
/// environment named exists: the refusal is input-independent, so every row answers the
/// same message.
#[tokio::test]
async fn create_backend_auth_is_refused_on_every_call() {
    let (router, engine) = router_with_apps(&[APP]);
    let (status, body) = create_backend(&router, APP, ENVIRONMENT).await;
    assert_eq!(status, 200, "seeding CreateBackend expected 200, got {body}");
    let jobs_before = recorded_jobs(&engine, APP);

    // (case, environment, resource name, configuration)
    let rows: Vec<(&str, &str, &str, Value)> = vec![
        (
            "a minimal configuration in an environment that exists",
            ENVIRONMENT,
            "authone",
            minimal_auth_config(),
        ),
        (
            "a deeply nested configuration",
            ENVIRONMENT,
            "authtwo",
            nested_auth_config(),
        ),
        (
            "an environment that was never created",
            "nosuchenv",
            "authone",
            minimal_auth_config(),
        ),
        (
            "a resource name that is one character",
            ENVIRONMENT,
            "a",
            minimal_auth_config(),
        ),
    ];

    for (case, environment, resource_name, config) in rows {
        let (status, body) = call(
            &router,
            "POST",
            &format!("/backend/{APP}/auth"),
            Some(json!({
                "backendEnvironmentName": environment,
                "resourceName": resource_name,
                "resourceConfig": config,
            })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            field(&body, "code"),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected code {VALIDATION_EXCEPTION:?}, got {body}"
        );
        assert_eq!(
            field(&body, "message"),
            studio_only_message(),
            "case {case:?}: the refusal names the channel the operation is reachable through, got {body}"
        );
        for member in ["jobId", "status", "operation", "appId"] {
            assert!(
                body.get(member).is_none(),
                "case {case:?}: a refused create reports no {member}, got {body}"
            );
        }
        assert_eq!(
            recorded_jobs(&engine, APP),
            jobs_before,
            "case {case:?}: a refused create records no job"
        );

        // …and the resource it would have created is not there to read.
        let (status, body) = call(
            &router,
            "POST",
            &format!("/backend/{APP}/auth/{environment}/details"),
            Some(json!({ "resourceName": resource_name })),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: GetBackendAuth expected 404, got {status} {body}"
        );
        assert_eq!(
            field(&body, "code"),
            RESOURCE_NOT_FOUND_EXCEPTION,
            "case {case:?}: expected code {RESOURCE_NOT_FOUND_EXCEPTION:?}, got {body}"
        );
        assert_eq!(
            field(&body, "message"),
            format!("{ERROR_MESSAGE_PREFIX}{AUTH_MISSING_SENTENCE}"),
            "case {case:?}: the read answers one fixed sentence, got {body}"
        );
    }
}

/// `CloneBackend` refuses EVERY call with HTTP 400 `ValidationException` and the Amplify
/// CLI guidance, whatever the target name is, and creates no environment.
///
/// The rows differ in the TARGET name — one free, one already in use, the source's own
/// name, one outside the form the service creates — and in whether the SOURCE environment
/// exists: the refusal precedes every one of those checks, so all of them answer the same
/// message.
#[tokio::test]
async fn clone_backend_is_refused_on_every_call() {
    let (router, engine) = router_with_apps(&[APP]);
    for environment in [ENVIRONMENT, "inuse"] {
        let (status, body) = create_backend(&router, APP, environment).await;
        assert_eq!(
            status, 200,
            "seeding CreateBackend for {environment:?} expected 200, got {body}"
        );
    }
    let jobs_before = recorded_jobs(&engine, APP);
    let before = environments(&router, APP).await;

    // (case, source environment, target environment)
    let rows: Vec<(&str, &str, &str)> = vec![
        ("a target name nothing uses", ENVIRONMENT, "fresh"),
        ("a target name already in use", ENVIRONMENT, "inuse"),
        ("the source's own name", ENVIRONMENT, ENVIRONMENT),
        (
            "a target name outside the form the service creates",
            ENVIRONMENT,
            "target-1",
        ),
        ("a source environment that does not exist", "nosuchenv", "fresh"),
    ];

    for (case, source, target) in rows {
        let (status, body) = call(
            &router,
            "POST",
            &format!("/backend/{APP}/environments/{source}/clone"),
            Some(json!({ "targetEnvironmentName": target })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            field(&body, "code"),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected code {VALIDATION_EXCEPTION:?}, got {body}"
        );
        assert_eq!(
            field(&body, "message"),
            amplify_cli_only_message(),
            "case {case:?}: every call is refused with the same fixed message, got {body}"
        );
        assert_eq!(
            environments(&router, APP).await,
            before,
            "case {case:?}: a refused clone creates no environment"
        );
        assert_eq!(
            recorded_jobs(&engine, APP),
            jobs_before,
            "case {case:?}: a refused clone records no job"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4 and 5 — the not-found a read answers, and the form of every message
// ---------------------------------------------------------------------------

/// Every `GetBackendJob` whose (app, environment, job) triple resolves to no job answers
/// HTTP 404, the error code `ResourceNotFoundException` and ONE sentence — whichever link
/// of the address is missing.
///
/// The rows differ in WHICH link fails: an app that exists with no backend at all, an
/// environment that was never created, an environment whose jobs were all recorded under
/// another name, a job id nothing minted, and a job id that exists under a DIFFERENT
/// environment of the same app.
#[tokio::test]
async fn an_unresolvable_job_lookup_is_one_resource_not_found() {
    let app_with_no_backend = "d1nobackend001";
    let (router, _engine) = router_with_apps(&[APP, app_with_no_backend]);
    let (status, body) = create_backend(&router, APP, ENVIRONMENT).await;
    assert_eq!(status, 200, "seeding CreateBackend expected 200, got {body}");
    let job_id = field(&body, "jobId").to_string();
    let (status, body) = create_backend(&router, APP, "other").await;
    assert_eq!(status, 200, "seeding CreateBackend expected 200, got {body}");

    // (case, app id, environment, job id)
    let rows: Vec<(&str, &str, &str, String)> = vec![
        (
            "an app Amplify minted that has no backend",
            app_with_no_backend,
            ENVIRONMENT,
            job_id.clone(),
        ),
        (
            "an environment that was never created",
            APP,
            "nosuchenv",
            job_id.clone(),
        ),
        ("a job id nothing minted", APP, ENVIRONMENT, "1".to_string()),
        (
            "a job id past the ones minted",
            APP,
            ENVIRONMENT,
            format!("{job_id}0"),
        ),
        (
            "a job id of another environment of the same app",
            APP,
            "other",
            job_id.clone(),
        ),
    ];

    for (case, app_id, environment, job) in rows {
        let (status, body) = get_backend_job(&router, app_id, environment, &job).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            field(&body, "code"),
            RESOURCE_NOT_FOUND_EXCEPTION,
            "case {case:?}: expected code {RESOURCE_NOT_FOUND_EXCEPTION:?}, got {body}"
        );
        assert_eq!(
            field(&body, "message"),
            format!("{ERROR_MESSAGE_PREFIX}{UNRESOLVABLE_JOB_SENTENCE}"),
            "case {case:?}: every unresolvable lookup answers the same sentence, got {body}"
        );
        for member in [app_id, environment, job.as_str()] {
            assert!(
                !field(&body, "message").contains(member),
                "case {case:?}: the sentence names no part of the address, got {body}"
            );
        }
    }

    // The job that DOES resolve still does: the collapsed refusal narrows nothing.
    let (status, body) = get_backend_job(&router, APP, ENVIRONMENT, &job_id).await;
    assert_eq!(
        status, 200,
        "case \"the job that exists\": expected 200, got {status} {body}"
    );
}

/// An environment a mutation addresses that the app does not have — never created, or
/// already removed — answers the modelled `NotFoundException` (404) with the service's
/// sentence, naming the environment FROM THE REQUEST, and the second removal of one
/// answers exactly that.
///
/// The rows differ in the environment NAME and in why it is absent (never created, removed
/// a moment ago, belonging to an app with no backend at all).
#[tokio::test]
async fn a_second_removal_reports_the_environment_as_missing_by_name() {
    let app_with_no_backend = "d1nobackend002";
    let (router, _engine) = router_with_apps(&[APP, app_with_no_backend]);

    // Each name is created, removed, and then removed again: the second removal is the
    // case under test. The names differ in length and in form inside the one the service
    // creates.
    for name in ["ab", ENVIRONMENT, "abcdefghij"] {
        let (status, body) = create_backend(&router, APP, name).await;
        assert_eq!(
            status, 200,
            "case {name:?}: seeding CreateBackend expected 200, got {status} {body}"
        );
        let (status, body) = delete_backend(&router, APP, name).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the first DeleteBackend expected 200, got {status} {body}"
        );

        let (status, body) = delete_backend(&router, APP, name).await;
        assert_eq!(
            status, 404,
            "case {name:?}: the second DeleteBackend expected 404, got {status} {body}"
        );
        assert_eq!(
            field(&body, "code"),
            NOT_FOUND_EXCEPTION,
            "case {name:?}: expected code {NOT_FOUND_EXCEPTION:?}, got {body}"
        );
        assert_eq!(
            field(&body, "message"),
            format!(
                "{ERROR_MESSAGE_PREFIX}{}",
                environment_missing_sentence(name)
            ),
            "case {name:?}: the refusal names the environment from the request, got {body}"
        );
        assert!(
            !field(&body, "message").contains(APP),
            "case {name:?}: the sentence is about the environment, not the app, got {body}"
        );
    }

    // The same answer for an environment never created, and for an app that holds no
    // backend at all: an app with no backend holds no environment either.
    for (case, app_id, name) in [
        ("an environment never created", APP, "nosuchenv"),
        ("an app with no backend at all", app_with_no_backend, "ab"),
    ] {
        let (status, body) = delete_backend(&router, app_id, name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            field(&body, "message"),
            format!(
                "{ERROR_MESSAGE_PREFIX}{}",
                environment_missing_sentence(name)
            ),
            "case {case:?}: the refusal names the environment from the request, got {body}"
        );
    }
}

/// Every error this service renders carries the fixed prefix, whatever its code and status:
/// the rows span the identity refusal, the missing app on the create path, the
/// environment-name rule, the withdrawal, the modelled not-found, the recorded
/// `ResourceNotFoundException` and the modelled `BadRequestException`.
#[tokio::test]
async fn every_error_message_carries_the_services_fixed_prefix() {
    let (router, _engine) = router_with_apps(&[APP]);
    let (status, body) = create_backend(&router, APP, ENVIRONMENT).await;
    assert_eq!(status, 200, "seeding CreateBackend expected 200, got {body}");

    let over_bound = "a".repeat(300);
    // (case, method, path, body)
    let rows: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "the app-identity refusal",
            "POST",
            "/backend/dnotanapp0001/details".to_string(),
            Some(json!({})),
        ),
        (
            "the missing app on the create path",
            "POST",
            "/backend".to_string(),
            Some(json!({
                "appId": "dnotanapp0001",
                "appName": "fidelity",
                "backendEnvironmentName": "dev",
            })),
        ),
        (
            "the environment-name rule",
            "POST",
            "/backend".to_string(),
            Some(json!({
                "appId": APP,
                "appName": "fidelity",
                "backendEnvironmentName": "env-1",
            })),
        ),
        (
            "the withdrawal of an operation AWS serves elsewhere",
            "POST",
            format!("/backend/{APP}/auth"),
            Some(json!({
                "backendEnvironmentName": ENVIRONMENT,
                "resourceName": "authone",
                "resourceConfig": minimal_auth_config(),
            })),
        ),
        (
            "the modelled not-found of a mutation",
            "POST",
            format!("/backend/{APP}/environments/nosuchenv/remove"),
            Some(json!({})),
        ),
        (
            "the recorded not-found of a read",
            "GET",
            format!("/backend/{APP}/job/{ENVIRONMENT}/1"),
            None,
        ),
        (
            "an identifier past this emulator's length bound",
            "POST",
            "/backend".to_string(),
            Some(json!({
                "appId": APP,
                "appName": over_bound,
                "backendEnvironmentName": "dev",
            })),
        ),
    ];

    for (case, method, path, request) in rows {
        let (status, body) = call(&router, method, &path, request).await;
        assert!(
            (400..500).contains(&status),
            "case {case:?}: expected a client-side refusal, got {status} {body}"
        );
        assert!(
            field(&body, "message").starts_with(ERROR_MESSAGE_PREFIX),
            "case {case:?}: the message carries this service's fixed prefix, got {body}"
        );
        assert!(
            field(&body, "message").len() > ERROR_MESSAGE_PREFIX.len(),
            "case {case:?}: the prefix is followed by the service's own sentence, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 6 — the job status vocabulary
// ---------------------------------------------------------------------------

/// A job reports the service's own vocabulary: the call that records it answers `PENDING`,
/// a read while it is in flight answers `PENDING` or `RUNNING`, and once the instant its
/// work was due has passed it answers `COMPLETED` and keeps answering it.
///
/// Both mutating operations that record a job are rows, so the vocabulary is the job's and
/// not one operation's.
#[tokio::test]
async fn a_job_reports_the_services_status_vocabulary() {
    let (router, _engine) = router_with_apps(&[APP]);
    let in_flight = [JOB_STATUS_PENDING, JOB_STATUS_RUNNING];
    let vocabulary = [
        JOB_STATUS_PENDING,
        JOB_STATUS_RUNNING,
        JOB_STATUS_COMPLETED,
        JOB_STATUS_FAILED,
    ];

    let (status, created) = create_backend(&router, APP, ENVIRONMENT).await;
    assert_eq!(
        status, 200,
        "case \"CreateBackend\": expected 200, got {status} {created}"
    );
    let (status, removed) = delete_backend(&router, APP, ENVIRONMENT).await;
    assert_eq!(
        status, 200,
        "case \"DeleteBackend\": expected 200, got {status} {removed}"
    );

    for (case, answer) in [("CreateBackend", &created), ("DeleteBackend", &removed)] {
        assert_eq!(
            field(answer, "status"),
            JOB_STATUS_PENDING,
            "case {case:?}: the call that records a job answers the status of a job that has not started, got {answer}"
        );
    }

    let jobs: Vec<(&str, String)> = vec![
        ("CreateBackend", field(&created, "jobId").to_string()),
        ("DeleteBackend", field(&removed, "jobId").to_string()),
    ];

    for (case, job_id) in &jobs {
        let (status, body) = get_backend_job(&router, APP, ENVIRONMENT, job_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetBackendJob expected 200, got {status} {body}"
        );
        assert!(
            in_flight.contains(&field(&body, "status")),
            "case {case:?}: a job read while it is in flight reports {in_flight:?}, got {body}"
        );
        assert!(
            body.get("error").is_none(),
            "case {case:?}: a job that has not failed carries no Error, got {body}"
        );
    }

    tokio::time::sleep(JOB_COMPLETION_DELAY + Duration::from_millis(100)).await;

    for (case, job_id) in &jobs {
        let (_, body) = get_backend_job(&router, APP, ENVIRONMENT, job_id).await;
        assert_eq!(
            field(&body, "status"),
            JOB_STATUS_COMPLETED,
            "case {case:?}: a job whose due instant has passed reports it is complete, got {body}"
        );
        let (_, again) = get_backend_job(&router, APP, ENVIRONMENT, job_id).await;
        assert_eq!(
            field(&again, "status"),
            JOB_STATUS_COMPLETED,
            "case {case:?}: a settled job keeps reporting the same status, got {again}"
        );
        assert!(
            vocabulary.contains(&field(&again, "status")),
            "case {case:?}: every reported status is in the service's vocabulary, got {again}"
        );
    }
}
