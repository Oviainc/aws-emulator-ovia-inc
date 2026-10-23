// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListBackendJobs`: the enumeration of the job records every
//! mutating AmplifyBackend call leaves behind.
//!
//! The rule under test is that the operation REPORTS stored state — the same records
//! `GetBackendJob` reads one at a time — narrowed by the request's `JobId`, `Operation`
//! and `Status` filters, ordered newest first, and cut into pages of at most
//! `MaxResults` with a `NextToken` that resumes exactly where the previous page stopped.
//! Each case drives the real wire path (`MultiServiceRouter::dispatch` with this one
//! service registered), so the route templates, the `restJson1` codec, the modelled
//! constraints and the handlers are exercised as a served request exercises them.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_amplifybackend_amplifybackend::state::{
    JOB_COMPLETION_DELAY, JOB_STATUS_COMPLETED, JOB_STATUS_FAILED, JOB_STATUS_IN_PROGRESS,
    OPERATION_CREATE_BACKEND, OPERATION_CREATE_BACKEND_AUTH, OPERATION_DELETE_BACKEND,
};
use service_com_amazonaws_amplifybackend_amplifybackend::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The endpoint host an AmplifyBackend request addresses; its first label is the routing
/// key the router claims the request by.
const HOST: &str = "amplifybackend.us-east-1.amazonaws.com";

/// `ListBackendJobsRequest$MaxResults` is modelled `@range{min:1,max:25}`; these are the
/// published bounds, which the pagination cases walk and step past.
const MIN_MAX_RESULTS: i64 = 1;
const MAX_MAX_RESULTS: i64 = 25;

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

/// The `jobs` array of a `ListBackendJobs` response.
fn listed(body: &Value) -> Vec<Value> {
    body.get("jobs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `jobId` of every record in a `ListBackendJobs` response, in reported order.
fn listed_ids(body: &Value) -> Vec<String> {
    listed(body)
        .iter()
        .map(|job| field(job, "jobId").to_string())
        .collect()
}

/// A minimal valid `CreateBackendAuthResourceConfig`, as the wire document. The enum
/// values come from the model's own value lists (`AuthResources`, `Service`,
/// `SignInMethod`, `RequiredSignUpAttributesElement` in `src/ops/types/shared_types.rs`).
fn auth_config(user_pool_name: &str) -> Value {
    json!({
        "authResources": "USER_POOL_ONLY",
        "service": "COGNITO",
        "userPoolConfigs": {
            "requiredSignUpAttributes": ["EMAIL"],
            "signInMethod": "EMAIL",
            "userPoolName": user_pool_name
        }
    })
}

/// One mutating AmplifyBackend operation this slice implements. Each one answers with a
/// job record, which is what `ListBackendJobs` enumerates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seed {
    CreateBackend,
    CreateBackendAuth,
    DeleteBackend,
}

impl Seed {
    /// The `Operation` the recorded job reports, which is the operation's own name.
    fn operation(self) -> &'static str {
        match self {
            Seed::CreateBackend => OPERATION_CREATE_BACKEND,
            Seed::CreateBackendAuth => OPERATION_CREATE_BACKEND_AUTH,
            Seed::DeleteBackend => OPERATION_DELETE_BACKEND,
        }
    }
}

/// Run one seed call against `app_id`/`environment` and hand back the `JobId` it minted.
/// `index` keeps the auth resource names distinct within one case.
async fn seed(
    router: &MultiServiceRouter,
    label: &str,
    app_id: &str,
    environment: &str,
    index: usize,
    step: Seed,
) -> String {
    let (method, path, body) = match step {
        Seed::CreateBackend => (
            "POST",
            "/backend".to_string(),
            json!({
                "appId": app_id,
                "appName": format!("{app_id}-app"),
                "backendEnvironmentName": environment
            }),
        ),
        Seed::CreateBackendAuth => (
            "POST",
            format!("/backend/{app_id}/auth"),
            json!({
                "backendEnvironmentName": environment,
                "resourceName": format!("auth{index}"),
                "resourceConfig": auth_config(&format!("pool{index}"))
            }),
        ),
        Seed::DeleteBackend => (
            "POST",
            format!("/backend/{app_id}/environments/{environment}/remove"),
            json!({}),
        ),
    };
    let (status, body) = call(router, method, &path, Some(body)).await;
    assert_eq!(
        status, 200,
        "case {label:?}: seeding {step:?} expected 200, got {status} {body}"
    );
    let job_id = field(&body, "jobId").to_string();
    assert!(
        !job_id.is_empty(),
        "case {label:?}: seeding {step:?} must answer with a job id, got {body}"
    );
    job_id
}

