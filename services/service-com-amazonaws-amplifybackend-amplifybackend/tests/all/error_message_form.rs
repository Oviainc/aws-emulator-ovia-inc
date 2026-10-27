// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the FORM of every message this service renders.
//!
//! The rule under test: an AmplifyBackend error message is the service's fixed prefix
//! followed by one sentence of the service's own — whatever the error SHAPE (the modelled
//! `BadRequestException`, `NotFoundException`, `TooManyRequestsException` and
//! `GatewayTimeoutException`, and the recorded `ValidationException` /
//! `ResourceNotFoundException` codes this service's model does not declare), whatever the
//! STATUS class (400, 404, 429, 504), whichever OPERATION renders it, and whether the
//! refusal was raised inside a handler or by the request walk before one ran. Where the
//! sentence names a resource it names it from the request, and the prefix appears exactly
//! once.
//!
//! `control_plane_fidelity.rs` pins the recorded WORDING of the refusals the recordings
//! fix; this suite pins the form of the channel they all leave through, over the error
//! shapes and status classes those cases do not reach.
//!
//! Each wire case drives the real path — `MultiServiceRouter::dispatch` with this one
//! service registered — so the route templates, the `restJson1` codec, the modelled
//! constraints, this service's error envelope and the handlers are exercised as a served
//! request exercises them.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifybackend_amplifybackend::ops::types::{
    GatewayTimeoutException, NotFoundException, TooManyRequestsException,
};
use service_com_amazonaws_amplifybackend_amplifybackend::state::{
    bad_request, gateway_timeout, limit_exceeded, not_found, store_unavailable,
    ERROR_MESSAGE_PREFIX, LIMIT_TYPE_ENVIRONMENTS, MAX_ENVIRONMENTS_PER_APP, MAX_IDENTIFIER_CHARS,
    RESOURCE_TYPE_BACKEND, STORE_UNAVAILABLE_SENTENCE,
};
use service_com_amazonaws_amplifybackend_amplifybackend::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The endpoint host an AmplifyBackend request addresses; its first label is the routing
/// key the router claims the request by.
const HOST: &str = "amplifybackend.us-east-1.amazonaws.com";

/// The app every case addresses, and the environment most of them start from.
const APP: &str = "d1messages001";
const ENVIRONMENT: &str = "staging";

/// A router with just this service mounted on a fresh shared engine, with an Amplify app
/// registered for every id in `app_ids` — the registry this service settles an app's
/// identity against (`crate::amplify_registry`).
fn router_with_apps<S: AsRef<str>>(app_ids: &[S]) -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    for app_id in app_ids {
        crate::amplify_registry::register_amplify_app(&engine, app_id.as_ref(), "messages");
    }
    let mut router = MultiServiceRouter::new();
    // These tests exercise the service, not SigV4: requests carry no signature.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
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
            "appName": "messages",
            "backendEnvironmentName": environment,
        })),
    )
    .await
}

/// One wire case: the request, and what this service's message channel must answer it with.
struct Case {
    /// What the row exercises, named in every assertion.
    label: &'static str,
    method: &'static str,
    path: String,
    request: Option<Value>,
    /// The status class the refusal belongs to, and the wire error code it carries.
    status: u16,
    code: &'static str,
    /// Text the SENTENCE must carry, taken from the request: a member name, an environment
    /// name, a bound. Empty where the service's sentence names nothing from the request.
    names: Vec<String>,
}

/// The wire error code one answer states, wherever it states it: the `code` member this
/// service's error envelope writes beside `message`, or — for a refusal the request walk
/// raised before any handler ran, which the framework renders — the `__type` the restJson1
/// envelope carries.
fn reported_code(body: &Value) -> &str {
    match body.get("code").and_then(Value::as_str) {
        Some(code) => code,
        None => field(body, "__type")
            .split('#')
            .next_back()
            .unwrap_or_default(),
    }
}

/// Assert the message of one answered case: the fixed prefix exactly once, then a
/// non-empty sentence that carries whatever the row says it names.
fn assert_message_form(case: &str, body: &Value) {
    let message = field(body, "message");
    assert!(
        message.starts_with(ERROR_MESSAGE_PREFIX),
        "case {case:?}: the message carries this service's fixed prefix, got {message:?}"
    );
    let sentence = &message[ERROR_MESSAGE_PREFIX.len()..];
    assert!(
        !sentence.trim().is_empty(),
        "case {case:?}: the prefix is followed by the service's own sentence, got {message:?}"
    );
    assert!(
        !sentence.contains(ERROR_MESSAGE_PREFIX),
        "case {case:?}: the prefix is rendered exactly once, got {message:?}"
    );
}

