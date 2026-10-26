// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `RemoveAllBackends`.
//!
//! The rule under test, from the operation's own documentation ("Removes all backend
//! environments from your Amplify project", `CleanAmplifyApp` "Cleans up the Amplify
//! Console app if this value is set to true"): the call removes EVERY backend environment
//! the app holds — and with each one the auth, API and storage resources created inside it
//! — and answers with one app-wide async job record, whatever the app held and however many
//! environments it had. `CleanAmplifyApp` decides whether the APP survives: without it the
//! app keeps its record and reads back with no environments, with it the Amplify Console
//! app is cleaned up and nothing keyed by that app id is left in either service.
//!
//! The app addressed is the Amplify PROJECT: the id is resolved against Amplify's own app
//! registry as well as this service's backend records, so a project with no backend here is
//! answered (there is nothing to remove) while an `AppId` no app exists under at all earns
//! this service's app-identity refusal — HTTP 400 with the error code `ValidationException`.
//!
//! Every case drives the real wire path (`MultiServiceRouter::dispatch`), so the route
//! templates, the `restJson1` codec and the handlers are exercised as a served request
//! exercises them. The rows differ along what the rule generalises over: how many
//! environments the app holds (none, one, several), whether those environments hold
//! resources, the FORM of the `CleanAmplifyApp` member (absent, false, true), and the form
//! and size of an app id no registry holds.
//!
//! The resources inside an environment are seeded through the store — the same state a
//! handler reaches — rather than over the wire: `CreateBackendAPI` is not implemented and
//! `CreateBackendStorage` is withdrawn from SDK callers, so no wire path creates them, and
//! the point of the rows is what REMOVAL does to a populated environment.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifybackend_amplifybackend::ops::types::{
    BackendAPIResourceConfig, BackendStoragePermissions, CreateBackendAuthResourceConfig,
    CreateBackendAuthUserPoolConfig, CreateBackendStorageResourceConfig,
};
use service_com_amazonaws_amplifybackend_amplifybackend::state::{
    invalid_app_id_message, ComAmazonawsAmplifybackendAmplifybackendState,
    JOB_ENVIRONMENT_APP_WIDE, JOB_STATUS_PENDING, MAX_IDENTIFIER_CHARS,
    OPERATION_REMOVE_ALL_BACKENDS,
};
use service_com_amazonaws_amplifybackend_amplifybackend::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The endpoint hosts; the first label of each is the route key its service claims.
const BACKEND_HOST: &str = "amplifybackend.us-east-1.amazonaws.com";
const AMPLIFY_HOST: &str = "amplify.us-east-1.amazonaws.com";

/// The wire error code of this service's app-identity refusal, which is what an `AppId`
/// no app exists under earns here too.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The resource name every seeded auth, API and storage resource is stored under.
const RESOURCE: &str = "removeall";