/// Run every seed step of a case in order, handing back the minted job ids in call order.
async fn seed_all(
    router: &MultiServiceRouter,
    label: &str,
    app_id: &str,
    environment: &str,
    steps: &[Seed],
) -> Vec<String> {
    let mut ids = Vec::new();
    for (index, step) in steps.iter().enumerate() {
        ids.push(seed(router, label, app_id, environment, index, *step).await);
    }
    ids
}

/// List one environment's jobs, asserting a 200.
async fn list(
    router: &MultiServiceRouter,
    label: &str,
    app_id: &str,
    environment: &str,
    request: Value,
) -> Value {
    let (status, body) = call(
        router,
        "POST",
        &format!("/backend/{app_id}/job/{environment}"),
        Some(request),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label:?}: ListBackendJobs expected 200, got {status} {body}"
    );
    body
}

/// Wait out a freshly recorded job's completion delay, so every seeded job has settled
/// and reports a stable `Status` and `UpdateTime`.
async fn settle() {
    tokio::time::sleep(JOB_COMPLETION_DELAY + Duration::from_millis(100)).await;
}

/// One listing case: the jobs recorded for an environment, and what the enumeration of
/// them must report.
struct Case {
    name: &'static str,
    app_id: &'static str,
    environment: &'static str,
    /// The mutating calls to make, in order. Each adds one job record.
    steps: &'static [Seed],
}

/// The listing cases. They differ along what the rule generalises over: HOW MANY records
/// an environment has accumulated, WHICH operations recorded them (one kind, several
/// kinds, the same kind repeated), the environment name, and whether the environment the
/// jobs belong to still exists at listing time (the last case deletes it).
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "one record",
            app_id: "dlist1",
            environment: "staging",
            steps: &[Seed::CreateBackend],
        },
        Case {
            name: "two records, two operations",
            app_id: "dlist2",
            environment: "integration",
            steps: &[Seed::CreateBackend, Seed::CreateBackendAuth],
        },
        Case {
            name: "the same operation repeated",
            app_id: "dlist3",
            environment: "p",
            steps: &[
                Seed::CreateBackend,
                Seed::CreateBackend,
                Seed::CreateBackend,
            ],
        },
        Case {
            name: "records of a deleted environment",
            app_id: "dlist4",
            environment: "ephemeral",
            steps: &[
                Seed::CreateBackend,
                Seed::CreateBackendAuth,
                Seed::DeleteBackend,
            ],
        },
        Case {
            name: "many records, interleaved operations",
            app_id: "dlist5",
            environment: "dev-2",
            steps: &[
                Seed::CreateBackend,
                Seed::CreateBackendAuth,
                Seed::CreateBackend,
                Seed::CreateBackendAuth,
                Seed::CreateBackend,
            ],
        },
    ]
}

