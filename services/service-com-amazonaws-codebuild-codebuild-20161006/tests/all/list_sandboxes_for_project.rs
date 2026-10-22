// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListSandboxesForProject`, driven over the wire the router
//! serves: `POST /` with `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! The rules under test: the operation reports the ids of the sandboxes the named
//! project holds, in the order they were started, `DESCENDING` reversing it, one page
//! of at most `maxResults` entries at a time with a `nextToken` for the rest — and a
//! `projectName` no project of the account answers to is refused with the modelled
//! `ResourceNotFoundException` (HTTP 400) rather than any other error.
//!
//! Each suite in `tests/all/` drives the wire itself, so the harness below is this
//! suite's own.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The access key id every request below is signed with.
const ACCESS_KEY_ID: &str = "AKIDCODEBUILDLISTSBX";

/// One router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::new(engine::InMemoryEngine::new())));
    router
}

/// Call one operation and return its HTTP status and decoded body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("CodeBuild_20161006.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20260101/{REGION}/codebuild/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0f0f"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let decoded = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, decoded)
}

/// Create a minimal NO_SOURCE project.
async fn create_project(router: &MultiServiceRouter, name: &str) {
    let (status, response) = call(
        router,
        "CreateProject",
        json!({
            "name": name,
            "source": {"type": "NO_SOURCE", "buildspec": "version: 0.2"},
            "artifacts": {"type": "NO_ARTIFACTS"},
            "environment": {
                "type": "LINUX_CONTAINER",
                "image": "aws/codebuild/standard:7.0",
                "computeType": "BUILD_GENERAL1_SMALL",
            },
            "serviceRole": "arn:aws:iam::000000000000:role/service-role/codebuild",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateProject {name:?}: expected 200, got {status} with {response}"
    );
}

/// Start a sandbox of `project` and hand back its id.
async fn start_sandbox(router: &MultiServiceRouter, project: &str) -> String {
    let (status, response) = call(router, "StartSandbox", json!({"projectName": project})).await;
    assert_eq!(
        status, 200,
        "StartSandbox {project:?}: expected 200, got {status} with {response}"
    );
    text(&response["sandbox"], "id")
}

/// One `ListSandboxesForProject` call, asserting it succeeded.
async fn list(router: &MultiServiceRouter, body: Value) -> Value {
    let (status, response) = call(router, "ListSandboxesForProject", body.clone()).await;
    assert_eq!(
        status, 200,
        "ListSandboxesForProject {body}: expected 200, got {status} with {response}"
    );
    response
}

/// Every id the operation reports for `project`, walked page by page with
/// `max_results` entries per page, plus the number of pages it took.
async fn walk(
    router: &MultiServiceRouter,
    project: &str,
    sort_order: Option<&str>,
    max_results: Option<i32>,
) -> (Vec<String>, usize) {
    let mut ids = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut body = json!({"projectName": project});
        if let Some(order) = sort_order {
            body["sortOrder"] = json!(order);
        }
        if let Some(limit) = max_results {
            body["maxResults"] = json!(limit);
        }
        if let Some(token) = &next_token {
            body["nextToken"] = json!(token);
        }
        let response = list(router, body).await;
        pages += 1;
        let page: Vec<String> = response["ids"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|id| id.as_str().unwrap_or_default().to_string())
            .collect();
        if let Some(limit) = max_results {
            assert!(
                page.len() <= limit as usize,
                "{project}: a page must hold at most maxResults={limit} entries, \
                 got {} in {response}",
                page.len()
            );
        }
        ids.extend(page);
        next_token = response["nextToken"].as_str().map(str::to_string);
        if next_token.is_none() {
            return (ids, pages);
        }
        assert!(
            pages < 100,
            "{project}: paging did not terminate after {pages} pages"
        );
    }
}

/// The `__type` an error body carries, for asserting the error code a client branches on.
fn error_type(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// A string member, for an assertion message that shows what was actually there.
fn text(value: &Value, member: &str) -> String {
    value[member].as_str().unwrap_or_default().to_string()
}

/// One row of the paging table: the case, the `sortOrder` asked for, the `maxResults`
/// asked for, and the ids expected back in order.
type PagingCase<'a> = (&'a str, Option<&'a str>, Option<i32>, Vec<String>);

/// RULE: the operation reports the ids of the sandboxes the named project holds, in
/// start order (`DESCENDING` reversing it), paged at `maxResults` entries.
///
/// The table spans the counts a project can hold (none, one, several, more than a
/// page's worth of them) against the page sizes the model's `PageSize` allows
/// (`@range(min: 1, max: 100)`): one per page, a few per page, the maximum, and no
/// `maxResults` at all.
#[tokio::test]
async fn list_sandboxes_for_project_reports_each_project_in_start_order() {
    let router = emulator();

    // (project, how many sandboxes it holds)
    let projects: Vec<(&str, usize)> = vec![
        ("list-empty", 0),
        ("list-one", 1),
        ("list-three", 3),
        ("list-seven", 7),
    ];

    for (project, count) in &projects {
        create_project(&router, project).await;
        let mut started_in_order = Vec::new();
        for _ in 0..*count {
            started_in_order.push(start_sandbox(&router, project).await);
        }

        // (case, sortOrder, maxResults, the ids expected in order)
        let mut descending = started_in_order.clone();
        descending.reverse();
        let cases: Vec<PagingCase<'_>> = vec![
            ("no sortOrder, no maxResults", None, None, started_in_order.clone()),
            (
                "ASCENDING is start order",
                Some("ASCENDING"),
                None,
                started_in_order.clone(),
            ),
            (
                "DESCENDING reverses it",
                Some("DESCENDING"),
                None,
                descending.clone(),
            ),
            ("one entry per page", None, Some(1), started_in_order.clone()),
            ("two entries per page", None, Some(2), started_in_order.clone()),
            (
                "the maximum page size",
                None,
                Some(100),
                started_in_order.clone(),
            ),
            (
                "DESCENDING, one entry per page",
                Some("DESCENDING"),
                Some(1),
                descending.clone(),
            ),
        ];

        for (case, sort_order, max_results, expected) in cases {
            let (reported, pages) = walk(&router, project, sort_order, max_results).await;
            assert_eq!(
                reported, expected,
                "{project} / {case}: expected {expected:?}, got {reported:?}"
            );
            let expected_pages = match max_results {
                Some(limit) => expected.len().div_ceil(limit as usize).max(1),
                None => 1,
            };
            assert_eq!(
                pages, expected_pages,
                "{project} / {case}: expected {expected_pages} page(s), got {pages}"
            );
        }
    }

    // A project with no sandboxes answers with an empty list and no token.
    let response = list(&router, json!({"projectName": "list-empty"})).await;
    assert_eq!(
        response["ids"],
        json!([]),
        "a project with no sandboxes reports an empty list, got {response}"
    );
    assert!(
        response["nextToken"].is_null(),
        "a single page carries no nextToken, got {response}"
    );

    // Each project reports only its own sandboxes.
    let seven = walk(&router, "list-seven", None, None).await.0;
    let three = walk(&router, "list-three", None, None).await.0;
    assert!(
        seven.iter().all(|id| id.starts_with("list-seven:"))
            && three.iter().all(|id| id.starts_with("list-three:")),
        "a project must report only its own sandboxes, got {seven:?} and {three:?}"
    );

    // A token that names nothing in the list is a refusable request.
    let (status, body) = call(
        &router,
        "ListSandboxesForProject",
        json!({"projectName": "list-three", "nextToken": "list-three:nope"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "an unknown nextToken: expected 400, got {status} with {body}"
    );
    assert!(
        error_type(&body).contains("InvalidInputException"),
        "an unknown nextToken: expected InvalidInputException, got {body}"
    );
}

/// RULE: a `projectName` no project of the account answers to is a not-found
/// condition, refused with the modelled `ResourceNotFoundException` (HTTP 400) naming
/// the project asked for — never an unmodelled error code.
#[tokio::test]
async fn list_sandboxes_for_project_refuses_a_project_the_account_does_not_hold() {
    let router = emulator();
    create_project(&router, "held-proj").await;
    create_project(&router, "doomed-proj").await;
    let (status, body) = call(&router, "DeleteProject", json!({"name": "doomed-proj"})).await;
    assert_eq!(
        status, 200,
        "DeleteProject doomed-proj: expected 200, got {status} with {body}"
    );

    // (case, the projectName asked for)
    let cases: Vec<(&str, &str)> = vec![
        ("a name never created", "never-created"),
        ("a project just deleted", "doomed-proj"),
        ("a held name in the wrong case", "HELD-PROJ"),
        ("a name that is a prefix of a held one", "held"),
        ("a project ARN rather than a name", "arn:aws:codebuild:us-west-2:000000000000:project/held-proj"),
    ];

    for (case, project_name) in cases {
        let (status, body) = call(
            &router,
            "ListSandboxesForProject",
            json!({"projectName": project_name}),
        )
        .await;
        assert_eq!(status, 400, "{case}: expected 400, got {status} with {body}");
        assert!(
            error_type(&body).contains("ResourceNotFoundException"),
            "{case}: expected ResourceNotFoundException, got {body}"
        );
        assert!(
            text(&body, "message").contains(project_name),
            "{case}: the refusal must name the project asked for, got {body}"
        );
    }

    // The held project itself is served, so the refusal is about the project and not
    // about the operation.
    let response = list(&router, json!({"projectName": "held-proj"})).await;
    assert_eq!(
        response["ids"],
        json!([]),
        "a held project with no sandboxes is served, got {response}"
    );
}
