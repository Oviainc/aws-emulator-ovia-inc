// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AmplifyBackend backend-environment lifecycle and the async
//! job record every mutating call answers with.
//!
//! Each case drives the real wire path — `MultiServiceRouter::dispatch` with this one
//! service registered — so the route templates, the `restJson1` codec, the modelled
//! constraints and the handlers are all exercised exactly as a served request does.

use std::sync::Arc;
use std::time::Duration;

use aws_smithy_types::date_time::{DateTime, Format};
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifybackend_amplifybackend::state::{
    JOB_COMPLETION_DELAY, JOB_STATUS_COMPLETED, JOB_STATUS_IN_PROGRESS, LIMIT_TYPE_APPS,
    LIMIT_TYPE_AUTH_RESOURCES, LIMIT_TYPE_ENVIRONMENTS, MAX_APPS,
    MAX_AUTH_RESOURCES_PER_ENVIRONMENT, MAX_ENVIRONMENTS_PER_APP, MAX_IDENTIFIER_CHARS,
    MAX_RETAINED_JOBS_PER_APP,
};
use service_com_amazonaws_amplifybackend_amplifybackend::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The endpoint host an AmplifyBackend request addresses; its first label is the routing
/// key the router claims the request by.
const HOST: &str = "amplifybackend.us-east-1.amazonaws.com";

/// A router with just this service mounted on a fresh shared engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
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

/// The same JSON document with every number read as `f64`, so a `double` that
/// round-trips as `12` rather than `12.0` still compares equal.
fn canonical(value: &Value) -> Value {
    match value {
        Value::Number(number) => json!(number.as_f64().unwrap_or_default()),
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|(name, member)| (name.clone(), canonical(member)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// A string member of a response, for an assertion that names the case.
fn field<'a>(body: &'a Value, name: &str) -> &'a str {
    body.get(name).and_then(Value::as_str).unwrap_or_default()
}

/// One of this service's RFC-3339 timestamps as a comparable instant.
///
/// Parsed rather than compared as text: `Format::DateTime` omits a zero fractional part,
/// so `"…:09Z"` and `"…:09.500Z"` do not order correctly as strings.
fn instant(value: &str) -> (i64, u32) {
    let parsed = DateTime::from_str(value, Format::DateTime)
        .unwrap_or_else(|error| panic!("parse {value:?} as an RFC-3339 instant: {error}"));
    (parsed.secs(), parsed.subsec_nanos())
}

/// One lifecycle case: the app, the environments created for it, and the auth resource
/// configured in the first of them.
struct Case {
    name: &'static str,
    app_id: &'static str,
    app_name: &'static str,
    /// The environments to create, in the order the calls are made. The reported
    /// `BackendEnvironmentList` is compared against these names SORTED.
    environments: &'static [&'static str],
    resource_name: &'static str,
    /// The `CreateBackendAuthResourceConfig` to submit, as the wire document.
    resource_config: fn() -> Value,
}