/// Over every case: `ListBackendJobs` reports exactly the records the mutating calls
/// left, newest first, and each record is the one `GetBackendJob` reads back for the same
/// job id — the listing computes from the stored state, not from the request.
#[tokio::test]
async fn a_listing_reports_every_recorded_job_newest_first() {
    for case in cases() {
        let router = router();
        let label = case.name;
        let seeded = seed_all(&router, label, case.app_id, case.environment, case.steps).await;

        // The job just minted is still transient: a caller listing straight after the
        // mutating call observes it IN_PROGRESS, and it is reported FIRST.
        let body = list(&router, label, case.app_id, case.environment, json!({})).await;
        let first = listed(&body).first().cloned().unwrap_or(Value::Null);
        assert_eq!(
            field(&first, "status"),
            JOB_STATUS_IN_PROGRESS,
            "case {label:?}: the newest record is still in progress, got {first}"
        );

        settle().await;
        let body = list(&router, label, case.app_id, case.environment, json!({})).await;

        let mut newest_first = seeded.clone();
        newest_first.reverse();
        assert_eq!(
            listed_ids(&body),
            newest_first,
            "case {label:?}: expected the recorded job ids newest first, got {body}"
        );
        assert!(
            body.get("nextToken").is_none(),
            "case {label:?}: a listing that reached the end mints no token, got {body}"
        );

        for (position, record) in listed(&body).iter().enumerate() {
            let job_id = field(record, "jobId");
            let (status, read_back) = call(
                &router,
                "GET",
                &format!("/backend/{}/job/{}/{job_id}", case.app_id, case.environment),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label:?}: GetBackendJob of listed job {job_id} expected 200, got {status} {read_back}"
            );
            assert_eq!(
                record, &read_back,
                "case {label:?}: listed record {position} must equal what GetBackendJob reports, got {record} vs {read_back}"
            );
            assert_eq!(
                field(record, "appId"),
                case.app_id,
                "case {label:?}: record {position} names its app, got {record}"
            );
            assert_eq!(
                field(record, "backendEnvironmentName"),
                case.environment,
                "case {label:?}: record {position} names its environment, got {record}"
            );
            assert_eq!(
                field(record, "status"),
                JOB_STATUS_COMPLETED,
                "case {label:?}: record {position} has settled, got {record}"
            );
        }

        // The operation reported for each record is the operation that recorded it, in
        // the reverse of the call order.
        let expected_operations: Vec<&str> = case
            .steps
            .iter()
            .rev()
            .map(|step| step.operation())
            .collect();
        let reported_operations: Vec<String> = listed(&body)
            .iter()
            .map(|record| field(record, "operation").to_string())
            .collect();
        assert_eq!(
            reported_operations, expected_operations,
            "case {label:?}: expected {expected_operations:?}, got {reported_operations:?}"
        );
    }
}

/// Listing is a REPORT: two listings of the same settled state answer identically, and a
/// listing does not change what a later reader sees.
#[tokio::test]
async fn listing_twice_reports_the_same_records() {
    let router = router();
    let seeded = seed_all(
        &router,
        "repeat",
        "drepeat",
        "staging",
        &[Seed::CreateBackend, Seed::CreateBackendAuth],
    )
    .await;
    settle().await;

    let first = list(&router, "repeat", "drepeat", "staging", json!({})).await;
    let second = list(&router, "repeat", "drepeat", "staging", json!({})).await;
    assert_eq!(
        first, second,
        "case \"repeat\": two listings of settled state must agree, got {first} vs {second}"
    );
    for job_id in &seeded {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/backend/drepeat/job/staging/{job_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case \"repeat\": job {job_id} is still readable after two listings, got {status} {body}"
        );
    }
}

/// `JobId`, `Operation` and `Status` narrow the reported records; a filter that matches
/// nothing answers an EMPTY list, never a refusal and never the unfiltered set.
#[tokio::test]
async fn the_filters_narrow_the_reported_records() {
    let router = router();
    let label = "filters";
    // Two records are seeded and allowed to settle, so a third seeded afterwards is the
    // only one still in progress: the `Status` rows below then span both states.
    let settled = seed_all(
        &router,
        label,
        "dfilter",
        "staging",
        &[Seed::CreateBackend, Seed::CreateBackendAuth],
    )
    .await;
    settle().await;
    let in_progress = seed(&router, label, "dfilter", "staging", 9, Seed::CreateBackend).await;

    let (create_backend_job, create_auth_job) = (settled[0].clone(), settled[1].clone());
    // Newest first: the in-progress record, then the auth record, then the first create.
    let all = vec![
        in_progress.clone(),
        create_auth_job.clone(),
        create_backend_job.clone(),
    ];

    // Each row is a filter document and the job ids it must report, newest first.
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        ("no filter", json!({}), all.clone()),
        (
            "jobId of the oldest record",
            json!({ "jobId": create_backend_job }),
            vec![create_backend_job.clone()],
        ),
        (
            "jobId of the newest record",
            json!({ "jobId": in_progress }),
            vec![in_progress.clone()],
        ),
        (
            "jobId no record carries",
            json!({ "jobId": "0" }),
            Vec::new(),
        ),
        (
            "operation CreateBackend",
            json!({ "operation": OPERATION_CREATE_BACKEND }),
            vec![in_progress.clone(), create_backend_job.clone()],
        ),
        (
            "operation CreateBackendAuth",
            json!({ "operation": OPERATION_CREATE_BACKEND_AUTH }),
            vec![create_auth_job.clone()],
        ),
        (
            "operation no record carries",
            json!({ "operation": OPERATION_DELETE_BACKEND }),
            Vec::new(),
        ),
        (
            "status COMPLETED",
            json!({ "status": JOB_STATUS_COMPLETED }),
            vec![create_auth_job.clone(), create_backend_job.clone()],
        ),
        (
            "status IN_PROGRESS",
            json!({ "status": JOB_STATUS_IN_PROGRESS }),
            vec![in_progress.clone()],
        ),
        (
            "status no record carries",
            json!({ "status": JOB_STATUS_FAILED }),
            Vec::new(),
        ),
        (
            "operation and status together",
            json!({ "operation": OPERATION_CREATE_BACKEND, "status": JOB_STATUS_COMPLETED }),
            vec![create_backend_job.clone()],
        ),
        (
            "a jobId that the status filter excludes",
            json!({ "jobId": in_progress, "status": JOB_STATUS_COMPLETED }),
            Vec::new(),
        ),
    ];

    for (row, request, expected) in rows {
        let body = list(&router, row, "dfilter", "staging", request.clone()).await;
        assert_eq!(
            listed_ids(&body),
            expected,
            "case {row:?} ({request}): expected {expected:?}, got {body}"
        );
    }
}

