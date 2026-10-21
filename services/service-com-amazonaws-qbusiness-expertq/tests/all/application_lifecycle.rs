// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Amazon Q Business application + index lifecycle.
//!
//! Every case drives the service the way a client does — one HTTP request per call,
//! through the same `MultiServiceRouter` the emulator boots — and asserts only what the
//! stored state can justify: an id the service minted, a record read back through
//! `GetApplication`/`GetIndex`, a page that resumes where the previous one ended, and a
//! 404 for an id nothing was ever created under.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A region that is NOT the emulator default, so an ARN built from the request's own
/// SigV4 credential scope is distinguishable from a hardcoded one.
const REGION: &str = "eu-west-2";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// Longer than `crate::state::PROVISIONING_SPAN`, so the `CREATING` -> `ACTIVE`
/// transition the clock owes has come due.
const PAST_PROVISIONING: Duration = Duration::from_millis(800);

/// The provisioning span the service itself uses, so the transient-status assertion below
/// is tied to the implementation's own constant rather than to a copy of it.
const PROVISIONING_SPAN: Duration = service_com_amazonaws_qbusiness_expertq::state::PROVISIONING_SPAN;

/// What one call answered: the status, the modelled error shape (empty on success) and
/// the parsed body.
#[derive(Debug)]
struct Answer {
    status: u16,
    error_type: String,
    body: Value,
}

impl Answer {
    fn field(&self, name: &str) -> &Value {
        &self.body[name]
    }

    fn text(&self, name: &str) -> String {
        self.body[name]
            .as_str()
            .unwrap_or_else(|| panic!("expected a string `{name}` in {:?}", self.body))
            .to_string()
    }
}

fn new_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's own concern and is exercised elsewhere;
    // these cases are about this service's behaviour.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_qbusiness_expertq::EmulatedService::new(engine),
    );
    router
}

async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Answer {
    let payload = match body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://qbusiness.{REGION}.amazonaws.com{path}"))
        // The credential scope is how a handler learns the region its ARNs name.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/qbusiness/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(['/', ':'])
                .next()
                .unwrap_or(value)
                .trim()
                .to_string()
        })
        .unwrap_or_default();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        body,
    }
}

/// `ApplicationId` and `IndexId` are both `@length(36, 36)` with pattern
/// `^[a-zA-Z0-9][a-zA-Z0-9-]{35}$` (smithy_query: shape ApplicationId / IndexId).
fn is_resource_id(value: &str) -> bool {
    value.len() == 36
        && value
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

async fn create_application(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "POST", "/applications", Some(body)).await
}

async fn created_application_id(router: &MultiServiceRouter, display_name: &str) -> String {
    let created = create_application(router, json!({ "displayName": display_name })).await;
    assert_eq!(
        created.status, 200,
        "case {display_name}: expected 200, got {} / {:?}",
        created.status, created.body
    );
    created.text("applicationId")
}

/// A resource read within `elapsed` of its own create must still report `CREATING`; only
/// a read that took longer than the provisioning span may already see it settled.
fn assert_transient_status(case: &str, read: &Answer, elapsed: Duration) {
    let status = read.text("status");
    if status != "CREATING" {
        assert!(
            elapsed >= PROVISIONING_SPAN,
            "case {case:?}: a just-created resource is not ready yet — expected CREATING \
             within the {PROVISIONING_SPAN:?} provisioning span, got {status:?} after {elapsed:?}"
        );
    }
}