/// Every error this service renders over the wire is the fixed prefix plus one sentence,
/// whatever the error shape, the status class and the operation.
///
/// The rows differ along what the rule generalises over: the error SHAPE (every modelled
/// one this slice can reach, plus the two recorded codes the model does not declare), the
/// STATUS class (400, 404, 429), the OPERATION rendering it, and whether the refusal was
/// raised by a handler or by the request walk before one ran.
#[tokio::test]
async fn every_error_this_service_renders_is_the_prefix_and_one_sentence() {
    let router = router_with_apps(&[APP]);
    let (status, body) = create_backend(&router, APP, ENVIRONMENT).await;
    assert_eq!(status, 200, "seeding CreateBackend expected 200, got {body}");

    let over_bound = "a".repeat(MAX_IDENTIFIER_CHARS + 44);
    let cases: Vec<Case> = vec![
        Case {
            label: "a required member the request omits",
            method: "POST",
            path: "/backend".to_string(),
            request: Some(json!({})),
            status: 400,
            code: "BadRequestException",
            names: Vec::new(),
        },
        Case {
            label: "an identifier past this emulator's length bound",
            method: "POST",
            path: "/backend".to_string(),
            request: Some(json!({
                "appId": APP,
                "appName": over_bound,
                "backendEnvironmentName": "dev",
            })),
            status: 400,
            code: "BadRequestException",
            names: vec!["appName".to_string(), MAX_IDENTIFIER_CHARS.to_string()],
        },
        Case {
            label: "a pagination token this service did not issue",
            method: "POST",
            path: format!("/backend/{APP}/job/{ENVIRONMENT}"),
            request: Some(json!({ "nextToken": "not-a-token" })),
            status: 400,
            code: "BadRequestException",
            names: vec!["NextToken".to_string(), ENVIRONMENT.to_string()],
        },
        Case {
            label: "a pagination token whose scope is another environment",
            method: "POST",
            path: format!("/backend/{APP}/job/other"),
            request: Some(json!({ "nextToken": hex::encode("v1:1:other:x") })),
            status: 400,
            code: "BadRequestException",
            names: vec!["NextToken".to_string(), "other".to_string()],
        },
        Case {
            label: "an app id Amplify holds no app for",
            method: "POST",
            path: "/backend/dnotanapp0001/details".to_string(),
            request: Some(json!({})),
            status: 400,
            code: "ValidationException",
            names: Vec::new(),
        },
        Case {
            label: "an environment name outside the form the service creates",
            method: "POST",
            path: "/backend".to_string(),
            request: Some(json!({
                "appId": APP,
                "appName": "messages",
                "backendEnvironmentName": "Env-1",
            })),
            status: 400,
            code: "ValidationException",
            names: Vec::new(),
        },
        Case {
            label: "an operation AWS serves only through Amplify Studio",
            method: "POST",
            path: format!("/backend/{APP}/auth"),
            request: Some(json!({
                "backendEnvironmentName": ENVIRONMENT,
                "resourceName": "authone",
                "resourceConfig": {
                    "authResources": "USER_POOL_ONLY",
                    "service": "COGNITO",
                    "userPoolConfigs": {
                        "requiredSignUpAttributes": ["EMAIL"],
                        "signInMethod": "EMAIL",
                        "userPoolName": "messagesPool"
                    }
                },
            })),
            status: 400,
            code: "ValidationException",
            names: Vec::new(),
        },
        Case {
            label: "an app id Amplify holds no app for, on the create path",
            method: "POST",
            path: "/backend".to_string(),
            request: Some(json!({
                "appId": "dnotanapp0002",
                "appName": "messages",
                "backendEnvironmentName": "dev",
            })),
            status: 404,
            code: "NotFoundException",
            names: vec!["dnotanapp0002".to_string()],
        },
        Case {
            label: "an environment the app does not have",
            method: "POST",
            path: format!("/backend/{APP}/environments/nosuchenv/remove"),
            request: Some(json!({})),
            status: 404,
            code: "NotFoundException",
            names: vec!["nosuchenv".to_string()],
        },
        Case {
            label: "a job address that resolves to no job",
            method: "GET",
            path: format!("/backend/{APP}/job/{ENVIRONMENT}/1"),
            request: None,
            status: 404,
            code: "ResourceNotFoundException",
            names: Vec::new(),
        },
        Case {
            label: "an auth resource that was never created",
            method: "POST",
            path: format!("/backend/{APP}/auth/{ENVIRONMENT}/details"),
            request: Some(json!({ "resourceName": "authone" })),
            status: 404,
            code: "ResourceNotFoundException",
            names: Vec::new(),
        },
    ];

    for case in &cases {
        let (status, body) = call(
            &router,
            case.method,
            &case.path,
            case.request.clone(),
        )
        .await;
        assert_eq!(
            status, case.status,
            "case {:?}: expected {}, got {status} {body}",
            case.label, case.status
        );
        assert_eq!(
            reported_code(&body),
            case.code,
            "case {:?}: expected code {:?}, got {body}",
            case.label,
            case.code
        );
        assert_message_form(case.label, &body);
        for named in &case.names {
            assert!(
                field(&body, "message").contains(named.as_str()),
                "case {:?}: the sentence names {named:?} from the request, got {body}",
                case.label
            );
        }
    }
}

