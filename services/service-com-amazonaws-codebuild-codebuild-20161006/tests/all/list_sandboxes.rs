// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for CodeBuild's `ListSandboxes`, driven over the wire the router
//! serves: `POST /` with `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! The rule under test: `ListSandboxes` reports the id of EVERY sandbox the account
//! holds, whatever project each belongs to, in the order they were requested — newest
//! first unless `sortOrder` asks for `ASCENDING` — in pages of at most `maxResults`
//! entries, each page after the first opened by the `nextToken` the previous one
//! handed back.
//!
//! Each suite in `tests/all/` drives the wire itself, so the harness below (the router,
//! `call`, the project body) is this suite's own — the sibling suites' copies are
//! private to them.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The access key id every request below is signed with.
const ACCESS_KEY_ID: &str = "AKIDCODEBUILDLISTSANDBOXES";

/// The bounds `ListSandboxesInput$maxResults` is held to, from the model: its target
/// `PageSize` is `@range(min: 1, max: 100)`
/// (`smithy_query(query="shape",shape="PageSize")`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// One router carrying only this service, over one shared engine — the same assembly
/// `core/router` builds, minus the other services.
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

/// The `__type` an error body carries, for asserting the error code a client branches on.
fn error_type(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// A minimal valid `CreateProject` request for `name`.
fn create_project_body(name: &str) -> Value {
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
    })
}

/// Create `name` unless it is already there, asserting it succeeded.
async fn ensure_project(router: &MultiServiceRouter, name: &str) {
    let (status, body) = call(router, "CreateProject", create_project_body(name)).await;
    assert!(
        status == 200 || error_type(&body).contains("ResourceAlreadyExistsException"),
        "CreateProject {name:?}: expected 200 or an already-exists refusal, got {status} \
         with {body}"
    );
}