/// The lifecycle cases. They differ along what the slice generalises over: how many
/// environments an app has, the environment and resource names, and the SHAPE of the
/// auth resource configuration (a minimal one, a deeply nested one, and one whose
/// optional sub-structures differ). Enum values come from the model's own value lists
/// (`AuthResources`, `Service`, `SignInMethod`, `RequiredSignUpAttributesElement`,
/// `MFAMode`, `MfaTypesElement`, `AdditionalConstraintsElement`, `DeliveryMethod` in
/// `src/ops/types/shared_types.rs`).
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "one environment, minimal user-pool-only auth",
            app_id: "d1abc2",
            app_name: "demo",
            environments: &["staging"],
            resource_name: "authdemo",
            resource_config: || {
                json!({
                    "authResources": "USER_POOL_ONLY",
                    "service": "COGNITO",
                    "userPoolConfigs": {
                        "requiredSignUpAttributes": ["EMAIL"],
                        "signInMethod": "EMAIL",
                        "userPoolName": "demoPool"
                    }
                })
            },
        },
        Case {
            name: "three environments, identity pool plus MFA, password and verification",
            app_id: "dxyz89wq",
            app_name: "storefront",
            environments: &["prod", "dev", "integ"],
            resource_name: "storefrontAuth",
            resource_config: || {
                json!({
                    "authResources": "IDENTITY_POOL_AND_USER_POOL",
                    "service": "COGNITO",
                    "identityPoolConfigs": {
                        "identityPoolName": "storefrontIdPool",
                        "unauthenticatedLogin": true
                    },
                    "userPoolConfigs": {
                        "requiredSignUpAttributes": ["ADDRESS", "ZONE_INFO", "PHONE_NUMBER"],
                        "signInMethod": "EMAIL_AND_PHONE_NUMBER",
                        "userPoolName": "storefrontPool",
                        "mfa": {
                            "MFAMode": "OPTIONAL",
                            "settings": {
                                "mfaTypes": ["SMS", "TOTP"],
                                "smsMessage": "your storefront code is {####}"
                            }
                        },
                        "passwordPolicy": {
                            "minimumLength": 12.0,
                            "additionalConstraints": ["REQUIRE_DIGIT", "REQUIRE_SYMBOL"]
                        },
                        "verificationMessage": {
                            "deliveryMethod": "EMAIL",
                            "emailSettings": {
                                "emailMessage": "confirm your address",
                                "emailSubject": "storefront sign-up"
                            }
                        }
                    }
                })
            },
        },
        Case {
            name: "two environments, username sign-in with OAuth and SMS verification",
            app_id: "d00000000000zz",
            app_name: "an app name with spaces",
            environments: &["sandbox-2", "sandbox-1"],
            resource_name: "sandboxAuth",
            resource_config: || {
                json!({
                    "authResources": "IDENTITY_POOL_AND_USER_POOL",
                    "service": "COGNITO",
                    "identityPoolConfigs": {
                        "identityPoolName": "sandboxIdPool",
                        "unauthenticatedLogin": false
                    },
                    "userPoolConfigs": {
                        "requiredSignUpAttributes": ["PREFERRED_USERNAME"],
                        "signInMethod": "USERNAME",
                        "userPoolName": "sandboxPool",
                        "oAuth": {
                            "oAuthGrantType": "CODE",
                            "oAuthScopes": ["OPENID", "PROFILE", "AWS_COGNITO_SIGNIN_USER_ADMIN"],
                            "redirectSignInURIs": ["https://example.test/in"],
                            "redirectSignOutURIs": ["https://example.test/out"],
                            "domainPrefix": "sandbox",
                            "socialProviderSettings": {
                                "Google": { "client_id": "google-client", "client_secret": "google-secret" }
                            }
                        },
                        "verificationMessage": {
                            "deliveryMethod": "SMS",
                            "smsSettings": { "smsMessage": "sandbox code {####}" }
                        }
                    }
                })
            },
        },
    ]
}

