// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the APP-IDENTITY gate and this service's error envelope.
//!
//! Two rules are under test, and each row drives the real wire path
//! (`MultiServiceRouter::dispatch` with this one service registered), so the route
//! templates, the `restJson1` codec and the handlers are exercised as a served request
//! exercises them:
//!
//! 1. every operation that ADDRESSES an existing Amplify app refuses a request whose
//!    `AppId` identifies no registered app with HTTP 400 and the error code
//!    `ValidationException`, BEFORE any per-resource lookup — so a caller can tell a bad
//!    app id from a resource that was never created, and nothing is minted or stored for
//!    an app that does not exist;
//! 2. every error response this service renders carries the error `code` in the body
//!    beside `message`, declares `application/json; charset=utf-8`, and carries an `ETag`
//!    derived from the response body.
//!
//! An app EXISTS when the Amplify service's app store holds a record for it, which is
//! where an `AppId` is minted (`crate::state::app_is_registered`); these rows put one
//! there through `crate::amplify_registry` and then create this service's backend for it
//! with `CreateBackend`. Every case drives
//! `MultiServiceRouter::dispatch`, so the gate and the envelope are exercised on the same
//! wire path a served request takes; the same rule with the Amplify control plane mounted
//! beside this service, and its app minted by Amplify's own `CreateApp`, is
//! `core/router/tests/all/amplifybackend_app_registration.rs`.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifybackend_amplifybackend::state::{
    invalid_app_id_message, ComAmazonawsAmplifybackendAmplifybackendState, ERROR_MESSAGE_PREFIX,
    MAX_IDENTIFIER_CHARS,
};
use service_com_amazonaws_amplifybackend_amplifybackend::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The endpoint host an AmplifyBackend request addresses; its first label is the routing
/// key the router claims the request by.
const HOST: &str = "amplifybackend.us-east-1.amazonaws.com";

/// The media type every error response declares (recorded behaviour).
const JSON_CONTENT_TYPE: &str = "application/json; charset=utf-8";

/// The wire error code of the app-identity refusal.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The app id every "registered" row below registers through `CreateBackend`, and the
/// environment it registers.
const REGISTERED_APP: &str = "d1ab2cd3ef4gh5";
const ENVIRONMENT: &str = "staging";

/// A router with just this service mounted, and the shared engine it was mounted on, with
/// an Amplify app registered under `REGISTERED_APP` — the registry the gate reads.
fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    crate::amplify_registry::register_amplify_app(&engine, REGISTERED_APP, "gate");
    let mut router = MultiServiceRouter::new();
    // These tests exercise the service, not SigV4: requests carry no signature.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// One response, as the rules under test read it: the status, the two headers the
/// envelope rule is about, and the parsed body.
struct Answer {
    status: u16,
    content_type: String,
    etag: Option<String>,
    body: Value,
}

impl Answer {
    /// The error code the body carries (`code`), or the empty string when it carries
    /// none — which is itself what the envelope rule forbids.
    fn code(&self) -> &str {
        self.body.get("code").and_then(Value::as_str).unwrap_or("")
    }

    /// The restJson1 envelope's own discriminator, which `code` must agree with.
    fn wire_type(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("")
    }

    /// The body's `message`, or the empty string.
    fn message(&self) -> &str {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("")
    }
}

/// Send one request and read the answer back.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
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
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let etag = response
        .headers()
        .get("etag")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let payload = response.into_body();
    let body = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    Answer {
        status,
        content_type,
        etag,
        body,
    }
}

/// One operation of this service that addresses an app, as a request template.
struct Op {
    name: &'static str,
    method: &'static str,
    /// The modelled URI with `{AppId}` left to fill in — every one of these operations
    /// binds the app id as an `@httpLabel`.
    path: String,
    body: Option<Value>,
    /// The status this same call answers for a REGISTERED app that holds one backend
    /// environment (`ENVIRONMENT`) and nothing else: either the success, or the
    /// per-resource not-found refusal the request's other identifiers earn.
    registered_status: u16,
    /// The error code that refusal carries, empty when the call succeeds. Not one code
    /// for every operation: this service answers the modelled `NotFoundException` for
    /// some resources and the recorded `ResourceNotFoundException` for others.
    registered_code: &'static str,
}