/// Jobs are addressed within one (app, environment): a listing reports only the records
/// of the environment it names, and only of the app it names.
#[tokio::test]
async fn a_listing_is_scoped_to_its_app_and_environment() {
    let router = router();
    let label = "scope";
    let one = seed_all(&router, label, "dscopea", "one", &[Seed::CreateBackend]).await;
    let two = seed_all(
        &router,
        label,
        "dscopea",
        "two",
        &[Seed::CreateBackend, Seed::CreateBackendAuth],
    )
    .await;
    let other_app = seed_all(&router, label, "dscopeb", "one", &[Seed::CreateBackend]).await;
    settle().await;

    for (row, app_id, environment, expected) in [
        ("app a, environment one", "dscopea", "one", &one),
        ("app a, environment two", "dscopea", "two", &two),
        ("app b, environment one", "dscopeb", "one", &other_app),
    ] {
        let body = list(&router, row, app_id, environment, json!({})).await;
        let mut newest_first = expected.clone();
        newest_first.reverse();
        assert_eq!(
            listed_ids(&body),
            newest_first,
            "case {row:?}: expected only that environment's records, got {body}"
        );
    }

    // An environment name the app has no record for reports an EMPTY list: a job record
    // outlives the environment it ran against, so "no records" and "no environment" are
    // the same observation, and neither is a missing resource.
    let body = list(&router, "unknown environment", "dscopea", "three", json!({})).await;
    assert!(
        listed(&body).is_empty(),
        "case \"unknown environment\": expected no records, got {body}"
    );
}

/// `MaxResults` caps a page and `NextToken` resumes exactly after it: over a span of page
/// sizes, paging to the end reports every record exactly once, newest first.
#[tokio::test]
async fn paging_walks_every_record_exactly_once() {
    let total = 7;
    // The published bounds of `MaxResults` (1 and 25) and sizes between them: a size
    // smaller than the record count, one equal to it, and ones larger.
    for page_size in [MIN_MAX_RESULTS, 2, 3, total as i64, MAX_MAX_RESULTS] {
        let router = router();
        let label = "paging";
        let steps = vec![Seed::CreateBackend; total];
        let seeded = seed_all(&router, label, "dpage", "staging", &steps).await;
        settle().await;

        let mut newest_first = seeded.clone();
        newest_first.reverse();

        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut request = json!({ "maxResults": page_size });
            if let Some(token) = &next_token {
                request
                    .as_object_mut()
                    .expect("the request body is an object")
                    .insert("nextToken".to_string(), json!(token));
            }
            let body = list(&router, label, "dpage", "staging", request).await;
            let page = listed_ids(&body);
            pages += 1;
            assert!(
                pages <= total + 1,
                "case page size {page_size}: paging must terminate, walked {walked:?}"
            );
            let remaining = newest_first.len() - walked.len();
            let expected_len = remaining.min(page_size as usize);
            assert_eq!(
                page.len(),
                expected_len,
                "case page size {page_size}: expected {expected_len} records on page {pages}, got {page:?}"
            );
            walked.extend(page);
            next_token = body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            let more_remain = walked.len() < newest_first.len();
            assert_eq!(
                next_token.is_some(),
                more_remain,
                "case page size {page_size}: a token appears exactly when records remain, got {body}"
            );
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, newest_first,
            "case page size {page_size}: paging reports every record once, newest first"
        );
    }
}