/// The whole slice, over every case: create the environments, read the backend back,
/// configure auth, read it back verbatim, read the job back, then delete the
/// environment and prove it is gone while its jobs remain.
#[tokio::test]
async fn backend_environment_lifecycle_round_trips_through_state() {
    for case in cases() {
        let router = router();
        let label = case.name;
        let mut create_job_ids = Vec::new();

        for environment in case.environments {
            let (status, body) = call(
                &router,
                "POST",
                "/backend",
                Some(json!({
                    "appId": case.app_id,
                    "appName": case.app_name,
                    "backendEnvironmentName": environment,
                })),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label:?}: CreateBackend for {environment:?} expected 200, got {status} {body}"
            );
            assert_eq!(
                field(&body, "appId"),
                case.app_id,
                "case {label:?}: CreateBackend must report the app it created, got {body}"
            );
            assert_eq!(
                field(&body, "backendEnvironmentName"),
                *environment,
                "case {label:?}: CreateBackend must report the environment it created, got {body}"
            );
            assert_eq!(
                field(&body, "operation"),
                "CreateBackend",
                "case {label:?}: expected Operation=CreateBackend, got {body}"
            );
            assert_eq!(
                field(&body, "status"),
                JOB_STATUS_IN_PROGRESS,
                "case {label:?}: a freshly recorded job is not finished yet, got {body}"
            );
            assert!(
                body.get("error").is_none(),
                "case {label:?}: Error must be absent on success, got {body}"
            );
            let job_id = field(&body, "jobId").to_string();
            assert!(
                !job_id.is_empty(),
                "case {label:?}: CreateBackend must report a JobId, got {body}"
            );
            assert!(
                !create_job_ids.contains(&job_id),
                "case {label:?}: job ids must be unique, got {job_id:?} twice"
            );
            create_job_ids.push(job_id);
        }

        // GetBackend reports what was created, not what was asked for.
        let details_path = format!("/backend/{}/details", case.app_id);
        let (status, body) = call(&router, "POST", &details_path, Some(json!({}))).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetBackend expected 200, got {status} {body}"
        );
        assert_eq!(
            field(&body, "appName"),
            case.app_name,
            "case {label:?}: GetBackend must report the app name CreateBackend was given, got {body}"
        );
        let mut expected: Vec<String> = case.environments.iter().map(|e| (*e).to_string()).collect();
        expected.sort();
        let listed: Vec<String> = body
            .get("backendEnvironmentList")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            listed, expected,
            "case {label:?}: BackendEnvironmentList must be the environments created, got {body}"
        );

        // The requested environment is echoed only when it exists.
        let first = case.environments[0];
        let (_, body) = call(
            &router,
            "POST",
            &details_path,
            Some(json!({ "backendEnvironmentName": first })),
        )
        .await;
        assert_eq!(
            field(&body, "backendEnvironmentName"),
            first,
            "case {label:?}: GetBackend must echo an environment that exists, got {body}"
        );
        let (_, body) = call(
            &router,
            "POST",
            &details_path,
            Some(json!({ "backendEnvironmentName": "never-created" })),
        )
        .await;
        assert!(
            body.get("backendEnvironmentName").is_none(),
            "case {label:?}: GetBackend must not name an environment that does not exist, got {body}"
        );

        // CreateBackendAuth stores the submitted configuration verbatim.
        let submitted = (case.resource_config)();
        let (status, body) = call(
            &router,
            "POST",
            &format!("/backend/{}/auth", case.app_id),
            Some(json!({
                "backendEnvironmentName": first,
                "resourceName": case.resource_name,
                "resourceConfig": submitted,
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateBackendAuth expected 200, got {status} {body}"
        );
        assert_eq!(
            field(&body, "operation"),
            "CreateBackendAuth",
            "case {label:?}: expected Operation=CreateBackendAuth, got {body}"
        );
        let auth_job_id = field(&body, "jobId").to_string();
        assert!(
            !auth_job_id.is_empty() && !create_job_ids.contains(&auth_job_id),
            "case {label:?}: CreateBackendAuth must record its own fresh job, got {body}"
        );

        // GetBackendAuth reports exactly what was stored.
        let auth_details = format!("/backend/{}/auth/{first}/details", case.app_id);
        let (status, body) = call(
            &router,
            "POST",
            &auth_details,
            Some(json!({ "resourceName": case.resource_name })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetBackendAuth expected 200, got {status} {body}"
        );
        assert_eq!(
            field(&body, "resourceName"),
            case.resource_name,
            "case {label:?}: GetBackendAuth must report the resource asked for, got {body}"
        );
        let returned = body.get("resourceConfig").cloned().unwrap_or(Value::Null);
        assert_eq!(
            canonical(&returned),
            canonical(&submitted),
            "case {label:?}: GetBackendAuth must return the stored ResourceConfig unchanged, got {returned}"
        );

        // GetBackendJob reads the stored job back.
        let job_path = format!("/backend/{}/job/{first}/{auth_job_id}", case.app_id);
        let (status, body) = call(&router, "GET", &job_path, None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetBackendJob expected 200, got {status} {body}"
        );
        assert_eq!(
            field(&body, "jobId"),
            auth_job_id,
            "case {label:?}: GetBackendJob must report the job asked for, got {body}"
        );
        assert_eq!(
            field(&body, "operation"),
            "CreateBackendAuth",
            "case {label:?}: the stored job must remember its operation, got {body}"
        );
        let create_time = field(&body, "createTime").to_string();
        let update_time = field(&body, "updateTime").to_string();
        assert!(
            !create_time.is_empty() && !update_time.is_empty(),
            "case {label:?}: a job carries both timestamps, got {body}"
        );
        assert!(
            instant(&update_time) >= instant(&create_time),
            "case {label:?}: UpdateTime must not precede CreateTime, got {create_time} then {update_time}"
        );

        // DeleteBackend removes the environment and its resources, and records a job.
        let (status, body) = call(
            &router,
            "POST",
            &format!("/backend/{}/environments/{first}/remove", case.app_id),
            Some(json!({})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: DeleteBackend expected 200, got {status} {body}"
        );
        assert_eq!(
            field(&body, "operation"),
            "DeleteBackend",
            "case {label:?}: expected Operation=DeleteBackend, got {body}"
        );
        let delete_job_id = field(&body, "jobId").to_string();
        assert!(
            !delete_job_id.is_empty() && delete_job_id != auth_job_id,
            "case {label:?}: DeleteBackend must record its own fresh job, got {body}"
        );

        let (_, body) = call(&router, "POST", &details_path, Some(json!({}))).await;
        let listed: Vec<String> = body
            .get("backendEnvironmentList")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            !listed.contains(&first.to_string()),
            "case {label:?}: a removed environment must not be listed, got {body}"
        );
        assert_eq!(
            listed.len(),
            case.environments.len() - 1,
            "case {label:?}: only the removed environment goes away, got {body}"
        );

        let (status, body) = call(
            &router,
            "POST",
            &auth_details,
            Some(json!({ "resourceName": case.resource_name })),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {label:?}: auth of a removed environment is not found, got {status} {body}"
        );

        // Jobs outlive the environment they ran against.
        let (status, body) = call(&router, "GET", &job_path, None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a job still resolves after its environment is removed, got {status} {body}"
        );
    }
}

/// A job is recorded `IN_PROGRESS` and becomes `COMPLETED` once the instant its work is
/// due has passed — and a settled job reports the SAME `UpdateTime` on every later read.
#[tokio::test]
async fn a_job_completes_once_the_clock_makes_it_due() {
    let router = router();
    let (status, body) = call(
        &router,
        "POST",
        "/backend",
        Some(json!({ "appId": "djob1", "appName": "jobs", "backendEnvironmentName": "staging" })),
    )
    .await;
    assert_eq!(status, 200, "CreateBackend expected 200, got {status} {body}");
    let job_id = field(&body, "jobId").to_string();
    let job_path = format!("/backend/djob1/job/staging/{job_id}");

    let (_, body) = call(&router, "GET", &job_path, None).await;
    assert_eq!(
        field(&body, "status"),
        JOB_STATUS_IN_PROGRESS,
        "a caller reading straight after the call still sees the transient status, got {body}"
    );

    tokio::time::sleep(JOB_COMPLETION_DELAY + Duration::from_millis(100)).await;

    let (_, settled) = call(&router, "GET", &job_path, None).await;
    assert_eq!(
        field(&settled, "status"),
        JOB_STATUS_COMPLETED,
        "a job whose due instant has passed is complete, got {settled}"
    );
    assert!(
        instant(field(&settled, "updateTime")) > instant(field(&settled, "createTime")),
        "completing the job moves UpdateTime past CreateTime, got {settled}"
    );

    let (_, again) = call(&router, "GET", &job_path, None).await;
    assert_eq!(
        field(&again, "updateTime"),
        field(&settled, "updateTime"),
        "a settled job reports a stable UpdateTime, got {again}"
    );
}

/// A job is addressed by (app, environment, job id): an id minted under one environment
/// is not found under another.
#[tokio::test]
async fn a_job_lookup_is_scoped_to_its_environment() {
    let router = router();
    for environment in ["one", "two"] {
        let (status, _) = call(
            &router,
            "POST",
            "/backend",
            Some(json!({
                "appId": "dscope",
                "appName": "scoped",
                "backendEnvironmentName": environment,
            })),
        )
        .await;
        assert_eq!(status, 200, "CreateBackend for {environment:?} expected 200");
    }
    let (_, body) = call(
        &router,
        "POST",
        "/backend",
        Some(json!({ "appId": "dscope", "appName": "scoped", "backendEnvironmentName": "one" })),
    )
    .await;
    let job_id = field(&body, "jobId").to_string();

    let (status, body) = call(&router, "GET", &format!("/backend/dscope/job/one/{job_id}"), None).await;
    assert_eq!(
        status, 200,
        "the job resolves under its own environment, got {status} {body}"
    );
    let (status, body) = call(&router, "GET", &format!("/backend/dscope/job/two/{job_id}"), None).await;
    assert_eq!(
        status, 404,
        "the job must not resolve under another environment, got {status} {body}"
    );
}

/// Every lookup of a resource that does not exist answers the modelled
/// `NotFoundException` (404) — never 501 and never an empty 200 — whichever part of the
/// address is unknown.
///
/// The APP is not one of those parts: an `AppId` that identifies no registered Amplify
/// app is refused earlier, as HTTP 400 `ValidationException` (see
/// `app_registration_gate.rs`), so every row below names a registered app and is unknown
/// in some part BELOW it.
#[tokio::test]
async fn an_unknown_resource_is_a_modelled_not_found() {
    let router = router();
    // Seed one app, one environment, one auth resource, so each row below is unknown in
    // exactly one respect.
    let (status, _) = call(
        &router,
        "POST",
        "/backend",
        Some(json!({ "appId": "dknown", "appName": "known", "backendEnvironmentName": "staging" })),
    )
    .await;
    assert_eq!(status, 200, "seeding CreateBackend expected 200");
    let (status, _) = call(
        &router,
        "POST",
        "/backend/dknown/auth",
        Some(json!({
            "backendEnvironmentName": "staging",
            "resourceName": "authknown",
            "resourceConfig": {
                "authResources": "USER_POOL_ONLY",
                "service": "COGNITO",
                "userPoolConfigs": {
                    "requiredSignUpAttributes": ["EMAIL"],
                    "signInMethod": "EMAIL",
                    "userPoolName": "knownPool"
                }
            }
        })),
    )
    .await;
    assert_eq!(status, 200, "seeding CreateBackendAuth expected 200");

    // (what is unknown, method, path, body)
    let rows: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "the environment, on GetBackendAuth",
            "POST",
            "/backend/dknown/auth/unknownenv/details".to_string(),
            Some(json!({ "resourceName": "authknown" })),
        ),
        (
            "the resource name, on GetBackendAuth",
            "POST",
            "/backend/dknown/auth/staging/details".to_string(),
            Some(json!({ "resourceName": "unknownresource" })),
        ),
        (
            "the job id, on GetBackendJob",
            "GET",
            "/backend/dknown/job/staging/999".to_string(),
            None,
        ),
        (
            "the environment, on CreateBackendAuth",
            "POST",
            "/backend/dknown/auth".to_string(),
            Some(json!({
                "backendEnvironmentName": "unknownenv",
                "resourceName": "authknown",
                "resourceConfig": {
                    "authResources": "USER_POOL_ONLY",
                    "service": "COGNITO",
                    "userPoolConfigs": {
                        "requiredSignUpAttributes": ["EMAIL"],
                        "signInMethod": "EMAIL",
                        "userPoolName": "knownPool"
                    }
                }
            })),
        ),
        (
            "the environment, on DeleteBackend",
            "POST",
            "/backend/dknown/environments/unknownenv/remove".to_string(),
            Some(json!({})),
        ),
    ];

    for (unknown, method, path, body) in rows {
        let (status, response) = call(&router, method, &path, body).await;
        assert_eq!(
            status, 404,
            "case {unknown:?}: expected 404, got {status} {response}"
        );
        assert!(
            !field(&response, "resourceType").is_empty(),
            "case {unknown:?}: NotFoundException names the resource type, got {response}"
        );
        assert!(
            !field(&response, "message").is_empty(),
            "case {unknown:?}: NotFoundException carries a message, got {response}"
        );
    }
}