/// A router with just this service mounted, and the shared engine it was mounted on.
fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These tests exercise the services, not SigV4: requests carry no signature.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// This service AND the Amplify control plane on one shared engine, the way
/// `core/router/src/lib.rs` mounts them — what the `CleanAmplifyApp` rows need, since the
/// Amplify Console app is Amplify's own resource.
fn router_with_amplify() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (mut router, engine) = router_with_engine();
    router.register_service(service_com_amazonaws_amplify_amplify::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// Send one request to `host` and read back `(status, body)`. `body` is `Value::Null` when
/// the response carries no JSON document.
async fn call(
    router: &MultiServiceRouter,
    host: &str,
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
        .uri(format!("https://{host}{path}"))
        .header("host", host)
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

/// `CreateBackend`: register `app_id` with one backend environment.
async fn create_backend(router: &MultiServiceRouter, app_id: &str, environment: &str) -> u16 {
    call(
        router,
        BACKEND_HOST,
        "POST",
        "/backend",
        Some(json!({
            "appId": app_id,
            "appName": "removeall",
            "backendEnvironmentName": environment,
        })),
    )
    .await
    .0
}

/// `RemoveAllBackends`: `POST /backend/{AppId}/remove`, with `CleanAmplifyApp` sent only
/// when the case names it.
async fn remove_all_backends(
    router: &MultiServiceRouter,
    app_id: &str,
    clean_amplify_app: Option<bool>,
) -> (u16, Value) {
    let body = match clean_amplify_app {
        Some(clean) => json!({ "cleanAmplifyApp": clean }),
        None => json!({}),
    };
    call(
        router,
        BACKEND_HOST,
        "POST",
        &format!("/backend/{app_id}/remove"),
        Some(body),
    )
    .await
}

/// `GetBackend`: the read that reports which environments an app holds.
async fn get_backend(router: &MultiServiceRouter, app_id: &str) -> (u16, Value) {
    call(
        router,
        BACKEND_HOST,
        "POST",
        &format!("/backend/{app_id}/details"),
        Some(json!({})),
    )
    .await
}

/// A minimal valid auth resource configuration, as the store holds it. The enum values
/// come from the model's own value lists (`AuthResources`, `Service`, `SignInMethod`,
/// `RequiredSignUpAttributesElement` in `src/ops/types/shared_types.rs`).
fn auth_resource() -> CreateBackendAuthResourceConfig {
    CreateBackendAuthResourceConfig {
        auth_resources: "USER_POOL_ONLY".to_string(),
        identity_pool_configs: None,
        service: "COGNITO".to_string(),
        user_pool_configs: CreateBackendAuthUserPoolConfig {
            required_sign_up_attributes: vec!["EMAIL".to_string()],
            sign_in_method: "EMAIL".to_string(),
            user_pool_name: "removeallPool".to_string(),
            ..CreateBackendAuthUserPoolConfig::default()
        },
    }
}

/// A minimal API resource configuration, as the store holds it.
fn api_resource() -> BackendAPIResourceConfig {
    BackendAPIResourceConfig {
        api_name: Some("removeallApi".to_string()),
        service: Some("AppSync".to_string()),
        ..BackendAPIResourceConfig::default()
    }
}

/// A minimal storage resource configuration, as the store holds it. The permission values
/// come from the model's `BackendStoragePermissions` value list.
fn storage_resource() -> CreateBackendStorageResourceConfig {
    CreateBackendStorageResourceConfig {
        bucket_name: Some("removeall-storage-bucket".to_string()),
        permissions: BackendStoragePermissions {
            authenticated: vec!["READ".to_string()],
            un_authenticated: None,
        },
        service_name: "S3".to_string(),
    }
}

/// Put one auth, one API and one storage resource inside every environment the app holds,
/// reaching the store the way a handler does.
fn seed_resources(engine: &Arc<engine::InMemoryEngine>, app_id: &str, environments: &[&str]) {
    let store = engine.state::<ComAmazonawsAmplifybackendAmplifybackendState>();
    let mut held = store.lock();
    let app = held
        .app_mut(app_id)
        .expect("CreateBackend registered the app");
    for name in environments {
        let environment = app
            .environment_mut(name)
            .expect("CreateBackend created the environment");
        environment
            .auth
            .insert(RESOURCE.to_string(), auth_resource());
        environment.api.insert(RESOURCE.to_string(), api_resource());
        environment
            .storage
            .insert(RESOURCE.to_string(), storage_resource());
    }
}

/// How many environments the store holds for `app_id`, and whether it holds an app record
/// at all.
fn stored_environments(
    engine: &Arc<engine::InMemoryEngine>,
    app_id: &str,
) -> (bool, usize, usize) {
    let store = engine.state::<ComAmazonawsAmplifybackendAmplifybackendState>();
    let held = store.lock();
    match held.app(app_id) {
        Some(app) => {
            let resources: usize = app
                .environments
                .values()
                .map(|environment| {
                    environment.auth.len() + environment.api.len() + environment.storage.len()
                })
                .sum();
            (true, app.environments.len(), resources)
        }
        None => (false, 0, 0),
    }
}

/// One case: the app, the environments it holds when `RemoveAllBackends` is called, and
/// the `CleanAmplifyApp` member of that call.
struct Case {
    name: &'static str,
    app_id: &'static str,
    /// The environments to create, in the order the calls are made. Their names are 2-10
    /// lowercase letters, which is the form AmplifyBackend accepts.
    environments: &'static [&'static str],
    /// Whether the seeded environments also hold an auth, an API and a storage resource.
    with_resources: bool,
    /// `CleanAmplifyApp` as sent: absent, or the boolean value.
    clean_amplify_app: Option<bool>,
}

/// The cases. They differ along what the rule generalises over — the NUMBER of
/// environments removed (none, one, several), whether those environments hold resources,
/// and the form of `CleanAmplifyApp` (absent, false, true) — and none of them is special
/// in the handler.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "one environment, no resources, CleanAmplifyApp absent",
            app_id: "d1ab2cd3ef4gh5",
            environments: &["dev"],
            with_resources: false,
            clean_amplify_app: None,
        },
        Case {
            name: "one environment holding every kind of resource",
            app_id: "d2kk9zz1qq3wx7",
            environments: &["staging"],
            with_resources: true,
            clean_amplify_app: None,
        },
        Case {
            name: "three environments, each holding resources",
            app_id: "d3mn4op5qr6st7",
            environments: &["dev", "staging", "prodone"],
            with_resources: true,
            clean_amplify_app: Some(false),
        },
        Case {
            name: "an app that already holds no environment",
            app_id: "d4uv5wx6yz7ab8",
            environments: &[],
            with_resources: false,
            clean_amplify_app: Some(false),
        },
        Case {
            name: "three environments, CleanAmplifyApp true",
            app_id: "d5cd6ef7gh8ij9",
            environments: &["dev", "staging", "prodone"],
            with_resources: true,
            clean_amplify_app: Some(true),
        },
        Case {
            name: "one environment, CleanAmplifyApp true",
            app_id: "d6kl7mn8op9qr0",
            environments: &["dev"],
            with_resources: true,
            clean_amplify_app: Some(true),
        },
    ]
}