/// A create must be observable through a read, with every member it was given — across
/// inputs that differ in which optional members are present, in identity type, and in the
/// configuration structs carried along.
#[tokio::test]
async fn created_applications_read_back_exactly_as_created() {
    // `identityType` values are the full `IdentityType` enum (smithy_query: shape
    // IdentityType) — ANONYMOUS and AWS_QUICKSIGHT_IDP are not cited by the issue.
    let cases = vec![
        ("minimal", json!({ "displayName": "demo-app" })),
        (
            "described",
            json!({
                "displayName": "finance_reports-2026",
                "description": "all of it: \n punctuation, spaces and ümlauts",
                "identityType": "AWS_IAM_IDC",
            }),
        ),
        (
            "anonymous-with-configs",
            json!({
                "displayName": "A",
                "identityType": "ANONYMOUS",
                "attachmentsConfiguration": { "attachmentsControlMode": "ENABLED" },
                "qAppsConfiguration": { "qAppsControlMode": "DISABLED" },
                "personalizationConfiguration": { "personalizationControlMode": "ENABLED" },
                "tags": [{ "key": "team", "value": "search" }, { "key": "cost", "value": "" }],
            }),
        ),
        (
            "quicksight-identity",
            json!({
                "displayName": "q-0123456789",
                "identityType": "AWS_QUICKSIGHT_IDP",
                "quickSightConfiguration": { "clientNamespace": "default" },
                "roleArn": "arn:aws:iam::000000000000:role/qbusiness-app",
            }),
        ),
    ];

    let router = new_router();
    for (case, request) in cases {
        let created = create_application(&router, request.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateApplication, got {} / {:?}",
            created.status, created.body
        );

        let application_id = created.text("applicationId");
        assert!(
            is_resource_id(&application_id),
            "case {case:?}: minted applicationId must be a 36-char ApplicationId, got {application_id:?}"
        );
        assert_eq!(
            created.text("applicationArn"),
            format!("arn:aws:qbusiness:{REGION}:{ACCOUNT}:application/{application_id}"),
            "case {case:?}: applicationArn must name the minted id in the signed region, got {:?}",
            created.field("applicationArn")
        );

        let read = call(
            &router,
            "GET",
            &format!("/applications/{application_id}"),
            None,
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetApplication, got {} / {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.text("applicationId"),
            application_id,
            "case {case:?}: GetApplication returned another application, got {:?}",
            read.field("applicationId")
        );
        assert_eq!(
            read.text("applicationArn"),
            created.text("applicationArn"),
            "case {case:?}: the stored ARN changed between create and read, got {:?}",
            read.field("applicationArn")
        );
        assert_eq!(
            read.text("displayName"),
            request["displayName"].as_str().unwrap(),
            "case {case:?}: displayName did not round-trip, got {:?}",
            read.field("displayName")
        );
        for member in [
            "description",
            "identityType",
            "roleArn",
            "qAppsConfiguration",
            "personalizationConfiguration",
            "quickSightConfiguration",
        ] {
            assert_eq!(
                read.field(member),
                request.get(member).unwrap_or(&Value::Null),
                "case {case:?}: {member} did not round-trip, got {:?}",
                read.field(member)
            );
        }
        // The output models the APPLIED attachments configuration, which carries the
        // control mode the create was given.
        assert_eq!(
            read.field("attachmentsConfiguration"),
            request
                .get("attachmentsConfiguration")
                .unwrap_or(&Value::Null),
            "case {case:?}: attachmentsConfiguration did not round-trip, got {:?}",
            read.field("attachmentsConfiguration")
        );
        assert!(
            read.field("createdAt").as_f64().is_some_and(|at| at > 0.0),
            "case {case:?}: createdAt must be a populated timestamp, got {:?}",
            read.field("createdAt")
        );
    }
}

/// Two creates mint two different applications; a replay of one `clientToken` mints
/// nothing new (`clientToken` is an `@idempotencyToken`).
#[tokio::test]
async fn client_token_replays_instead_of_minting_a_second_application() {
    let router = new_router();
    let cases = ["token-a", "0", &"t".repeat(100)];

    let mut minted = Vec::new();
    for case in cases {
        let first = create_application(
            &router,
            json!({ "displayName": "replayed", "clientToken": case }),
        )
        .await;
        let second = create_application(
            &router,
            json!({ "displayName": "replayed", "clientToken": case }),
        )
        .await;
        assert_eq!(
            first.text("applicationId"),
            second.text("applicationId"),
            "case {case:?}: a replayed clientToken must return the first applicationId, got {:?}",
            second.field("applicationId")
        );
        minted.push(first.text("applicationId"));
    }

    let listed = call(&router, "GET", "/applications", None).await;
    let applications = listed.field("applications").as_array().cloned().unwrap();
    assert_eq!(
        applications.len(),
        minted.len(),
        "expected one application per distinct clientToken ({}), got {:?}",
        minted.len(),
        applications
    );
}

/// A resource the service reports as `CREATING` becomes `ACTIVE` because the clock made
/// the transition due — not because anybody read it.
#[tokio::test]
async fn application_and_index_settle_from_creating_to_active() {
    let router = new_router();

    // Each transient read is taken immediately after its OWN create, so it has the whole
    // provisioning span to land in; a read that still lands after the span has elapsed
    // (a badly loaded machine) may see the settled status instead of failing on a
    // wall-clock race it does not control.
    let started = std::time::Instant::now();
    let application_id = created_application_id(&router, "settling").await;
    let application_path = format!("/applications/{application_id}");
    let read = call(&router, "GET", &application_path, None).await;
    assert_transient_status("application", &read, started.elapsed());

    let started = std::time::Instant::now();
    let created_index = call(
        &router,
        "POST",
        &format!("/applications/{application_id}/indices"),
        Some(json!({ "displayName": "settling-index", "type": "STARTER" })),
    )
    .await;
    assert_eq!(
        created_index.status, 200,
        "expected 200 from CreateIndex, got {} / {:?}",
        created_index.status, created_index.body
    );
    let index_id = created_index.text("indexId");
    let index_path = format!("/applications/{application_id}/indices/{index_id}");
    let read = call(&router, "GET", &index_path, None).await;
    assert_transient_status("index", &read, started.elapsed());

    tokio::time::sleep(PAST_PROVISIONING).await;

    for (case, path) in [("application", &application_path), ("index", &index_path)] {
        let read = call(&router, "GET", path, None).await;
        assert_eq!(
            read.text("status"),
            "ACTIVE",
            "case {case:?}: the transition the clock owed was never materialised, got {:?}",
            read.field("status")
        );
        assert!(
            read.field("updatedAt").as_f64() >= read.field("createdAt").as_f64(),
            "case {case:?}: updatedAt must not precede createdAt, got {:?} / {:?}",
            read.field("updatedAt"),
            read.field("createdAt")
        );
    }
}

/// An index's `clientToken` is scoped to the application it creates the index under: a
/// replay under the same application replays, and the same token under another
/// application is a different create.
#[tokio::test]
async fn index_client_tokens_are_scoped_to_their_application() {
    let router = new_router();
    let first = created_application_id(&router, "scope-one").await;
    let second = created_application_id(&router, "scope-two").await;

    let create = |application_id: String, token: &'static str| {
        let router = &router;
        async move {
            call(
                router,
                "POST",
                &format!("/applications/{application_id}/indices"),
                Some(json!({ "displayName": "scoped", "clientToken": token })),
            )
            .await
            .text("indexId")
        }
    };

    let under_first = create(first.clone(), "shared-token").await;
    let under_second = create(second.clone(), "shared-token").await;
    assert_ne!(
        under_first, under_second,
        "case \"two applications, one token\": each application must get its own index, got {under_first} twice"
    );

    let replayed_first = create(first.clone(), "shared-token").await;
    assert_eq!(
        replayed_first, under_first,
        "case \"replay under the first application\": expected {under_first}, got {replayed_first}"
    );
    let replayed_second = create(second.clone(), "shared-token").await;
    assert_eq!(
        replayed_second, under_second,
        "case \"replay under the second application\": expected {under_second}, got {replayed_second}"
    );

    for (case, application_id) in [("first", first), ("second", second)] {
        let listed = call(
            &router,
            "GET",
            &format!("/applications/{application_id}/indices"),
            None,
        )
        .await;
        assert_eq!(
            listed.field("indices").as_array().map(Vec::len),
            Some(1),
            "case {case:?}: a replayed clientToken must not mint a second index, got {:?}",
            listed.field("indices")
        );
    }
}