/// Every operation this service implements that addresses an existing app, with the
/// request filled in for `app_id`. The URIs are the model's own
/// (`smithy_query(query="digest")` http bindings); `CreateBackend` is NOT here — it
/// answers an app that does not exist as the missing app it is (404), which is its own
/// rule — neither are the operations AWS has withdrawn from this channel
/// (`CreateBackendAuth`, `CreateBackendStorage`, `CloneBackend`), which refuse every
/// request before they look at the app at all, and neither is `RemoveAllBackends`, which
/// resolves the app against this service's backend records as well as Amplify's registry,
/// so an app this gate refuses can still be one it serves — its own suite,
/// `tests/all/remove_all_backends.rs`, is where both answers are pinned. `GetBackendJob`
/// IS here: `AppId` names an identity on a job read too, so it is resolved before the job
/// lookup — and `tests/all/backend_job_not_found.rs` pins the other half, that an address
/// under an app that DOES exist answers the job-absent 404.
fn operations(app_id: &str) -> Vec<Op> {
    vec![
        Op {
            name: "GetBackend",
            method: "POST",
            path: format!("/backend/{app_id}/details"),
            body: Some(json!({})),
            registered_status: 200,
            registered_code: "",
        },
        Op {
            name: "ListBackendJobs",
            method: "POST",
            path: format!("/backend/{app_id}/job/{ENVIRONMENT}"),
            body: Some(json!({})),
            registered_status: 200,
            registered_code: "",
        },
        Op {
            name: "GetBackendJob",
            method: "GET",
            path: format!("/backend/{app_id}/job/{ENVIRONMENT}/999"),
            body: None,
            // The app and the environment exist; no job with that id was ever minted.
            // Below the identity gate that address is absent STATE, so it answers the
            // recorded `ResourceNotFoundException` — see `src/ops/get_backend_job.rs`.
            registered_status: 404,
            registered_code: "ResourceNotFoundException",
        },
        Op {
            name: "DeleteBackend",
            method: "POST",
            path: format!("/backend/{app_id}/environments/{ENVIRONMENT}/remove"),
            body: Some(json!({})),
            registered_status: 200,
            registered_code: "",
        },
        Op {
            name: "GetBackendAuth",
            method: "POST",
            path: format!("/backend/{app_id}/auth/{ENVIRONMENT}/details"),
            body: Some(json!({ "resourceName": "authone" })),
            // The app and the environment exist; no auth resource was created. That read
            // answers the recorded `ResourceNotFoundException`, not the modelled shape —
            // see `src/ops/get_backend_auth.rs`.
            registered_status: 404,
            registered_code: "ResourceNotFoundException",
        },
        Op {
            name: "GetBackendStorage",
            method: "POST",
            path: format!("/backend/{app_id}/storage/{ENVIRONMENT}/details"),
            body: Some(json!({ "resourceName": "storageone" })),
            // The app and the environment exist; no storage resource was recorded. That
            // read answers the recorded `ResourceNotFoundException`, not the modelled
            // shape — see `src/ops/get_backend_storage.rs`.
            registered_status: 404,
            registered_code: "ResourceNotFoundException",
        },
        Op {
            name: "CreateToken",
            method: "POST",
            path: format!("/backend/{app_id}/challenge"),
            body: Some(json!({})),
            registered_status: 200,
            registered_code: "",
        },
        Op {
            name: "GetToken",
            method: "GET",
            path: format!("/backend/{app_id}/challenge/sess-1"),
            body: None,
            // The app exists; no token with that session id was minted. That read
            // answers the recorded `ResourceNotFoundException`, not the modelled shape —
            // see `src/ops/get_token.rs`.
            registered_status: 404,
            registered_code: "ResourceNotFoundException",
        },
        Op {
            name: "DeleteToken",
            method: "POST",
            path: format!("/backend/{app_id}/challenge/sess-1/remove"),
            body: Some(json!({})),
            // Session existence is not validated on this path — only app identity — so
            // deleting a session the app holds no token for SUCCEEDS as a no-op.
            registered_status: 200,
            registered_code: "",
        },
    ]
}

/// App ids no registry holds. They differ along what the rule generalises over — the
/// FORM and the SIZE of the id — and none of them is special in the handler: Amplify
/// mints an `AppId` of `length 1..20, pattern ^d[a-z0-9]+$` (the `App` shape's own
/// constraints, quoted in the Amplify service's `state.rs`), while AmplifyBackend models
/// its `AppId` as a bare `__string` `@httpLabel` with NO pattern, so every row here is a
/// well-formed request for this service that simply names no app.
fn unregistered_app_ids() -> Vec<(&'static str, String)> {
    vec![
        ("an id no app ever had", "d12440e142099".to_string()),
        (
            "one character past a registered id",
            format!("{REGISTERED_APP}9"),
        ),
        (
            "a registered id with its last character dropped",
            REGISTERED_APP[..REGISTERED_APP.len() - 1].to_string(),
        ),
        (
            "the shortest id Amplify's pattern allows",
            "d0".to_string(),
        ),
        (
            "an id at this emulator's identifier length bound",
            format!("d{}", "9".repeat(MAX_IDENTIFIER_CHARS - 1)),
        ),
        (
            "an id outside Amplify's pattern altogether",
            "not-an-app-id".to_string(),
        ),
    ]
}