/// Bring the case's app into being: register one environment per name, and empty the app
/// again through `DeleteBackend` when the case holds none — the only wire path to an app
/// record with no environment.
async fn seed_case(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &Case,
) {
    // An app's identity is settled against Amplify's registry before a backend can be
    // created for it, so the case's app goes there first (`crate::amplify_registry`).
    crate::amplify_registry::register_amplify_app(engine, case.app_id, "removeall");
    let created: &[&str] = if case.environments.is_empty() {
        &["dev"]
    } else {
        case.environments
    };
    for environment in created {
        let status = create_backend(router, case.app_id, environment).await;
        assert_eq!(
            status, 200,
            "case {:?}: seeding CreateBackend for {environment:?} expected 200, got {status}",
            case.name
        );
    }
    if case.environments.is_empty() {
        let (status, body) = call(
            router,
            BACKEND_HOST,
            "POST",
            &format!("/backend/{}/environments/dev/remove", case.app_id),
            Some(json!({})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: seeding DeleteBackend expected 200, got {status} {body}",
            case.name
        );
    }
    if case.with_resources {
        seed_resources(engine, case.app_id, case.environments);
    }
}

/// Every environment the app holds goes, with the resources inside it, and the call
/// answers one app-wide job record. `CleanAmplifyApp` decides whether the app record
/// itself survives.
#[tokio::test]
async fn remove_all_backends_removes_every_environment_the_app_holds() {
    for case in cases() {
        // One router per case: the call changes the app, and each case states the app as
        // its own seeding leaves it.
        let (router, engine) = router_with_engine();
        seed_case(&router, &engine, &case).await;

        let (status, body) = remove_all_backends(&router, case.app_id, case.clean_amplify_app).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status} {body}",
            case.name
        );
        assert_eq!(
            field(&body, "appId"),
            case.app_id,
            "case {:?}: the answer is about the app asked for, got {body}",
            case.name
        );
        assert_eq!(
            field(&body, "operation"),
            OPERATION_REMOVE_ALL_BACKENDS,
            "case {:?}: the job names the operation that recorded it, got {body}",
            case.name
        );
        assert_eq!(
            field(&body, "status"),
            JOB_STATUS_PENDING,
            "case {:?}: a freshly recorded job has not started yet, got {body}",
            case.name
        );
        let job_id = field(&body, "jobId").to_string();
        assert!(
            !job_id.is_empty(),
            "case {:?}: the call answers with a job id, got {body}",
            case.name
        );
        assert!(
            body.get("error").is_none(),
            "case {:?}: a successful call reports no error, got {body}",
            case.name
        );
        assert!(
            body.get("backendEnvironmentName").is_none(),
            "case {:?}: the answer names no environment, got {body}",
            case.name
        );

        let clean = case.clean_amplify_app.unwrap_or(false);
        let (held, environments, resources) = stored_environments(&engine, case.app_id);
        assert_eq!(
            held, !clean,
            "case {:?}: the app record is kept unless CleanAmplifyApp asked for it, got held={held}",
            case.name
        );
        assert_eq!(
            environments, 0,
            "case {:?}: expected every environment removed, got {environments} left",
            case.name
        );
        assert_eq!(
            resources, 0,
            "case {:?}: the resources inside an environment go with it, got {resources} left",
            case.name
        );

        let (read_status, read_body) = get_backend(&router, case.app_id).await;
        if clean {
            // The app was cleaned up, so this service no longer holds it: the read earns
            // the app-identity refusal, exactly as it would for an app id never registered.
            assert_eq!(
                read_status, 400,
                "case {:?}: GetBackend after a clean-up expected 400, got {read_status} {read_body}",
                case.name
            );
            assert_eq!(
                field(&read_body, "code"),
                VALIDATION_EXCEPTION,
                "case {:?}: expected code {VALIDATION_EXCEPTION:?}, got {read_body}",
                case.name
            );
            assert_eq!(
                field(&read_body, "message"),
                invalid_app_id_message(),
                "case {:?}: the refusal states the app id is invalid, got {read_body}",
                case.name
            );
        } else {
            assert_eq!(
                read_status, 200,
                "case {:?}: GetBackend expected 200, got {read_status} {read_body}",
                case.name
            );
            assert_eq!(
                read_body.get("backendEnvironmentList"),
                Some(&json!([])),
                "case {:?}: the app reads back with no environment, got {read_body}",
                case.name
            );
            assert_eq!(
                field(&read_body, "appId"),
                case.app_id,
                "case {:?}: the app itself survives the call, got {read_body}",
                case.name
            );

            // The job is the app's, not an environment's: it carries no environment name,
            // so no environment's job listing claims it.
            let store = engine.state::<ComAmazonawsAmplifybackendAmplifybackendState>();
            let held_store = store.lock();
            let app = held_store
                .app(case.app_id)
                .expect("the app record survives a call that did not clean it up");
            let job = app
                .jobs
                .get(&job_id)
                .unwrap_or_else(|| panic!("case {:?}: job {job_id} is in the store", case.name));
            assert_eq!(
                job.backend_environment_name, JOB_ENVIRONMENT_APP_WIDE,
                "case {:?}: an app-wide job names no environment, got {:?}",
                case.name, job.backend_environment_name
            );
        }
    }
}

