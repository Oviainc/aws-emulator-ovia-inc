// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AmplifyBackend STORAGE resource: the SDK/CLI withdrawal of
//! `CreateBackendStorage`, and the `GetBackendStorage` read that reports what the store
//! holds.
//!
//! Two rules are under test, and each row drives the real wire path
//! (`MultiServiceRouter::dispatch` with this one service registered), so the route
//! templates, the `restJson1` codec, the modelled constraints and the handlers are
//! exercised exactly as a served request exercises them:
//!
//! 1. `CreateBackendStorage` is WITHDRAWN from SDK and AWS CLI callers: every call is
//!    refused with HTTP 400, the error code `ValidationException` and the service's fixed
//!    Studio-only message, whatever the request names, and the call stores NOTHING — no
//!    storage resource and no job record;
//! 2. `GetBackendStorage` reports the storage resource the store holds under
//!    (AppId, BackendEnvironmentName, ResourceName) — `BucketName`, `ServiceName` and
//!    `Permissions` as recorded, with `Imported` false for one that was not adopted from
//!    an existing bucket — and answers a lookup that does not resolve with HTTP 404, the
//!    error code `ResourceNotFoundException` and one fixed message, whatever was asked
//!    for.
//!
//! Because rule 1 means no SDK/CLI call can bring a storage resource into being, the
//! rows for rule 2's success path SEED the store directly through the shared engine —
//! the same store a handler reaches — and then read it back over the wire.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifybackend_amplifybackend::ops::types::{
    BackendStoragePermissions, CreateBackendStorageResourceConfig,
};
use service_com_amazonaws_amplifybackend_amplifybackend::state::{
    invalid_app_id_message, studio_only_message, ComAmazonawsAmplifybackendAmplifybackendState,
    MAX_IDENTIFIER_CHARS,
};
use service_com_amazonaws_amplifybackend_amplifybackend::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The endpoint host an AmplifyBackend request addresses; its first label is the routing
/// key the router claims the request by.
const HOST: &str = "amplifybackend.us-east-1.amazonaws.com";

/// The message `GetBackendStorage` answers a lookup that does not resolve with: this
/// service's fixed prefix plus the fixed sentence the recording shows, written out here
/// rather than imported so the test pins the text independently of the handler.
const STORAGE_NOT_FOUND_MESSAGE: &str = "An error occurred while processing your request: \
     Given Storage resource not found in amplify-meta.json";

/// The app the rows register, and the environment they register it with.
const APP: &str = "d1ab2cd3ef4gh5";
const ENVIRONMENT: &str = "staging";

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