/// A request missing a required member is refused with this service's own modelled
/// client-input error, `BadRequestException` (400), naming the member.
#[tokio::test]
async fn a_missing_required_member_is_a_bad_request() {
    let router = router();
    let full = json!({ "appId": "dreq", "appName": "required", "backendEnvironmentName": "staging" });

    for absent in ["appId", "appName", "backendEnvironmentName"] {
        let mut body = full.clone();
        body.as_object_mut()
            .expect("the request body is an object")
            .remove(absent);
        let (status, response) = call(&router, "POST", "/backend", Some(body)).await;
        assert_eq!(
            status, 400,
            "case {absent:?}: expected 400, got {status} {response}"
        );
        let code = response
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            code.ends_with("BadRequestException"),
            "case {absent:?}: expected BadRequestException, got {code:?} in {response}"
        );
        assert!(
            field(&response, "message").contains(absent),
            "case {absent:?}: the refusal must name the absent member, got {response}"
        );
    }

    // The same holds inside a nested required structure.
    let (status, response) = call(
        &router,
        "POST",
        "/backend/dreq/auth",
        Some(json!({
            "backendEnvironmentName": "staging",
            "resourceName": "authreq",
            "resourceConfig": {
                "authResources": "USER_POOL_ONLY",
                "service": "COGNITO"
            }
        })),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"userPoolConfigs absent\": expected 400, got {status} {response}"
    );
}