/// A listing returns every stored application exactly once, in creation order, for any
/// page size — and the cursor resumes exactly where the previous page ended.
#[tokio::test]
async fn list_applications_pages_cover_every_application_once() {
    let router = new_router();
    let mut expected = Vec::new();
    for ordinal in 0..5 {
        expected.push(created_application_id(&router, &format!("paged-{ordinal}")).await);
    }

    // Page sizes spanning the `@range(1, 100)` on MaxResultsIntegerForListApplications:
    // both published bounds, a size that divides the set, one that does not, and none.
    for case in [None, Some(1), Some(2), Some(5), Some(100)] {
        let mut seen = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = Vec::new();
            if let Some(max_results) = case {
                query.push(format!("maxResults={max_results}"));
            }
            if let Some(token) = &next_token {
                query.push(format!("nextToken={token}"));
            }
            let path = if query.is_empty() {
                "/applications".to_string()
            } else {
                format!("/applications?{}", query.join("&"))
            };
            let page = call(&router, "GET", &path, None).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: expected 200 from ListApplications, got {} / {:?}",
                page.status, page.body
            );
            let items = page
                .field("applications")
                .as_array()
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: no applications array in {:?}", page.body));
            if let Some(limit) = case {
                assert!(
                    items.len() as i32 <= limit,
                    "case {case:?}: a page exceeded maxResults, got {} items",
                    items.len()
                );
            }
            for item in items {
                assert!(
                    item["status"].is_string() && item["createdAt"].as_f64().is_some(),
                    "case {case:?}: a summary is missing status/createdAt, got {item:?}"
                );
                seen.push(item["applicationId"].as_str().unwrap().to_string());
            }
            pages += 1;
            next_token = page
                .field("nextToken")
                .as_str()
                .map(|token| token.to_string());
            if next_token.is_none() {
                break;
            }
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: the pages must cover every application once, in creation order"
        );
    }
}