/// Register `APP` with one backend environment through `CreateBackend` — the only app
/// registry reachable with this service mounted alone.
async fn register_app(router: &MultiServiceRouter) {
    let (status, body) = call(
        router,
        "POST",
        "/backend",
        Some(json!({
            "appId": APP,
            "appName": "storage tests",
            "backendEnvironmentName": ENVIRONMENT,
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "seeding CreateBackend expected 200, got {status} {body}"
    );
}

/// Call `CreateBackendStorage`.
async fn create_storage(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    resource_name: &str,
    resource_config: Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/backend/{app_id}/storage"),
        Some(json!({
            "backendEnvironmentName": environment,
            "resourceName": resource_name,
            "resourceConfig": resource_config,
        })),
    )
    .await
}

/// Call `GetBackendStorage`.
async fn get_storage(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    resource_name: &str,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/backend/{app_id}/storage/{environment}/details"),
        Some(json!({ "resourceName": resource_name })),
    )
    .await
}

/// How many storage resources and job records the store holds for `APP`, across every
/// environment — the two things rule 1 says a refused create must not grow.
fn stored_counts(engine: &engine::InMemoryEngine) -> (usize, usize) {
    let store = engine.state::<ComAmazonawsAmplifybackendAmplifybackendState>();
    let held = store.lock();
    match held.app(APP) {
        Some(app) => (
            app.environments
                .values()
                .map(|environment| environment.storage.len())
                .sum(),
            app.jobs.len(),
        ),
        None => (0, 0),
    }
}

/// One `CreateBackendStorage` request, as a row of the withdrawal table.
struct Attempt {
    name: &'static str,
    app_id: String,
    environment: String,
    resource_name: String,
    resource_config: Value,
}

/// The create attempts. They differ along everything the request can vary — whether the
/// app is registered, whether the environment exists, the SIZE and form of the resource
/// name (including one AT this emulator's identifier bound and one past it), whether the
/// optional `BucketName` is sent, and the SIZE and CONTENT of the two permission lists —
/// because the rule under test is that NONE of it changes the answer.
///
/// Every enum value comes from the model's own value lists: `ServiceName`
/// (`SERVICE_NAME_VALUES`), `AuthenticatedElement` (`AUTHENTICATED_ELEMENT_VALUES`) and
/// `UnAuthenticatedElement` (`UN_AUTHENTICATED_ELEMENT_VALUES`) in
/// `src/ops/types/shared_types.rs`; the rows between them cover each of `READ`,
/// `CREATE_AND_UPDATE` and `DELETE`.
fn attempts() -> Vec<Attempt> {
    vec![
        Attempt {
            name: "the registered app and its environment, read-only access",
            app_id: APP.to_string(),
            environment: ENVIRONMENT.to_string(),
            resource_name: "s3storage".to_string(),
            resource_config: json!({
                "bucketName": "demo-app-storage-bucket",
                "permissions": { "authenticated": ["READ"] },
                "serviceName": "S3"
            }),
        },
        Attempt {
            name: "every authenticated permission plus guest read, no bucket name",
            app_id: APP.to_string(),
            environment: ENVIRONMENT.to_string(),
            resource_name: "storefrontFiles".to_string(),
            resource_config: json!({
                "permissions": {
                    "authenticated": ["READ", "CREATE_AND_UPDATE", "DELETE"],
                    "unAuthenticated": ["READ"]
                },
                "serviceName": "S3"
            }),
        },
        Attempt {
            name: "empty permission lists",
            app_id: APP.to_string(),
            environment: ENVIRONMENT.to_string(),
            resource_name: "lockedDown".to_string(),
            resource_config: json!({
                "bucketName": "locked-down-bucket",
                "permissions": { "authenticated": [], "unAuthenticated": [] },
                "serviceName": "S3"
            }),
        },
        Attempt {
            name: "an environment that was never created",
            app_id: APP.to_string(),
            environment: "nosuchenv".to_string(),
            resource_name: "s3storage".to_string(),
            resource_config: json!({
                "bucketName": "orphan-bucket",
                "permissions": {
                    "authenticated": ["CREATE_AND_UPDATE", "DELETE"],
                    "unAuthenticated": ["READ", "CREATE_AND_UPDATE", "DELETE"]
                },
                "serviceName": "S3"
            }),
        },
        Attempt {
            name: "an app id no app is registered for",
            app_id: "dneverregistered".to_string(),
            environment: ENVIRONMENT.to_string(),
            resource_name: "s3storage".to_string(),
            resource_config: json!({
                "bucketName": "unknown-app-bucket",
                "permissions": { "authenticated": ["READ"] },
                "serviceName": "S3"
            }),
        },
        Attempt {
            name: "a resource name at this emulator's identifier bound",
            app_id: APP.to_string(),
            environment: ENVIRONMENT.to_string(),
            resource_name: "s".repeat(MAX_IDENTIFIER_CHARS),
            resource_config: json!({
                "permissions": { "authenticated": ["DELETE"] },
                "serviceName": "S3"
            }),
        },
        Attempt {
            name: "a resource name one character past that bound",
            app_id: APP.to_string(),
            environment: ENVIRONMENT.to_string(),
            resource_name: "s".repeat(MAX_IDENTIFIER_CHARS + 1),
            resource_config: json!({
                "bucketName": "b".repeat(MAX_IDENTIFIER_CHARS + 1),
                "permissions": { "authenticated": ["READ"] },
                "serviceName": "S3"
            }),
        },
    ]
}

/// Rule 1: every `CreateBackendStorage` call is refused with the same 400
/// `ValidationException` and the same fixed message, whatever it names — and nothing is
/// minted or stored for it.
#[tokio::test]
async fn create_backend_storage_is_refused_on_every_call() {
    let (router, engine) = router_with_engine();
    register_app(&router).await;
    let (storage_before, jobs_before) = stored_counts(&engine);

    for attempt in attempts() {
        let label = attempt.name;
        let (status, body) = create_storage(
            &router,
            &attempt.app_id,
            &attempt.environment,
            &attempt.resource_name,
            attempt.resource_config.clone(),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: CreateBackendStorage is withdrawn, expected 400, got {status} {body}"
        );
        assert_eq!(
            field(&body, "code"),
            "ValidationException",
            "case {label:?}: expected code ValidationException, got {body}"
        );
        assert_eq!(
            field(&body, "message"),
            studio_only_message(),
            "case {label:?}: the refusal carries the service's fixed Studio-only message, got {body}"
        );
        // A refusal is not a job: nothing a caller could poll comes back.
        for member in ["jobId", "status", "appId", "backendEnvironmentName"] {
            assert!(
                body.get(member).is_none(),
                "case {label:?}: a refused create reports no {member}, got {body}"
            );
        }
        // …and the app-identity refusal is NOT what this answers: the withdrawal
        // precedes it, so a registered and an unregistered app id earn the same message.
        assert_ne!(
            field(&body, "message"),
            invalid_app_id_message(),
            "case {label:?}: the withdrawal precedes the app-identity gate, got {body}"
        );

        let (storage_after, jobs_after) = stored_counts(&engine);
        assert_eq!(
            (storage_after, jobs_after),
            (storage_before, jobs_before),
            "case {label:?}: a refused create stores neither a storage resource nor a job"
        );

        // The read agrees: the resource the refused call named does not exist.
        let (status, read) = get_storage(
            &router,
            &attempt.app_id,
            &attempt.environment,
            &attempt.resource_name,
        )
        .await;
        let expected_status = if attempt.app_id == APP { 404 } else { 400 };
        assert_eq!(
            status, expected_status,
            "case {label:?}: after the refusal the resource is not readable, got {status} {read}"
        );
    }
}

/// One `GetBackendStorage` success row: the resource as the store holds it, and the
/// document the read must report for it.
struct Held {
    name: &'static str,
    resource_name: &'static str,
    stored: fn() -> CreateBackendStorageResourceConfig,
    /// The `ResourceConfig` document the read answers with, written out independently of
    /// the handler's own mapping.
    reported: fn() -> Value,
}

/// A `BackendStoragePermissions` from the two permission lists.
fn permissions(authenticated: &[&str], un_authenticated: Option<&[&str]>) -> BackendStoragePermissions {
    BackendStoragePermissions {
        authenticated: authenticated.iter().map(|value| (*value).to_string()).collect(),
        un_authenticated: un_authenticated
            .map(|values| values.iter().map(|value| (*value).to_string()).collect()),
    }
}

/// The resources rule 2's success path is read over. They differ along what the read
/// generalises over: whether `BucketName` was recorded at all, the SIZE and CONTENT of
/// the two permission lists, and whether the optional `UnAuthenticated` list is present.
fn held_resources() -> Vec<Held> {
    vec![
        Held {
            name: "bucket named, authenticated read only",
            resource_name: "s3storage",
            stored: || CreateBackendStorageResourceConfig {
                bucket_name: Some("demo-app-storage-bucket".to_string()),
                permissions: permissions(&["READ"], None),
                service_name: "S3".to_string(),
            },
            reported: || {
                json!({
                    "bucketName": "demo-app-storage-bucket",
                    "imported": false,
                    "permissions": { "authenticated": ["READ"] },
                    "serviceName": "S3"
                })
            },
        },
        Held {
            name: "no bucket name recorded, every authenticated permission",
            resource_name: "storefrontFiles",
            stored: || CreateBackendStorageResourceConfig {
                bucket_name: None,
                permissions: permissions(&["READ", "CREATE_AND_UPDATE", "DELETE"], Some(&["READ"])),
                service_name: "S3".to_string(),
            },
            reported: || {
                json!({
                    "imported": false,
                    "permissions": {
                        "authenticated": ["READ", "CREATE_AND_UPDATE", "DELETE"],
                        "unAuthenticated": ["READ"]
                    },
                    "serviceName": "S3"
                })
            },
        },
        Held {
            name: "empty permission lists",
            resource_name: "lockedDown",
            stored: || CreateBackendStorageResourceConfig {
                bucket_name: Some("locked-down-bucket".to_string()),
                permissions: permissions(&[], Some(&[])),
                service_name: "S3".to_string(),
            },
            reported: || {
                json!({
                    "bucketName": "locked-down-bucket",
                    "imported": false,
                    "permissions": { "authenticated": [], "unAuthenticated": [] },
                    "serviceName": "S3"
                })
            },
        },
        Held {
            name: "write and delete for guests too",
            resource_name: "sharedDrop",
            stored: || CreateBackendStorageResourceConfig {
                bucket_name: Some("shared-drop-bucket".to_string()),
                permissions: permissions(
                    &["CREATE_AND_UPDATE", "DELETE"],
                    Some(&["READ", "CREATE_AND_UPDATE", "DELETE"]),
                ),
                service_name: "S3".to_string(),
            },
            reported: || {
                json!({
                    "bucketName": "shared-drop-bucket",
                    "imported": false,
                    "permissions": {
                        "authenticated": ["CREATE_AND_UPDATE", "DELETE"],
                        "unAuthenticated": ["READ", "CREATE_AND_UPDATE", "DELETE"]
                    },
                    "serviceName": "S3"
                })
            },
        },
    ]
}

/// Rule 2's success path: a storage resource the store holds is reported back as
/// recorded, with `Imported` false, and reading it changes nothing a later caller sees.
#[tokio::test]
async fn get_backend_storage_reports_the_resource_the_store_holds() {
    let (router, engine) = router_with_engine();
    register_app(&router).await;

    // Seed the store the way a handler reaches it: the shared engine's own state.
    {
        let store = engine.state::<ComAmazonawsAmplifybackendAmplifybackendState>();
        let mut held = store.lock();
        let app = held.app_mut(APP).expect("CreateBackend registered the app");
        let environment = app
            .environment_mut(ENVIRONMENT)
            .expect("CreateBackend created the environment");
        for resource in held_resources() {
            environment
                .storage
                .insert(resource.resource_name.to_string(), (resource.stored)());
        }
    }

    for resource in held_resources() {
        let label = resource.name;
        let (status, body) = get_storage(&router, APP, ENVIRONMENT, resource.resource_name).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetBackendStorage expected 200, got {status} {body}"
        );
        assert_eq!(
            field(&body, "appId"),
            APP,
            "case {label:?}: the read reports the app asked for, got {body}"
        );
        assert_eq!(
            field(&body, "backendEnvironmentName"),
            ENVIRONMENT,
            "case {label:?}: the read reports the environment asked for, got {body}"
        );
        assert_eq!(
            field(&body, "resourceName"),
            resource.resource_name,
            "case {label:?}: the read reports the resource asked for, got {body}"
        );
        let returned = body.get("resourceConfig").cloned().unwrap_or(Value::Null);
        assert_eq!(
            returned,
            (resource.reported)(),
            "case {label:?}: the read reports the stored configuration, got {returned}"
        );

        // A read is a read: twice answers the same, and the store still holds exactly
        // what was seeded.
        let (_, again) = get_storage(&router, APP, ENVIRONMENT, resource.resource_name).await;
        assert_eq!(
            again, body,
            "case {label:?}: two reads answer identically, got {again}"
        );
    }

    let (storage_count, _) = stored_counts(&engine);
    assert_eq!(
        storage_count,
        held_resources().len(),
        "reading storage resources neither adds nor removes one"
    );
}