/// A value outside a modelled enum is refused before any state is touched.
#[tokio::test]
async fn a_value_outside_a_modelled_enum_is_refused() {
    let router = router();
    let (status, _) = call(
        &router,
        "POST",
        "/backend",
        Some(json!({ "appId": "denum", "appName": "enums", "backendEnvironmentName": "staging" })),
    )
    .await;
    assert_eq!(status, 200, "seeding CreateBackend expected 200");

    for (member, bad) in [
        ("authResources", json!("EVERYTHING")),
        ("service", json!("NOT_COGNITO")),
    ] {
        let mut config = json!({
            "authResources": "USER_POOL_ONLY",
            "service": "COGNITO",
            "userPoolConfigs": {
                "requiredSignUpAttributes": ["EMAIL"],
                "signInMethod": "EMAIL",
                "userPoolName": "enumPool"
            }
        });
        config
            .as_object_mut()
            .expect("the resource config is an object")
            .insert(member.to_string(), bad.clone());
        let (status, response) = call(
            &router,
            "POST",
            "/backend/denum/auth",
            Some(json!({
                "backendEnvironmentName": "staging",
                "resourceName": "authenum",
                "resourceConfig": config,
            })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {member:?}={bad}: expected 400, got {status} {response}"
        );
    }
}

/// Job records are retained BOUNDEDLY: past `MAX_RETAINED_JOBS_PER_APP` the oldest ones
/// are reclaimed, so repeating a mutating call cannot grow this service's state without
/// limit, while the jobs a caller is still polling stay readable.
#[tokio::test]
async fn job_retention_is_bounded_and_drops_the_oldest_first() {
    let router = router();
    let create = json!({
        "appId": "dbound",
        "appName": "bounded",
        "backendEnvironmentName": "staging",
    });

    let (status, body) = call(&router, "POST", "/backend", Some(create.clone())).await;
    assert_eq!(status, 200, "CreateBackend expected 200, got {status} {body}");
    let oldest = field(&body, "jobId").to_string();

    // One more job than the app retains, so exactly the oldest ones fall off.
    let mut newest = oldest.clone();
    for _ in 0..MAX_RETAINED_JOBS_PER_APP {
        let (status, body) = call(&router, "POST", "/backend", Some(create.clone())).await;
        assert_eq!(status, 200, "CreateBackend expected 200, got {status} {body}");
        newest = field(&body, "jobId").to_string();
    }

    let (status, body) = call(
        &router,
        "GET",
        &format!("/backend/dbound/job/staging/{newest}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"newest\": the job just recorded is retained, got {status} {body}"
    );
    let (status, body) = call(
        &router,
        "GET",
        &format!("/backend/dbound/job/staging/{oldest}"),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"oldest\": the oldest job is reclaimed past the retention bound, got {status} {body}"
    );
}

/// Every store this slice grows is BOUNDED before the insert: past its cap the request
/// is refused with the modelled `TooManyRequestsException` (429) naming the limit, and
/// an over-long identifier is refused with `BadRequestException` (400) — a caller can
/// neither grow the emulator's state without limit nor key a record by unbounded text.
#[tokio::test]
async fn caller_driven_stores_are_bounded() {
    let router = router();
    let auth_config = json!({
        "authResources": "USER_POOL_ONLY",
        "service": "COGNITO",
        "userPoolConfigs": {
            "requiredSignUpAttributes": ["EMAIL"],
            "signInMethod": "EMAIL",
            "userPoolName": "boundedPool"
        }
    });

    // (1) apps. Fill the store with distinct app ids, then one more.
    for index in 0..MAX_APPS {
        let (status, body) = call(
            &router,
            "POST",
            "/backend",
            Some(json!({
                "appId": format!("dapp{index}"),
                "appName": "bounded",
                "backendEnvironmentName": "staging",
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case \"app {index}\": expected 200 under the bound, got {status} {body}"
        );
    }
    let (status, body) = call(
        &router,
        "POST",
        "/backend",
        Some(json!({
            "appId": "done-too-many",
            "appName": "bounded",
            "backendEnvironmentName": "staging",
        })),
    )
    .await;
    assert_eq!(
        status, 429,
        "case \"apps bound\": a new app past the cap is refused, got {status} {body}"
    );
    assert_eq!(
        field(&body, "limitType"),
        LIMIT_TYPE_APPS,
        "case \"apps bound\": the refusal names which limit was hit, got {body}"
    );
    // An app that already exists still works: nothing grows.
    let (status, body) = call(
        &router,
        "POST",
        "/backend",
        Some(json!({
            "appId": "dapp0",
            "appName": "bounded",
            "backendEnvironmentName": "staging",
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"apps bound, existing app\": expected 200, got {status} {body}"
    );

    // (2) environments per app. `dapp0` already has one.
    for index in 1..MAX_ENVIRONMENTS_PER_APP {
        let (status, body) = call(
            &router,
            "POST",
            "/backend",
            Some(json!({
                "appId": "dapp0",
                "appName": "bounded",
                "backendEnvironmentName": format!("env{index}"),
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case \"environment {index}\": expected 200 under the bound, got {status} {body}"
        );
    }
    let (status, body) = call(
        &router,
        "POST",
        "/backend",
        Some(json!({
            "appId": "dapp0",
            "appName": "bounded",
            "backendEnvironmentName": "one-too-many",
        })),
    )
    .await;
    assert_eq!(
        status, 429,
        "case \"environments bound\": a new environment past the cap is refused, got {status} {body}"
    );
    assert_eq!(
        field(&body, "limitType"),
        LIMIT_TYPE_ENVIRONMENTS,
        "case \"environments bound\": the refusal names which limit was hit, got {body}"
    );

    // (3) auth resources per environment.
    for index in 0..MAX_AUTH_RESOURCES_PER_ENVIRONMENT {
        let (status, body) = call(
            &router,
            "POST",
            "/backend/dapp0/auth",
            Some(json!({
                "backendEnvironmentName": "staging",
                "resourceName": format!("auth{index}"),
                "resourceConfig": auth_config,
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case \"auth resource {index}\": expected 200 under the bound, got {status} {body}"
        );
    }
    let (status, body) = call(
        &router,
        "POST",
        "/backend/dapp0/auth",
        Some(json!({
            "backendEnvironmentName": "staging",
            "resourceName": "one-too-many",
            "resourceConfig": auth_config,
        })),
    )
    .await;
    assert_eq!(
        status, 429,
        "case \"auth resources bound\": a new resource past the cap is refused, got {status} {body}"
    );
    assert_eq!(
        field(&body, "limitType"),
        LIMIT_TYPE_AUTH_RESOURCES,
        "case \"auth resources bound\": the refusal names which limit was hit, got {body}"
    );
    // Replacing an existing resource still works: nothing grows.
    let (status, body) = call(
        &router,
        "POST",
        "/backend/dapp0/auth",
        Some(json!({
            "backendEnvironmentName": "staging",
            "resourceName": "auth0",
            "resourceConfig": auth_config,
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"auth resources bound, existing resource\": expected 200, got {status} {body}"
    );

    // (4) identifier length, at the bound and one past it.
    let at_bound = "a".repeat(MAX_IDENTIFIER_CHARS);
    let over_bound = "a".repeat(MAX_IDENTIFIER_CHARS + 1);
    let (status, body) = call(
        &router,
        "POST",
        "/backend",
        Some(json!({
            "appId": "dapp0",
            "appName": "bounded",
            "backendEnvironmentName": at_bound,
        })),
    )
    .await;
    assert_eq!(
        status, 429,
        "case \"identifier at the bound\": accepted by the length check, refused only by the environment cap, got {status} {body}"
    );
    for (member, request) in [
        (
            "appId",
            json!({
                "appId": over_bound,
                "appName": "bounded",
                "backendEnvironmentName": "staging",
            }),
        ),
        (
            "backendEnvironmentName",
            json!({
                "appId": "dapp0",
                "appName": "bounded",
                "backendEnvironmentName": over_bound,
            }),
        ),
    ] {
        let (status, body) = call(&router, "POST", "/backend", Some(request)).await;
        assert_eq!(
            status, 400,
            "case {member:?} over the length bound: expected 400, got {status} {body}"
        );
        assert!(
            field(&body, "message").contains(member),
            "case {member:?} over the length bound: the refusal names the member, got {body}"
        );
    }
}

/// The operations this slice does not implement still answer 501 — an unimplemented
/// operation must not look like an empty success.
#[tokio::test]
async fn operations_outside_this_slice_still_answer_501() {
    let router = router();
    let (status, _) = call(
        &router,
        "POST",
        "/backend",
        Some(json!({ "appId": "dstub", "appName": "stubs", "backendEnvironmentName": "staging" })),
    )
    .await;
    assert_eq!(status, 200, "seeding CreateBackend expected 200");

    let rows: Vec<(&str, &str, &str, Option<Value>)> = vec![
        (
            "CloneBackend",
            "POST",
            "/backend/dstub/environments/staging/clone",
            Some(json!({ "targetEnvironmentName": "copy" })),
        ),
        (
            "RemoveAllBackends",
            "POST",
            "/backend/dstub/remove",
            Some(json!({})),
        ),
    ];
    for (operation, method, path, body) in rows {
        let (status, response) = call(&router, method, path, body).await;
        assert_eq!(
            status, 501,
            "case {operation:?}: a scaffolded operation answers 501, got {status} {response}"
        );
    }
}