/// A `MaxResults` outside the modelled range is refused before any state is read — one
/// step past each published bound.
#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let router = router();
    let _ = seed_all(&router, "range", "drange", "staging", &[Seed::CreateBackend]).await;

    for value in [MIN_MAX_RESULTS - 1, MAX_MAX_RESULTS + 1] {
        let (status, body) = call(
            &router,
            "POST",
            "/backend/drange/job/staging",
            Some(json!({ "maxResults": value })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case maxResults={value}: expected 400, got {status} {body}"
        );
    }

    for value in [MIN_MAX_RESULTS, MAX_MAX_RESULTS] {
        let (status, body) = call(
            &router,
            "POST",
            "/backend/drange/job/staging",
            Some(json!({ "maxResults": value })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case maxResults={value}: a published bound is accepted, got {status} {body}"
        );
    }
}

/// A `NextToken` this operation did not mint for this (app, environment) is REFUSED with
/// the modelled `BadRequestException`, never served by silently restarting the walk.
#[tokio::test]
async fn a_next_token_it_did_not_issue_is_refused() {
    let router = router();
    let label = "tokens";
    seed_all(
        &router,
        label,
        "dtoka",
        "one",
        &[Seed::CreateBackend, Seed::CreateBackendAuth],
    )
    .await;
    seed_all(
        &router,
        label,
        "dtoka",
        "two",
        &[Seed::CreateBackend, Seed::CreateBackendAuth],
    )
    .await;
    seed_all(
        &router,
        label,
        "dtokb",
        "one",
        &[Seed::CreateBackend, Seed::CreateBackendAuth],
    )
    .await;
    settle().await;

    let body = list(
        &router,
        label,
        "dtoka",
        "one",
        json!({ "maxResults": MIN_MAX_RESULTS }),
    )
    .await;
    let token = body
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !token.is_empty(),
        "case {label:?}: a truncated page mints a token, got {body}"
    );

    // Its own scope resumes.
    let body = list(
        &router,
        "the token's own scope",
        "dtoka",
        "one",
        json!({ "maxResults": MIN_MAX_RESULTS, "nextToken": token }),
    )
    .await;
    assert_eq!(
        listed(&body).len(),
        1,
        "case \"the token's own scope\": the token resumes the walk, got {body}"
    );

    // Every other scope, and every token shape this operation does not mint, is refused.
    let rows: Vec<(&str, &str, &str, String)> = vec![
        ("another environment of the same app", "dtoka", "two", token.clone()),
        ("another app", "dtokb", "one", token.clone()),
        ("a token that is not hex", "dtoka", "one", "not-a-token".to_string()),
        ("hex that is not a token", "dtoka", "one", hex::encode("hello")),
        (
            "a token naming a job id no record carries",
            "dtoka",
            "one",
            hex::encode("v1:0:dtoka:one"),
        ),
    ];
    for (row, app_id, environment, bad) in rows {
        let (status, body) = call(
            &router,
            "POST",
            &format!("/backend/{app_id}/job/{environment}"),
            Some(json!({ "nextToken": bad })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {row:?}: expected 400, got {status} {body}"
        );
        let code = body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            code.ends_with("BadRequestException"),
            "case {row:?}: expected BadRequestException, got {code:?} in {body}"
        );
    }
}

/// Listing the jobs of an app this service holds no app record for is the app-identity
/// refusal — HTTP 400 with the error code `ValidationException`, not a missing resource
/// and not an empty 200. The whole gate, over every app-addressing operation, is
/// `app_registration_gate.rs`.
#[tokio::test]
async fn listing_an_unregistered_app_is_a_validation_exception() {
    let router = router();
    let (status, body) = call(
        &router,
        "POST",
        "/backend/dunknownapp/job/staging",
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"unregistered app\": expected 400, got {status} {body}"
    );
    let code = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        code.ends_with("ValidationException"),
        "case \"unregistered app\": expected ValidationException, got {code:?} in {body}"
    );
    assert_eq!(
        field(&body, "code"),
        "ValidationException",
        "case \"unregistered app\": the body carries the error code, got {body}"
    );
}