/// The bound-exceeded refusal is worded the same way: this emulator's own sentence, behind
/// the service's prefix, naming the limit it holds.
///
/// A separate case because reaching it means filling a store: the app is given every
/// backend environment it may hold, and the next create is the row under test.
#[tokio::test]
async fn a_bound_this_emulator_holds_is_refused_in_the_same_form() {
    let router = router_with_apps(&[APP]);

    // `MAX_ENVIRONMENTS_PER_APP` distinct names inside the form the service creates (2 to
    // 10 lowercase letters): "aa", "ab", … — enough for any bound this emulator holds.
    let alphabet: Vec<char> = ('a'..='z').collect();
    let names: Vec<String> = alphabet
        .iter()
        .flat_map(|first| alphabet.iter().map(move |second| format!("{first}{second}")))
        .take(MAX_ENVIRONMENTS_PER_APP)
        .collect();
    assert_eq!(
        names.len(),
        MAX_ENVIRONMENTS_PER_APP,
        "the fixture needs one name per environment the app may hold, got {}",
        names.len()
    );
    for name in &names {
        let (status, body) = create_backend(&router, APP, name).await;
        assert_eq!(
            status, 200,
            "seeding the environment {name:?} expected 200, got {status} {body}"
        );
    }

    let (status, body) = create_backend(&router, APP, "overbound").await;
    assert_eq!(
        status, 429,
        "case \"one environment past the bound\": expected 429, got {status} {body}"
    );
    assert_eq!(
        field(&body, "code"),
        "TooManyRequestsException",
        "case \"one environment past the bound\": expected the limit code, got {body}"
    );
    assert_eq!(
        field(&body, "limitType"),
        LIMIT_TYPE_ENVIRONMENTS,
        "case \"one environment past the bound\": the refusal names which limit, got {body}"
    );
    assert_message_form("one environment past the bound", &body);
    for named in [APP, &MAX_ENVIRONMENTS_PER_APP.to_string()] {
        assert!(
            field(&body, "message").contains(named),
            "case \"one environment past the bound\": the sentence names {named:?}, got {body}"
        );
    }
}

/// An engine that is not the one shared store every handler reads: mounting the service on
/// it is how a request reaches the service's own server-side failure.
///
/// `crate::state::service_state` resolves this service's store out of the SHARED in-memory
/// engine and answers `None` for anything else, which every handler turns into the modelled
/// `GatewayTimeoutException` before it looks at the request.
#[derive(Debug)]
struct NotTheSharedEngine;

