// SPDX-License-Identifier: CC0-1.0

//! AmplifyBackend's app-identity gate through the ROUTER, with the Amplify control plane
//! mounted beside it on the one shared engine.
//!
//! An `AppId` is minted by Amplify's `CreateApp` and by nothing else, so Amplify's app
//! store is the registry AmplifyBackend settles identity against. This suite pins that
//! rule where every service is mounted together — the router is what a client actually
//! talks to — and it pins which STORE decides:
//!
//! * an id Amplify holds no app for is refused, by every operation that addresses an
//!   existing app with HTTP 400 `ValidationException`, and by `CreateBackend` as the
//!   missing app it names (404 `NotFoundException`);
//! * an app Amplify DID mint is reached: `CreateBackend` creates its backend, and before
//!   that backend exists the app's own resource reads answer the recorded
//!   `ResourceNotFoundException` rather than the identity refusal.
//!
//! The gate's own table — every app-addressing operation × every form of unregistered app
//! id, with the error envelope — is
//! `services/service-com-amazonaws-amplifybackend-amplifybackend/tests/all/app_registration_gate.rs`.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The two endpoint hosts; the first label of each is the route key its service claims.
const AMPLIFY_HOST: &str = "amplify.us-east-1.amazonaws.com";
const BACKEND_HOST: &str = "amplifybackend.us-east-1.amazonaws.com";

/// The media type AmplifyBackend declares on an error response, and the error code of the
/// app-identity refusal — the wire values, written out here rather than read from the
/// crate, so this suite pins what a client sees.
const JSON_CONTENT_TYPE: &str = "application/json; charset=utf-8";
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// Amplify and AmplifyBackend over ONE shared engine, exactly as `core/router/src/lib.rs`
/// mounts them.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These tests exercise the services, not SigV4: requests carry no signature.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_amplifybackend_amplifybackend::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_amplify_amplify::EmulatedService::new(
        engine,
    ));
    router
}

/// One response: the status, the two headers AmplifyBackend's error envelope is about,
/// and the parsed body.
struct Answer {
    status: u16,
    content_type: String,
    etag: Option<String>,
    body: Value,
}

impl Answer {
    /// The error code the body carries, or the empty string on a success.
    fn code(&self) -> &str {
        self.body.get("code").and_then(Value::as_str).unwrap_or("")
    }

    /// A string member of the body, for an assertion that names the case.
    fn field(&self, name: &str) -> &str {
        self.body.get(name).and_then(Value::as_str).unwrap_or("")
    }
}