/// Rule 2's refusals: a lookup that does not resolve answers HTTP 404
/// `ResourceNotFoundException` with ONE fixed message, whatever it was asked about,
/// while an `AppId` no app is registered for is still the app-identity refusal.
#[tokio::test]
async fn get_backend_storage_refuses_a_lookup_that_does_not_resolve() {
    let (router, engine) = router_with_engine();
    register_app(&router).await;
    {
        let store = engine.state::<ComAmazonawsAmplifybackendAmplifybackendState>();
        let mut held = store.lock();
        let app = held.app_mut(APP).expect("CreateBackend registered the app");
        let environment = app
            .environment_mut(ENVIRONMENT)
            .expect("CreateBackend created the environment");
        environment.storage.insert(
            "s3storage".to_string(),
            CreateBackendStorageResourceConfig {
                bucket_name: Some("demo-app-storage-bucket".to_string()),
                permissions: permissions(&["READ"], None),
                service_name: "S3".to_string(),
            },
        );
    }

    // The rows differ in WHICH part of the (app, environment, resource) triple fails to
    // resolve, and in the form and size of the identifier that fails — the rule is that
    // the answer does not.
    let rows: Vec<(&str, &str, String)> = vec![
        (
            "a resource name no storage resource was recorded under",
            ENVIRONMENT,
            "neverCreated".to_string(),
        ),
        (
            "a resource name that differs from a held one by one character",
            ENVIRONMENT,
            "s3storag".to_string(),
        ),
        (
            "a resource name at this emulator's identifier bound",
            ENVIRONMENT,
            "s".repeat(MAX_IDENTIFIER_CHARS),
        ),
        (
            "an environment the app never created",
            "nosuchenv",
            "s3storage".to_string(),
        ),
        (
            "an environment name of one character",
            "x",
            "neverCreated".to_string(),
        ),
    ];

    for (label, environment, resource_name) in rows {
        let (status, body) = get_storage(&router, APP, environment, &resource_name).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            field(&body, "code"),
            "ResourceNotFoundException",
            "case {label:?}: expected code ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            field(&body, "message"),
            STORAGE_NOT_FOUND_MESSAGE,
            "case {label:?}: the refusal carries the service's fixed sentence, got {body}"
        );
        // The refusal names nothing from the request, so it cannot leak which part of the
        // lookup failed.
        assert!(
            !field(&body, "message").contains(&resource_name)
                && !field(&body, "message").contains(environment),
            "case {label:?}: the fixed refusal echoes no identifier, got {body}"
        );
    }

    // A well-formed app id no app is registered for is the app-identity refusal instead:
    // 400, and a different code and message.
    let (status, body) = get_storage(&router, "dneverregistered", ENVIRONMENT, "s3storage").await;
    assert_eq!(
        status, 400,
        "case \"an unregistered app id\": expected 400, got {status} {body}"
    );
    assert_eq!(
        field(&body, "code"),
        "ValidationException",
        "case \"an unregistered app id\": expected code ValidationException, got {body}"
    );
    assert_eq!(
        field(&body, "message"),
        invalid_app_id_message(),
        "case \"an unregistered app id\": expected the app-identity message, got {body}"
    );

    // The resource that IS held is still readable: the refusals narrow nothing else.
    let (status, body) = get_storage(&router, APP, ENVIRONMENT, "s3storage").await;
    assert_eq!(
        status, 200,
        "case \"the held resource\": expected 200, got {status} {body}"
    );
}