/// The resources created inside an environment are unreachable afterwards: the reads that
/// resolved before the call answer 404 once the environment holding them is gone.
#[tokio::test]
async fn the_resources_inside_a_removed_environment_are_gone() {
    let app_id = "d7st8uv9wx0yz1";
    let environments: &[&str] = &["dev", "staging"];
    let (router, engine) = router_with_engine();
    crate::amplify_registry::register_amplify_app(&engine, app_id, "removeall");
    for environment in environments {
        let status = create_backend(&router, app_id, environment).await;
        assert_eq!(status, 200, "seeding CreateBackend expected 200, got {status}");
    }
    seed_resources(&engine, app_id, environments);

    // (case, method, path, body) — the two reads that report a resource held inside an
    // environment.
    let reads = |environment: &str| {
        vec![
            (
                "GetBackendAuth",
                format!("/backend/{app_id}/auth/{environment}/details"),
            ),
            (
                "GetBackendStorage",
                format!("/backend/{app_id}/storage/{environment}/details"),
            ),
        ]
    };

    for environment in environments {
        for (name, path) in reads(environment) {
            let (status, body) = call(
                &router,
                BACKEND_HOST,
                "POST",
                &path,
                Some(json!({ "resourceName": RESOURCE })),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {name:?} in {environment:?}: the seeded resource reads back before the \
                 removal, got {status} {body}"
            );
        }
    }

    let (status, body) = remove_all_backends(&router, app_id, None).await;
    assert_eq!(status, 200, "RemoveAllBackends expected 200, got {status} {body}");

    for environment in environments {
        for (name, path) in reads(environment) {
            let (status, body) = call(
                &router,
                BACKEND_HOST,
                "POST",
                &path,
                Some(json!({ "resourceName": RESOURCE })),
            )
            .await;
            assert_eq!(
                status, 404,
                "case {name:?} in {environment:?}: the resource went with its environment, got \
                 {status} {body}"
            );
        }
    }
}