/// A `nextToken` no listing of this service minted is refused as a ValidationException,
/// never served as a silent first page.
#[tokio::test]
async fn a_cursor_this_service_never_minted_is_refused() {
    let router = new_router();
    let application_id = created_application_id(&router, "cursors").await;

    // Each row is a token that cannot have come from this service: not hex, hex of text
    // that is not a cursor, and hex of a cursor with a non-numeric position.
    let tokens = [
        "not-a-cursor",
        "6e6f742d612d637572736f72",
        "61667465722d73657175656e63653a78",
    ];
    let paths = [
        "/applications".to_string(),
        format!("/applications/{application_id}/indices"),
    ];

    for path in &paths {
        for token in tokens {
            let refused = call(&router, "GET", &format!("{path}?nextToken={token}"), None).await;
            assert_eq!(
                refused.status, 400,
                "case {token:?} on {path}: expected 400, got {} / {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_type, "ValidationException",
                "case {token:?} on {path}: expected a ValidationException, got {:?}",
                refused.error_type
            );
            assert_eq!(
                refused.field("reason"),
                &json!("CANNOT_PARSE"),
                "case {token:?} on {path}: expected the CANNOT_PARSE reason, got {:?}",
                refused.field("reason")
            );
        }
    }
}

/// An index is created under one application, read back with what it was created with,
/// and listed only under its own application.
#[tokio::test]
async fn indices_round_trip_under_their_own_application() {
    let router = new_router();
    let application_id = created_application_id(&router, "indexed").await;
    let other_application_id = created_application_id(&router, "not-indexed").await;

    // `type` spans the whole `IndexType` enum (smithy_query: shape IndexType ->
    // ENTERPRISE | STARTER) plus the absent case, since the member is optional.
    let cases = vec![
        ("starter", json!({ "displayName": "starter-index", "type": "STARTER" })),
        (
            "enterprise-with-capacity",
            json!({
                "displayName": "enterprise_index-1",
                "type": "ENTERPRISE",
                "description": "the big one",
                "capacityConfiguration": { "units": 3 },
                "tags": [{ "key": "tier", "value": "enterprise" }],
            }),
        ),
        ("untyped", json!({ "displayName": "I" })),
    ];

    let mut created_ids = Vec::new();
    for (case, request) in cases {
        let created = call(
            &router,
            "POST",
            &format!("/applications/{application_id}/indices"),
            Some(request.clone()),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateIndex, got {} / {:?}",
            created.status, created.body
        );
        let index_id = created.text("indexId");
        assert!(
            is_resource_id(&index_id),
            "case {case:?}: minted indexId must be a 36-char IndexId, got {index_id:?}"
        );
        assert_eq!(
            created.text("indexArn"),
            format!(
                "arn:aws:qbusiness:{REGION}:{ACCOUNT}:application/{application_id}/index/{index_id}"
            ),
            "case {case:?}: indexArn must nest the index under its application, got {:?}",
            created.field("indexArn")
        );

        let read = call(
            &router,
            "GET",
            &format!("/applications/{application_id}/indices/{index_id}"),
            None,
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetIndex, got {} / {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.text("applicationId"),
            application_id,
            "case {case:?}: GetIndex must report the owning application, got {:?}",
            read.field("applicationId")
        );
        for member in ["displayName", "type", "description", "capacityConfiguration"] {
            assert_eq!(
                read.field(member),
                request.get(member).unwrap_or(&Value::Null),
                "case {case:?}: {member} did not round-trip, got {:?}",
                read.field(member)
            );
        }
        created_ids.push(index_id);
    }

    let listed = call(
        &router,
        "GET",
        &format!("/applications/{application_id}/indices"),
        None,
    )
    .await;
    let listed_ids: Vec<String> = listed
        .field("indices")
        .as_array()
        .cloned()
        .unwrap()
        .iter()
        .map(|item| item["indexId"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        listed_ids, created_ids,
        "ListIndices must return this application's indices in creation order"
    );

    let other = call(
        &router,
        "GET",
        &format!("/applications/{other_application_id}/indices"),
        None,
    )
    .await;
    assert_eq!(
        other.field("indices"),
        &json!([]),
        "an application with no index of its own must list none, got {:?}",
        other.field("indices")
    );

    // An index id is scoped to its application: it is not found under another one.
    let crossed = call(
        &router,
        "GET",
        &format!(
            "/applications/{other_application_id}/indices/{}",
            created_ids[0]
        ),
        None,
    )
    .await;
    assert_eq!(
        crossed.status, 404,
        "an index must not be readable under another application, got {} / {:?}",
        crossed.status, crossed.body
    );
}

/// Deleting an application removes it and the indices stored under it, and every later
/// read of them answers 404.
#[tokio::test]
async fn deleting_an_application_removes_it_and_its_indices() {
    let router = new_router();
    let doomed = created_application_id(&router, "doomed").await;
    let survivor = created_application_id(&router, "survivor").await;
    let index_id = call(
        &router,
        "POST",
        &format!("/applications/{doomed}/indices"),
        Some(json!({ "displayName": "doomed-index" })),
    )
    .await
    .text("indexId");

    let deleted = call(&router, "DELETE", &format!("/applications/{doomed}"), None).await;
    assert_eq!(
        deleted.status, 200,
        "expected 200 from DeleteApplication, got {} / {:?}",
        deleted.status, deleted.body
    );

    for (case, path) in [
        ("GetApplication", format!("/applications/{doomed}")),
        (
            "GetIndex",
            format!("/applications/{doomed}/indices/{index_id}"),
        ),
        ("ListIndices", format!("/applications/{doomed}/indices")),
    ] {
        let gone = call(&router, "GET", &path, None).await;
        assert_eq!(
            gone.status, 404,
            "case {case:?}: a deleted application must be gone, got {} / {:?}",
            gone.status, gone.body
        );
    }

    let repeated = call(&router, "DELETE", &format!("/applications/{doomed}"), None).await;
    assert_eq!(
        repeated.status, 404,
        "case {:?}: deleting an application twice must answer ResourceNotFoundException, got {} / {:?}",
        "DeleteApplication-again", repeated.status, repeated.body
    );

    let listed = call(&router, "GET", "/applications", None).await;
    let remaining: Vec<String> = listed
        .field("applications")
        .as_array()
        .cloned()
        .unwrap()
        .iter()
        .map(|item| item["applicationId"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        remaining,
        vec![survivor],
        "ListApplications must observe the delete"
    );
}

/// Every operation that resolves a resource by id answers `ResourceNotFoundException`
/// (HTTP 404) for an id nothing was created under — naming the id and its type.
#[tokio::test]
async fn unknown_ids_answer_resource_not_found() {
    let router = new_router();
    let application_id = created_application_id(&router, "known").await;

    // Well-formed `ApplicationId`s (36 chars, matching the pattern) that name nothing:
    // all digits, all letters, and a hyphenated shape.
    let unknown_ids = [
        "0".repeat(36),
        "a".repeat(36),
        "ffffffff-ffff-4fff-bfff-ffffffffffff".to_string(),
    ];

    for unknown in &unknown_ids {
        let cases: Vec<(&str, &str, String, Option<Value>)> = vec![
            (
                "GetApplication",
                "GET",
                format!("/applications/{unknown}"),
                None,
            ),
            (
                "DeleteApplication",
                "DELETE",
                format!("/applications/{unknown}"),
                None,
            ),
            (
                "CreateIndex",
                "POST",
                format!("/applications/{unknown}/indices"),
                Some(json!({ "displayName": "x" })),
            ),
            (
                "ListIndices",
                "GET",
                format!("/applications/{unknown}/indices"),
                None,
            ),
            (
                "GetIndex",
                "GET",
                format!("/applications/{unknown}/indices/{unknown}"),
                None,
            ),
            (
                "GetIndex-unknown-index",
                "GET",
                format!("/applications/{application_id}/indices/{unknown}"),
                None,
            ),
        ];

        for (case, method, path, body) in cases {
            let answer = call(&router, method, &path, body).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?} with id {unknown:?}: expected 404, got {} / {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_type, "ResourceNotFoundException",
                "case {case:?} with id {unknown:?}: expected a ResourceNotFoundException, got {:?}",
                answer.error_type
            );
            assert_eq!(
                answer.field("resourceId"),
                &json!(unknown),
                "case {case:?} with id {unknown:?}: the error must name the id, got {:?}",
                answer.field("resourceId")
            );
            assert!(
                matches!(
                    answer.text("resourceType").as_str(),
                    "APPLICATION" | "INDEX"
                ),
                "case {case:?} with id {unknown:?}: the error must name the resource type, got {:?}",
                answer.field("resourceType")
            );
        }
    }
}

/// Neither create grows the store without end: once the emulator's ceiling for a resource
/// is reached, the next create is refused with the `ServiceQuotaExceededException` the
/// model declares for exactly that (HTTP 402), naming the resource type that ran out.
#[tokio::test]
async fn a_create_past_the_ceiling_is_refused_with_the_modelled_quota_error() {
    use service_com_amazonaws_qbusiness_expertq::state::{
        MAX_APPLICATIONS, MAX_INDICES_PER_APPLICATION,
    };

    let router = new_router();
    let application_id = created_application_id(&router, "ceiling").await;

    // Fill the index ceiling of one application, then the application ceiling of the
    // account; each row below asserts the refusal at its own site.
    for ordinal in 0..MAX_INDICES_PER_APPLICATION {
        let created = call(
            &router,
            "POST",
            &format!("/applications/{application_id}/indices"),
            Some(json!({ "displayName": format!("index-{ordinal}") })),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case \"index {ordinal}\": a create below the ceiling must succeed, got {} / {:?}",
            created.status, created.body
        );
    }
    for ordinal in 1..MAX_APPLICATIONS {
        let created =
            create_application(&router, json!({ "displayName": format!("app-{ordinal}") })).await;
        assert_eq!(
            created.status, 200,
            "case \"application {ordinal}\": a create below the ceiling must succeed, got {} / {:?}",
            created.status, created.body
        );
    }

    let cases: Vec<(&str, String, Value, &str)> = vec![
        (
            "CreateIndex",
            format!("/applications/{application_id}/indices"),
            json!({ "displayName": "one-too-many" }),
            "INDEX",
        ),
        (
            "CreateApplication",
            "/applications".to_string(),
            json!({ "displayName": "one-too-many" }),
            "APPLICATION",
        ),
    ];

    for (case, path, body, resource_type) in cases {
        let refused = call(&router, "POST", &path, Some(body)).await;
        assert_eq!(
            refused.status, 402,
            "case {case:?}: expected 402 past the ceiling, got {} / {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, "ServiceQuotaExceededException",
            "case {case:?}: expected a ServiceQuotaExceededException, got {:?}",
            refused.error_type
        );
        assert_eq!(
            refused.field("resourceType"),
            &json!(resource_type),
            "case {case:?}: the error must name what ran out, got {:?}",
            refused.field("resourceType")
        );
    }
}