/// Create `REGISTERED_APP`'s backend with one environment through `CreateBackend`. The app
/// itself is registered in the Amplify store by `router_with_engine`, which is what lets
/// this call through.
async fn register_app(router: &MultiServiceRouter) {
    let answer = call(
        router,
        "POST",
        "/backend",
        Some(json!({
            "appId": REGISTERED_APP,
            "appName": "gate",
            "backendEnvironmentName": ENVIRONMENT,
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "seeding CreateBackend expected 200, got {} {}",
        answer.status, answer.body
    );
}

/// Assert the envelope rule on one error response: the body carries a `code` that agrees
/// with the protocol's own discriminator, the media type carries its charset, and an
/// `ETag` is present.
fn assert_error_envelope(case: &str, answer: &Answer) {
    assert!(
        !answer.code().is_empty(),
        "case {case:?}: an error body carries a `code` member, got {}",
        answer.body
    );
    assert!(
        answer.wire_type().ends_with(answer.code()),
        "case {case:?}: the body's `code` agrees with the envelope's `__type`, got {:?} and {:?}",
        answer.code(),
        answer.wire_type()
    );
    assert_eq!(
        answer.content_type, JSON_CONTENT_TYPE,
        "case {case:?}: expected content-type {JSON_CONTENT_TYPE:?}, got {:?}",
        answer.content_type
    );
    assert!(
        answer.etag.as_deref().is_some_and(|tag| !tag.is_empty()),
        "case {case:?}: an error response carries an ETag, got {:?}",
        answer.etag
    );
}

/// Every operation that addresses an app refuses an unregistered `AppId` with HTTP 400
/// `ValidationException`, whatever the rest of the request names.
#[tokio::test]
async fn an_unregistered_app_id_is_refused_as_a_validation_exception() {
    let (router, _engine) = router_with_engine();
    register_app(&router).await;

    for (form, app_id) in unregistered_app_ids() {
        for op in operations(&app_id) {
            let case = format!("{} with {form}", op.name);
            let answer = call(&router, op.method, &op.path, op.body.clone()).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400, got {} {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                VALIDATION_EXCEPTION,
                "case {case:?}: expected code {VALIDATION_EXCEPTION:?}, got {}",
                answer.body
            );
            assert!(
                answer.message().starts_with(ERROR_MESSAGE_PREFIX),
                "case {case:?}: the message carries this service's fixed prefix, got {}",
                answer.body
            );
            assert_eq!(
                answer.message(),
                invalid_app_id_message(),
                "case {case:?}: the refusal states the app id is invalid, got {}",
                answer.body
            );
            assert_error_envelope(&case, &answer);
        }
    }
}

/// The refusal happens BEFORE anything is minted or stored: a `CreateToken` for an
/// unregistered app leaves no token and no app record behind, so a later `GetToken`
/// cannot report a token bound to an app that does not exist.
#[tokio::test]
async fn a_refused_request_stores_nothing() {
    let (router, engine) = router_with_engine();

    for (form, app_id) in unregistered_app_ids() {
        let answer = call(
            &router,
            "POST",
            &format!("/backend/{app_id}/challenge"),
            Some(json!({})),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {form:?}: CreateToken expected 400, got {} {}",
            answer.status, answer.body
        );
        for member in ["challengeCode", "sessionId"] {
            assert!(
                answer.body.get(member).is_none(),
                "case {form:?}: a refusal mints no {member}, got {}",
                answer.body
            );
        }

        let store = engine.state::<ComAmazonawsAmplifybackendAmplifybackendState>();
        let held = store.lock();
        assert!(
            !held.challenge_tokens.contains_key(&app_id),
            "case {form:?}: no challenge token is stored for an unregistered app"
        );
        assert!(
            !held.apps.contains_key(&app_id),
            "case {form:?}: a refused call registers no app"
        );
    }
}

/// A REGISTERED app reaches its own resources: the same calls answer the success or the
/// per-resource refusal they earn, never the app-identity refusal — so the gate narrows
/// nothing but the app id.
#[tokio::test]
async fn a_registered_app_reaches_its_own_resources() {
    for op in operations(REGISTERED_APP) {
        // One router per row: `DeleteBackend` and `CreateBackendAuth` change the app, and
        // each row states the status of a call against the app as `register_app` leaves it.
        let (router, _engine) = router_with_engine();
        register_app(&router).await;

        let answer = call(&router, op.method, &op.path, op.body.clone()).await;
        assert_eq!(
            answer.status, op.registered_status,
            "case {:?}: expected {}, got {} {}",
            op.name, op.registered_status, answer.status, answer.body
        );
        assert_ne!(
            answer.code(),
            VALIDATION_EXCEPTION,
            "case {:?}: a registered app never earns the app-identity refusal, got {}",
            op.name,
            answer.body
        );
        if answer.status == 404 {
            assert_eq!(
                answer.code(),
                op.registered_code,
                "case {:?}: a missing resource answers this operation's not-found code, got {}",
                op.name,
                answer.body
            );
            assert_error_envelope(op.name, &answer);
        }
    }
}

/// One row of the envelope table: the case, the status and error code it must answer,
/// and the request that provokes it.
struct ErrorCase {
    name: &'static str,
    status: u16,
    code: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
}

/// The envelope rule holds for every error this service renders, not just the identity
/// refusal: the rows differ in the error KIND — a modelled 404, the recorded
/// `ResourceNotFoundException` 404, a modelled 400, the identity 400, the Studio-only
/// withdrawal's 400 and the scaffold's 501 — and each carries the `code`, the
/// charset-qualified media type and an `ETag`.
#[tokio::test]
async fn every_error_response_carries_the_code_and_the_service_headers() {
    let (router, _engine) = router_with_engine();
    register_app(&router).await;

    let rows = vec![
        ErrorCase {
            name: "a registered app with no such environment",
            status: 404,
            code: "NotFoundException",
            method: "POST",
            path: format!("/backend/{REGISTERED_APP}/environments/nosuchenv/remove"),
            body: Some(json!({})),
        },
        ErrorCase {
            name: "an app id past this emulator's identifier bound",
            status: 400,
            code: "BadRequestException",
            method: "POST",
            path: format!("/backend/{}/challenge", "d".repeat(MAX_IDENTIFIER_CHARS + 1)),
            body: Some(json!({})),
        },
        ErrorCase {
            name: "an unregistered app id",
            status: 400,
            code: VALIDATION_EXCEPTION,
            method: "POST",
            path: "/backend/dnotregistered9/details".to_string(),
            body: Some(json!({})),
        },
        ErrorCase {
            name: "a storage resource that was never recorded",
            status: 404,
            code: "ResourceNotFoundException",
            method: "POST",
            path: format!("/backend/{REGISTERED_APP}/storage/{ENVIRONMENT}/details"),
            body: Some(json!({ "resourceName": "storageone" })),
        },
        ErrorCase {
            name: "an operation AWS serves only through Amplify Studio",
            status: 400,
            code: VALIDATION_EXCEPTION,
            method: "POST",
            path: format!("/backend/{REGISTERED_APP}/storage"),
            body: Some(json!({
                "backendEnvironmentName": ENVIRONMENT,
                "resourceName": "storageone",
                "resourceConfig": {
                    "bucketName": "gate-storage-bucket",
                    "permissions": { "authenticated": ["READ"] },
                    "serviceName": "S3"
                },
            })),
        },
        ErrorCase {
            name: "an operation this emulator has not implemented",
            status: 501,
            code: "NotImplemented",
            method: "POST",
            path: format!("/backend/{REGISTERED_APP}/api"),
            body: Some(json!({
                "backendEnvironmentName": ENVIRONMENT,
                "resourceName": "apione",
                "resourceConfig": { "service": "AppSync" },
            })),
        },
    ];

    let mut tags = Vec::new();
    for row in rows {
        let answer = call(&router, row.method, &row.path, row.body.clone()).await;
        assert_eq!(
            answer.status, row.status,
            "case {:?}: expected {}, got {} {}",
            row.name, row.status, answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            row.code,
            "case {:?}: expected code {:?}, got {}",
            row.name,
            row.code,
            answer.body
        );
        assert!(
            !answer.message().is_empty(),
            "case {:?}: an error body carries a message beside the code, got {}",
            row.name,
            answer.body
        );
        assert_error_envelope(row.name, &answer);
        tags.push((row.name, answer.etag.unwrap_or_default()));
    }

    // The tag is a function of the BODY: four different error bodies, four different
    // tags.
    for (left, right) in tags.iter().zip(tags.iter().skip(1)) {
        assert_ne!(
            left.1, right.1,
            "cases {:?} and {:?} answer different bodies, so their ETags differ",
            left.0, right.0
        );
    }

    // …and only of the body: the same refusal twice answers the same tag.
    let repeat = call(&router, "POST", "/backend/dnotregistered9/details", Some(json!({}))).await;
    let first = tags
        .iter()
        .find(|(case, _)| *case == "an unregistered app id")
        .map(|(_, tag)| tag.clone())
        .unwrap_or_default();
    assert_eq!(
        repeat.etag.clone().unwrap_or_default(),
        first,
        "case \"repeated refusal\": the same body answers the same ETag, got {:?} then {:?}",
        first,
        repeat.etag
    );
}