/// A failure on this service's OWN side is worded like every failure it renders: the fixed
/// prefix and one sentence, the same one whichever operation was called, naming nothing
/// from the request.
///
/// The rows span the operations and the methods — a create, two reads, a listing, a
/// removal and a token mint — so the wording belongs to the channel and not to one handler.
#[tokio::test]
async fn a_failure_on_the_services_own_side_is_worded_the_same_way() {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::new(NotTheSharedEngine)));

    let expected = format!("{ERROR_MESSAGE_PREFIX}{STORE_UNAVAILABLE_SENTENCE}");
    // (case, method, path, body)
    let rows: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "CreateBackend",
            "POST",
            "/backend".to_string(),
            Some(json!({
                "appId": APP,
                "appName": "messages",
                "backendEnvironmentName": ENVIRONMENT,
            })),
        ),
        (
            "GetBackend",
            "POST",
            format!("/backend/{APP}/details"),
            Some(json!({})),
        ),
        (
            "GetBackendJob",
            "GET",
            format!("/backend/{APP}/job/{ENVIRONMENT}/1"),
            None,
        ),
        (
            "ListBackendJobs",
            "POST",
            format!("/backend/{APP}/job/{ENVIRONMENT}"),
            Some(json!({})),
        ),
        (
            "DeleteBackend",
            "POST",
            format!("/backend/{APP}/environments/{ENVIRONMENT}/remove"),
            Some(json!({})),
        ),
        (
            "CreateToken",
            "POST",
            format!("/backend/{APP}/challenge"),
            Some(json!({})),
        ),
    ];

    for (case, method, path, request) in rows {
        let (status, body) = call(&router, method, &path, request).await;
        assert_eq!(
            status, 504,
            "case {case:?}: a store this service cannot reach is its own failure, got {status} {body}"
        );
        assert_eq!(
            field(&body, "code"),
            "GatewayTimeoutException",
            "case {case:?}: expected the service's server-side code, got {body}"
        );
        assert_eq!(
            field(&body, "message"),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
        assert!(
            !field(&body, "message").contains(APP),
            "case {case:?}: the sentence names nothing from the request, got {body}"
        );
        assert_message_form(case, &body);
    }
}

/// Every builder a handler renders a message through puts the prefix in front of the
/// sentence it was given, and nothing else: the sentence passes through unchanged, so the
/// prefix cannot be forgotten at a call site and cannot be doubled by one.
///
/// The rows differ in the error SHAPE being built and in the sentence given — one naming a
/// resource, one naming nothing, one carrying punctuation, and the empty one — so the rule
/// is the builders' and not one sentence's.
#[test]
fn every_builder_renders_the_prefix_in_front_of_the_sentence_it_was_given() {
    let sentences = [
        "BackendEnvironment staging does not exist.",
        "Unable to find matching job with provided parameters.",
        "appName may be at most 256 characters, got 300.",
        "",
    ];

    for sentence in sentences {
        let expected = format!("{ERROR_MESSAGE_PREFIX}{sentence}");
        let rendered: Vec<(&str, Option<String>)> = vec![
            ("BadRequestException", bad_request(sentence).message),
            ("GatewayTimeoutException", gateway_timeout(sentence).message),
            (
                "NotFoundException",
                not_found(RESOURCE_TYPE_BACKEND, sentence).message,
            ),
            (
                "TooManyRequestsException",
                limit_exceeded(LIMIT_TYPE_ENVIRONMENTS, sentence).message,
            ),
        ];
        for (shape, message) in rendered {
            assert_eq!(
                message.as_deref(),
                Some(expected.as_str()),
                "case {shape:?} with sentence {sentence:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    // The shapes carry the member that says WHICH resource or limit beside the message, so
    // a caller reading the payload is not left with the sentence alone.
    let missing: NotFoundException = not_found(RESOURCE_TYPE_BACKEND, "Backend does not exist.");
    assert_eq!(
        missing.resource_type.as_deref(),
        Some(RESOURCE_TYPE_BACKEND),
        "case \"NotFoundException\": the payload names the resource type, got {missing:?}"
    );
    let over: TooManyRequestsException = limit_exceeded(LIMIT_TYPE_ENVIRONMENTS, "Too many.");
    assert_eq!(
        over.limit_type.as_deref(),
        Some(LIMIT_TYPE_ENVIRONMENTS),
        "case \"TooManyRequestsException\": the payload names the limit, got {over:?}"
    );

    // The unreachable-store failure is one of those builders' sentences, not a message of
    // its own.
    let unavailable: GatewayTimeoutException = store_unavailable();
    assert_eq!(
        unavailable.message.as_deref(),
        Some(format!("{ERROR_MESSAGE_PREFIX}{STORE_UNAVAILABLE_SENTENCE}").as_str()),
        "case \"the unreachable store\": expected the prefixed sentence, got {unavailable:?}"
    );
}