/// Send one request to `host` and read the answer back.
async fn call(
    router: &MultiServiceRouter,
    host: &str,
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
        .uri(format!("https://{host}{path}"))
        .header("host", host)
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

/// `GetBackend`: `POST /backend/{AppId}/details`.
async fn get_backend(router: &MultiServiceRouter, app_id: &str) -> Answer {
    call(
        router,
        BACKEND_HOST,
        "POST",
        &format!("/backend/{app_id}/details"),
        Some(json!({})),
    )
    .await
}

/// Create an app through Amplify's own `CreateApp` and hand back the `appId` it minted.
async fn create_amplify_app(router: &MultiServiceRouter, name: &str) -> String {
    let created = call(
        router,
        AMPLIFY_HOST,
        "POST",
        "/apps",
        Some(json!({ "name": name })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "seeding Amplify CreateApp expected 200, got {} {}",
        created.status, created.body
    );
    let app_id = created
        .body
        .get("app")
        .and_then(|app| app.get("appId"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !app_id.is_empty(),
        "seeding Amplify CreateApp must report the id it minted, got {}",
        created.body
    );
    app_id
}

/// An app id Amplify holds no app for is refused as invalid INPUT — 400 with the code
/// `ValidationException` and this service's error envelope — however well-formed it is,
/// and whatever else the Amplify control plane mounted beside it holds.
#[tokio::test]
async fn an_unregistered_app_id_is_refused_through_the_router() {
    let router = router();

    // An app Amplify minted. None of the ids below is it, so none of them decides any
    // answer here — the row is what shows the refusals are not an artefact of an empty
    // engine.
    let minted = create_amplify_app(&router, "router-app").await;

    // (case, app id) — the rows differ in the FORM and SIZE of the id; Amplify holds no
    // app for any of them.
    let rows: Vec<(&str, String)> = vec![
        ("an id no app ever had", "d12440e142099".to_string()),
        ("the shortest id Amplify's pattern allows", "d0".to_string()),
        (
            "an id outside Amplify's pattern altogether",
            "not-an-app-id".to_string(),
        ),
        ("one character past the minted id", format!("{minted}9")),
    ];

    for (case, app_id) in rows {
        let answer = get_backend(&router, &app_id).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: GetBackend expected 400, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected code {VALIDATION_EXCEPTION:?}, got {}",
            answer.body
        );
        assert!(
            !answer.field("message").is_empty(),
            "case {case:?}: the refusal carries a message beside the code, got {}",
            answer.body
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
}

/// An app AMPLIFY minted reaches its own resources through the same router: the gate
/// narrows nothing but the app id, the reads of an app that has no backend YET answer the
/// missing resource rather than the identity refusal, and `GetBackend`'s answer is
/// computed from the record `CreateBackend` stored.
#[tokio::test]
async fn a_registered_app_reaches_its_backend_through_the_router() {
    let router = router();
    let app_id = create_amplify_app(&router, "router").await;

    // Before any backend exists for it, the app is NOT refused as a bad app id: the
    // resource the request addresses is what is missing, which is the recorded
    // `ResourceNotFoundException` for a resource read.
    let before = call(
        &router,
        BACKEND_HOST,
        "GET",
        &format!("/backend/{app_id}/job/staging/999"),
        None,
    )
    .await;
    assert_eq!(
        before.status, 404,
        "case \"app with no backend yet\": GetBackendJob expected 404, got {} {}",
        before.status, before.body
    );
    assert_eq!(
        before.code(),
        "ResourceNotFoundException",
        "case \"app with no backend yet\": expected code ResourceNotFoundException, got {}",
        before.body
    );

    let created = call(
        &router,
        BACKEND_HOST,
        "POST",
        "/backend",
        Some(json!({
            "appId": app_id,
            "appName": "router",
            "backendEnvironmentName": "staging",
        })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "seeding CreateBackend expected 200, got {} {}",
        created.status, created.body
    );

    let answer = get_backend(&router, &app_id).await;
    assert_eq!(
        answer.status, 200,
        "case \"registered app\": GetBackend expected 200, got {} {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.field("appId"),
        app_id,
        "case \"registered app\": the answer is about the app asked for, got {}",
        answer.body
    );
    assert_eq!(
        answer
            .body
            .get("backendEnvironmentList")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>()),
        Some(vec!["staging"]),
        "case \"registered app\": the environments are the ones created, got {}",
        answer.body
    );
    assert_ne!(
        answer.code(),
        VALIDATION_EXCEPTION,
        "case \"registered app\": a registered app never earns the app-identity refusal, got {}",
        answer.body
    );

    // A resource missing INSIDE the app is the not-found of that resource, not the
    // identity refusal — the distinction the gate exists to make.
    let missing = call(
        &router,
        BACKEND_HOST,
        "POST",
        &format!("/backend/{app_id}/auth/nosuchenv/details"),
        Some(json!({ "resourceName": "authone" })),
    )
    .await;
    assert_eq!(
        missing.status, 404,
        "case \"missing environment\": expected 404, got {} {}",
        missing.status, missing.body
    );
    assert_eq!(
        missing.code(),
        "ResourceNotFoundException",
        "case \"missing environment\": expected code ResourceNotFoundException, got {}",
        missing.body
    );
    assert_eq!(
        missing.content_type, JSON_CONTENT_TYPE,
        "case \"missing environment\": expected content-type {JSON_CONTENT_TYPE:?}, got {:?}",
        missing.content_type
    );

    // And an id Amplify minted no app for is the missing APP on the create path: a
    // backend is never created for an app that does not exist.
    let unknown = call(
        &router,
        BACKEND_HOST,
        "POST",
        "/backend",
        Some(json!({
            "appId": format!("{app_id}9"),
            "appName": "router",
            "backendEnvironmentName": "staging",
        })),
    )
    .await;
    assert_eq!(
        unknown.status, 404,
        "case \"CreateBackend for an app Amplify never minted\": expected 404, got {} {}",
        unknown.status, unknown.body
    );
    assert_eq!(
        unknown.code(),
        "NotFoundException",
        "case \"CreateBackend for an app Amplify never minted\": expected code NotFoundException, got {}",
        unknown.body
    );
}