/// Start one sandbox of `project_name` and hand back its id.
async fn start_sandbox(router: &MultiServiceRouter, project_name: &str) -> String {
    let (status, body) = call(
        router,
        "StartSandbox",
        json!({"projectName": project_name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "StartSandbox {project_name:?}: expected 200, got {status} with {body}"
    );
    body["sandbox"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("StartSandbox {project_name:?} must report an id, got {body}"))
        .to_string()
}

/// Start one sandbox per entry of `projects`, in order, and hand back the ids in that
/// same request order (oldest first).
async fn start_sandboxes(router: &MultiServiceRouter, projects: &[&str]) -> Vec<String> {
    let mut ids = Vec::new();
    for project_name in projects {
        ensure_project(router, project_name).await;
        ids.push(start_sandbox(router, project_name).await);
    }
    ids
}

/// `ListSandboxes` with `body`, asserting it succeeded, and hand back its `ids` and
/// `nextToken`.
async fn list_sandboxes(router: &MultiServiceRouter, body: Value) -> (Vec<String>, Option<String>) {
    let (status, response) = call(router, "ListSandboxes", body.clone()).await;
    assert_eq!(
        status, 200,
        "ListSandboxes {body}: expected 200, got {status} with {response}"
    );
    let ids = response["ids"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("ListSandboxes {body}: `ids` must be a list, got {response}")
        })
        .iter()
        .map(|id| {
            id.as_str()
                .unwrap_or_else(|| panic!("ListSandboxes {body}: an id must be a string, got {response}"))
                .to_string()
        })
        .collect();
    let next_token = response["nextToken"].as_str().map(str::to_string);
    (ids, next_token)
}

/// RULE: the ids of every sandbox of the account, in request order — newest first
/// unless `sortOrder` is `ASCENDING`.
///
/// The table spans the count (none, one, several), how the sandboxes are spread over
/// projects (one project, several, interleaved) and every spelling of `sortOrder`
/// (absent, `DESCENDING`, `ASCENDING` — the two values
/// `smithy_query(query="shape",shape="SortOrderType")` lists).
#[tokio::test]
async fn list_sandboxes_reports_every_sandbox_id_in_request_order() {
    // (case, the project each sandbox is started for in order, sortOrder)
    let cases: Vec<(&str, Vec<&str>, Option<&str>)> = vec![
        ("an account holding no sandbox", vec![], None),
        ("one sandbox, no sortOrder", vec!["list-one"], None),
        (
            "three sandboxes of one project, no sortOrder",
            vec!["list-three", "list-three", "list-three"],
            None,
        ),
        (
            "three sandboxes of one project, ascending",
            vec!["list-three", "list-three", "list-three"],
            Some("ASCENDING"),
        ),
        (
            "five sandboxes interleaved over three projects, descending",
            vec!["list-a", "list-b", "list-a", "list-c", "list-b"],
            Some("DESCENDING"),
        ),
        (
            "five sandboxes interleaved over three projects, ascending",
            vec!["list-a", "list-b", "list-a", "list-c", "list-b"],
            Some("ASCENDING"),
        ),
        (
            "no sandbox, ascending",
            vec![],
            Some("ASCENDING"),
        ),
    ];

    for (case, projects, sort_order) in cases {
        let router = emulator();
        let started = start_sandboxes(&router, &projects).await;

        let mut expected = started.clone();
        if sort_order != Some("ASCENDING") {
            expected.reverse();
        }

        let mut request = json!({});
        if let Some(order) = sort_order {
            request["sortOrder"] = json!(order);
        }
        let (ids, next_token) = list_sandboxes(&router, request).await;

        assert_eq!(
            ids, expected,
            "case {case:?}: expected the ids {expected:?}, got {ids:?}"
        );
        assert_eq!(
            next_token, None,
            "case {case:?}: a page holding every sandbox must carry no nextToken, got \
             {next_token:?}"
        );
    }
}

/// RULE: a page carries at most `maxResults` ids, and `nextToken` opens the page that
/// starts at the next id — so walking the tokens reassembles the whole list exactly
/// once, whatever the page size.
///
/// The table spans page sizes smaller than, equal to and larger than the number of
/// sandboxes held, including both bounds the model publishes for `maxResults`
/// (`PageSize` is `@range(min: 1, max: 100)`), and absent (the service's own default).
#[tokio::test]
async fn list_sandboxes_pages_at_max_results() {
    const HELD: usize = 7;

    let page_sizes: Vec<Option<i64>> = vec![
        None,
        Some(MAX_RESULTS_MIN),
        Some(2),
        Some(3),
        Some(HELD as i64),
        Some(MAX_RESULTS_MAX),
    ];

    for max_results in page_sizes {
        let case = format!("maxResults {max_results:?} over {HELD} sandboxes");
        let router = emulator();
        let projects: Vec<&str> = (0..HELD)
            .map(|index| if index % 2 == 0 { "page-even" } else { "page-odd" })
            .collect();
        let started = start_sandboxes(&router, &projects).await;
        let mut expected = started.clone();
        expected.reverse();

        // The page size the service applies when the caller names none is its own
        // ceiling, which is the model's `maxResults` maximum.
        let page_size = max_results.unwrap_or(MAX_RESULTS_MAX) as usize;

        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({});
            if let Some(size) = max_results {
                request["maxResults"] = json!(size);
            }
            if let Some(ref resume) = token {
                request["nextToken"] = json!(resume);
            }
            let (ids, next_token) = list_sandboxes(&router, request).await;
            pages += 1;
            assert!(
                ids.len() <= page_size,
                "case {case:?}: page {pages} must hold at most {page_size} ids, got {ids:?}"
            );
            if let Some(ref next) = next_token {
                assert_eq!(
                    ids.len(),
                    page_size,
                    "case {case:?}: page {pages} is not the last, so it must be full, got {ids:?}"
                );
                assert_eq!(
                    next,
                    &expected[walked.len() + ids.len()],
                    "case {case:?}: page {pages}'s nextToken must name the next id, got {next:?}"
                );
            }
            walked.extend(ids);
            match next_token {
                Some(next) => token = Some(next),
                None => break,
            }
            assert!(
                pages <= HELD + 1,
                "case {case:?}: paging must terminate, walked {walked:?}"
            );
        }

        assert_eq!(
            walked, expected,
            "case {case:?}: walking the pages must yield every id once, in order; expected \
             {expected:?}, got {walked:?}"
        );
        let expected_pages = HELD.div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} page(s), got {pages}"
        );
    }
}