/// Removing all backends twice is the same answer twice: the second call finds nothing
/// left to remove, still succeeds, and records its own job — so a retry is safe.
#[tokio::test]
async fn removing_all_backends_twice_succeeds_and_records_two_jobs() {
    let app_id = "d8ab9cd0ef1gh2";
    let (router, engine) = router_with_engine();
    crate::amplify_registry::register_amplify_app(&engine, app_id, "removeall");
    for environment in ["dev", "staging"] {
        let status = create_backend(&router, app_id, environment).await;
        assert_eq!(status, 200, "seeding CreateBackend expected 200, got {status}");
    }

    let (first_status, first) = remove_all_backends(&router, app_id, None).await;
    let (second_status, second) = remove_all_backends(&router, app_id, None).await;
    assert_eq!(
        (first_status, second_status),
        (200, 200),
        "case \"repeated removal\": both calls expected 200, got {first_status} {first} and \
         {second_status} {second}"
    );
    assert_ne!(
        field(&first, "jobId"),
        field(&second, "jobId"),
        "case \"repeated removal\": each call records its own job, got {first} and {second}"
    );

    let (read_status, read_body) = get_backend(&router, app_id).await;
    assert_eq!(
        read_status, 200,
        "case \"repeated removal\": GetBackend expected 200, got {read_status} {read_body}"
    );
    assert_eq!(
        read_body.get("backendEnvironmentList"),
        Some(&json!([])),
        "case \"repeated removal\": the app still holds no environment, got {read_body}"
    );

    // Neither app-wide job is claimed by an environment's job listing, including the
    // environments the first call removed.
    for environment in ["dev", "staging"] {
        let (status, body) = call(
            &router,
            BACKEND_HOST,
            "POST",
            &format!("/backend/{app_id}/job/{environment}"),
            Some(json!({})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {environment:?}: ListBackendJobs expected 200, got {status} {body}"
        );
        let listed: Vec<&str> = body
            .get("jobs")
            .and_then(Value::as_array)
            .map(|jobs| {
                jobs.iter()
                    .filter_map(|job| job.get("operation").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            !listed.contains(&OPERATION_REMOVE_ALL_BACKENDS),
            "case {environment:?}: an app-wide job is not one of this environment's jobs, got \
             {body}"
        );
    }

    let (held, environments, resources) = stored_environments(&engine, app_id);
    assert!(
        held && environments == 0 && resources == 0,
        "case \"repeated removal\": the app is held and empty, got held={held}, \
         {environments} environment(s), {resources} resource(s)"
    );
}

/// An `AppId` no app exists under — in Amplify's registry or this service's backend
/// records — is refused with this service's app-identity refusal (HTTP 400,
/// `ValidationException`), and nothing is recorded for it. The rows differ in the FORM and
/// SIZE of the id; none of them is special in the handler.
#[tokio::test]
async fn an_app_id_no_app_exists_under_is_refused() {
    // The ids Amplify mints match `^d[a-z0-9]+$` with length 1..20 (the `App` shape's own
    // constraints), while AmplifyBackend models its `AppId` as a bare `__string` label with
    // no pattern — so every row is a well-formed request for this service that simply
    // names no app.
    let rows: Vec<(&str, String)> = vec![
        ("an id no app ever had", "d12440e142099".to_string()),
        (
            "the shortest id Amplify's pattern allows",
            "d0".to_string(),
        ),
        (
            "an id outside Amplify's pattern altogether",
            "not-an-app-id".to_string(),
        ),
        (
            "an id at this emulator's identifier length bound",
            format!("d{}", "9".repeat(MAX_IDENTIFIER_CHARS - 1)),
        ),
    ];

    for (case, app_id) in rows {
        // Amplify is mounted, so the refusal is not an artefact of an unreachable
        // registry: it is what an id neither registry holds earns.
        let (router, engine) = router_with_amplify();
        let (created_status, created) = call(
            &router,
            AMPLIFY_HOST,
            "POST",
            "/apps",
            Some(json!({ "name": "removeall" })),
        )
        .await;
        assert_eq!(
            created_status, 200,
            "case {case:?}: seeding Amplify CreateApp expected 200, got {created_status} {created}"
        );

        for clean_amplify_app in [None, Some(false), Some(true)] {
            let (status, body) = remove_all_backends(&router, &app_id, clean_amplify_app).await;
            assert_eq!(
                status, 400,
                "case {case:?} with CleanAmplifyApp {clean_amplify_app:?}: expected 400, got \
                 {status} {body}"
            );
            assert_eq!(
                field(&body, "code"),
                VALIDATION_EXCEPTION,
                "case {case:?}: expected code {VALIDATION_EXCEPTION:?}, got {body}"
            );
            assert_eq!(
                field(&body, "message"),
                invalid_app_id_message(),
                "case {case:?}: the refusal states the app id is invalid, got {body}"
            );
            assert!(
                body.get("jobId").is_none(),
                "case {case:?}: a refusal records no job, got {body}"
            );
        }

        let store = engine.state::<ComAmazonawsAmplifybackendAmplifybackendState>();
        let held = store.lock();
        assert!(
            !held.apps.contains_key(&app_id),
            "case {case:?}: a refused call brings no app record into being"
        );
    }
}

/// An Amplify project that exists with NO backend here is answered, not refused: there is
/// nothing to remove, so the call reports a job and leaves the store exactly as it was —
/// no backend record is brought into being for a project that has none.
#[tokio::test]
async fn an_amplify_project_with_no_backend_here_is_answered() {
    let app_name = "project-without-a-backend";
    let (router, engine) = router_with_amplify();

    let (created_status, created) = call(
        &router,
        AMPLIFY_HOST,
        "POST",
        "/apps",
        Some(json!({ "name": app_name })),
    )
    .await;
    assert_eq!(
        created_status, 200,
        "seeding Amplify CreateApp expected 200, got {created_status} {created}"
    );
    let app_id = created
        .get("app")
        .and_then(|app| app.get("appId"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !app_id.is_empty(),
        "Amplify minted an app id, got {created}"
    );

    // No CreateBackend for this app: this service holds no record for it at all.
    let (held_before, _, _) = stored_environments(&engine, &app_id);
    assert!(
        !held_before,
        "case \"project with no backend\": this service holds no record before the call"
    );

    let (status, body) = remove_all_backends(&router, &app_id, None).await;
    assert_eq!(
        status, 200,
        "case \"project with no backend\": expected 200, got {status} {body}"
    );
    assert_eq!(
        field(&body, "appId"),
        app_id,
        "case \"project with no backend\": the answer is about the app asked for, got {body}"
    );
    assert_eq!(
        field(&body, "operation"),
        OPERATION_REMOVE_ALL_BACKENDS,
        "case \"project with no backend\": the job names this operation, got {body}"
    );
    assert_eq!(
        field(&body, "status"),
        JOB_STATUS_PENDING,
        "case \"project with no backend\": the job has not started yet, got {body}"
    );
    let first_job_id = field(&body, "jobId").to_string();
    assert!(
        !first_job_id.is_empty(),
        "case \"project with no backend\": the call answers with a job id, got {body}"
    );

    // Nothing was stored: the project has no backend afterwards either, so the reads that
    // answer about a backend answer exactly as they did before the call.
    let (held_after, _, _) = stored_environments(&engine, &app_id);
    assert!(
        !held_after,
        "case \"project with no backend\": the call brings no backend record into being"
    );
    let (read_status, read_body) = get_backend(&router, &app_id).await;
    assert_eq!(
        read_status, 404,
        "case \"project with no backend\": GetBackend answers about the backend the project \
         does not have, got {read_status} {read_body}"
    );
    assert_eq!(
        field(&read_body, "code"),
        "NotFoundException",
        "case \"project with no backend\": a project Amplify holds is not refused as a bad \
         app id — the BACKEND is what is missing, got {read_body}"
    );

    // A second call is the same answer with its own job id: the store is unchanged, so
    // nothing about the call depends on the first one.
    let (again_status, again) = remove_all_backends(&router, &app_id, None).await;
    assert_eq!(
        again_status, 200,
        "case \"project with no backend\": a repeated call expected 200, got {again_status} \
         {again}"
    );
    assert_ne!(
        field(&again, "jobId"),
        first_job_id.as_str(),
        "case \"project with no backend\": each call reports its own job id, got {again}"
    );

    // The Amplify project itself is untouched: nothing asked for it to be cleaned up.
    let (get_app_status, get_app_body) =
        call(&router, AMPLIFY_HOST, "GET", &format!("/apps/{app_id}"), None).await;
    assert_eq!(
        get_app_status, 200,
        "case \"project with no backend\": Amplify GetApp expected 200, got {get_app_status} \
         {get_app_body}"
    );
}

/// `CleanAmplifyApp` is what reaches the Amplify Console app: set, the Amplify control
/// plane no longer holds the app; absent or false, it still does. Both rows run with
/// Amplify mounted beside this service on one shared engine, and the app id is the one
/// Amplify minted.
#[tokio::test]
async fn clean_amplify_app_cleans_up_the_amplify_console_app() {
    // (case, CleanAmplifyApp as sent, whether Amplify still holds the app afterwards)
    let rows: Vec<(&str, Option<bool>, bool)> = vec![
        ("CleanAmplifyApp absent", None, true),
        ("CleanAmplifyApp false", Some(false), true),
        ("CleanAmplifyApp true", Some(true), false),
    ];

    for (case, clean_amplify_app, app_survives) in rows {
        let (router, engine) = router_with_amplify();

        let (created_status, created) = call(
            &router,
            AMPLIFY_HOST,
            "POST",
            "/apps",
            Some(json!({ "name": "removeall" })),
        )
        .await;
        assert_eq!(
            created_status, 200,
            "case {case:?}: seeding Amplify CreateApp expected 200, got {created_status} {created}"
        );
        let app_id = created
            .get("app")
            .and_then(|app| app.get("appId"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            !app_id.is_empty(),
            "case {case:?}: Amplify minted an app id, got {created}"
        );

        let status = create_backend(&router, &app_id, "dev").await;
        assert_eq!(
            status, 200,
            "case {case:?}: seeding CreateBackend expected 200, got {status}"
        );

        // A live challenge token for the same app, so the rows also pin what a clean-up
        // does to the other store this service keys by an app id.
        let (token_status, token) = call(
            &router,
            BACKEND_HOST,
            "POST",
            &format!("/backend/{app_id}/challenge"),
            Some(json!({})),
        )
        .await;
        assert_eq!(
            token_status, 200,
            "case {case:?}: seeding CreateToken expected 200, got {token_status} {token}"
        );

        let (status, body) = remove_all_backends(&router, &app_id, clean_amplify_app).await;
        assert_eq!(
            status, 200,
            "case {case:?}: RemoveAllBackends expected 200, got {status} {body}"
        );

        let (get_app_status, get_app_body) = call(
            &router,
            AMPLIFY_HOST,
            "GET",
            &format!("/apps/{app_id}"),
            None,
        )
        .await;
        let expected_status = if app_survives { 200 } else { 404 };
        assert_eq!(
            get_app_status, expected_status,
            "case {case:?}: Amplify GetApp expected {expected_status}, got {get_app_status} \
             {get_app_body}"
        );

        let store = engine.state::<ComAmazonawsAmplifybackendAmplifybackendState>();
        let held = store.lock();
        assert_eq!(
            held.apps.contains_key(&app_id),
            app_survives,
            "case {case:?}: this service holds the app record only while the app exists"
        );
        assert_eq!(
            held.challenge_tokens.contains_key(&app_id),
            app_survives,
            "case {case:?}: a cleaned-up app keeps no challenge token"
        );
    }
}