/// RULE: `maxResults` outside the range the model publishes for it is refused, and
/// each published bound is accepted.
#[tokio::test]
async fn list_sandboxes_holds_max_results_to_the_modelled_range() {
    let router = emulator();
    let started = start_sandboxes(&router, &["range-one", "range-two", "range-one"]).await;

    // (case, the maxResults asked for, how many ids the answer may carry)
    let accepted: Vec<(&str, i64, usize)> = vec![
        ("the published minimum", MAX_RESULTS_MIN, 1),
        ("the published maximum", MAX_RESULTS_MAX, started.len()),
    ];
    for (case, max_results, expected_len) in accepted {
        let (ids, _) = list_sandboxes(&router, json!({"maxResults": max_results})).await;
        assert_eq!(
            ids.len(),
            expected_len,
            "case {case:?} (maxResults {max_results}): expected {expected_len} id(s), got {ids:?}"
        );
    }

    let refused: Vec<(&str, i64)> = vec![
        ("one below the published minimum", MAX_RESULTS_MIN - 1),
        ("one past the published maximum", MAX_RESULTS_MAX + 1),
    ];
    for (case, max_results) in refused {
        let (status, body) = call(&router, "ListSandboxes", json!({"maxResults": max_results})).await;
        assert_eq!(
            status, 400,
            "case {case:?} (maxResults {max_results}): expected 400, got {status} with {body}"
        );
    }
}

/// RULE: a `nextToken` that names no sandbox of the list is a refusable request, not an
/// empty page.
#[tokio::test]
async fn list_sandboxes_refuses_a_next_token_naming_nothing() {
    let router = emulator();
    let started = start_sandboxes(&router, &["token-one", "token-two"]).await;

    // (case, the token sent)
    let cases: Vec<(&str, String)> = vec![
        ("a token that is not an id at all", "not-a-sandbox".to_string()),
        (
            "an id of a sandbox that was never started",
            format!("{}-nope", started[0]),
        ),
        ("an empty token", String::new()),
    ];
    for (case, token) in cases {
        let (status, body) = call(
            &router,
            "ListSandboxes",
            json!({"nextToken": token.clone()}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?} (nextToken {token:?}): expected 400, got {status} with {body}"
        );
        assert!(
            error_type(&body).contains("InvalidInputException"),
            "case {case:?} (nextToken {token:?}): expected InvalidInputException, got {body}"
        );
    }

    // The ids that ARE in the list resume paging rather than being refused.
    for id in &started {
        let (ids, _) = list_sandboxes(&router, json!({"nextToken": id})).await;
        assert_eq!(
            ids.first().map(String::as_str),
            Some(id.as_str()),
            "a token naming a held sandbox must open the page at it: expected {id:?} first, \
             got {ids:?}"
        );
    }
}

/// RULE: listing is a read — it reports what the account holds without changing what a
/// later caller sees.
#[tokio::test]
async fn list_sandboxes_leaves_the_list_unchanged() {
    let router = emulator();
    let started = start_sandboxes(&router, &["read-one", "read-two"]).await;
    let mut expected = started.clone();
    expected.reverse();

    for round in 1..=3 {
        let (ids, next_token) = list_sandboxes(&router, json!({})).await;
        assert_eq!(
            ids, expected,
            "round {round}: a repeated list must report the same ids; expected {expected:?}, \
             got {ids:?}"
        );
        assert_eq!(
            next_token, None,
            "round {round}: expected no nextToken, got {next_token:?}"
        );
    }

    // And the sandboxes themselves are still readable, by every id the list reported.
    let (status, body) = call(&router, "BatchGetSandboxes", json!({"ids": expected})).await;
    assert_eq!(
        status, 200,
        "BatchGetSandboxes after listing: expected 200, got {status} with {body}"
    );
    let found = body["sandboxes"].as_array().map(Vec::len).unwrap_or_default();
    assert_eq!(
        found,
        expected.len(),
        "every listed id must still name a sandbox: expected {} record(s), got {body}",
        expected.len()
    );
}
