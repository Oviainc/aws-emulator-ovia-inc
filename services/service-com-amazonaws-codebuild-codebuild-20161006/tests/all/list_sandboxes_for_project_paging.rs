// SPDX-License-Identifier: CC0-1.0

//! The paging and page-size rules of `ListSandboxesForProject`, driven over the wire
//! the router serves: `POST /` with `X-Amz-Target: CodeBuild_20161006.<Operation>` on
//! awsJson1_1.
//!
//! `list_sandboxes_for_project.rs` beside this one covers the ordering and the
//! not-found refusal over projects holding fewer sandboxes than one page. The rules
//! under test here are the ones only a project holding MORE than a default page's
//! worth of them shows:
//!
//! * a page holds at most `maxResults` ids, and at most this service's own page
//!   ceiling when the caller names none, with a `nextToken` that resumes exactly after
//!   the page it came with — so walking the pages reproduces the whole ordering;
//! * `maxResults` is accepted inside the range the model declares for it and refused
//!   outside it;
//! * a `nextToken` naming no entry of THIS project's list is refused rather than
//!   answered with a page, while one that names an entry opens the page at it;
//! * every id a page reports names a sandbox the implemented reader reports back.
//!
//! Each suite in `tests/all/` drives the wire itself, so the harness below is this
//! suite's own.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::state;
use service_com_amazonaws_codebuild_codebuild_20161006::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The access key id every request below is signed with.
const ACCESS_KEY_ID: &str = "AKIDCODEBUILDSBXPAGES";

/// How many pages a walk may take before it is called non-terminating: the biggest
/// project below holds `MAX_PAGE_SIZE + 5` sandboxes and the smallest page carries two,
/// so no walk here is anywhere near this many pages.
const MAX_PAGES_PER_WALK: usize = 200;

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

/// Start `count` sandboxes of `project` and hand back their ids, in start order.
async fn start_sandboxes(router: &MultiServiceRouter, project: &str, count: usize) -> Vec<String> {
    let mut ids = Vec::with_capacity(count);
    for index in 0..count {
        let (status, response) = call(router, "StartSandbox", json!({"projectName": project})).await;
        assert_eq!(
            status, 200,
            "StartSandbox {project:?} #{index}: expected 200, got {status} with {response}"
        );
        ids.push(
            response["sandbox"]["id"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("StartSandbox {project:?} #{index} must mint an id, got {response}")
                })
                .to_string(),
        );
    }
    ids
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

/// The `ids` member of a list answer as a vector of strings.
fn ids_of(case: &str, response: &Value) -> Vec<String> {
    response["ids"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the answer must carry an ids list, got {response}")
        })
        .iter()
        .map(|id| {
            id.as_str()
                .unwrap_or_else(|| panic!("case {case:?}: every id is a string, got {response}"))
                .to_string()
        })
        .collect()
}

/// The `__type` an error body carries, for asserting the error code a client branches on.
fn error_type(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// A string member, for an assertion message that shows what was actually there.
fn text(value: &Value, member: &str) -> String {
    value[member].as_str().unwrap_or_default().to_string()
}

/// Every id of `project`, walked page by page, asserting each page against the ceiling
/// in force — `max_results` when the caller names one, this service's page ceiling
/// otherwise — and that only the last page of the walk closes it.
async fn walk_pages(
    router: &MultiServiceRouter,
    case: &str,
    project: &str,
    sort_order: Option<&str>,
    max_results: Option<i32>,
) -> Vec<String> {
    let ceiling = max_results.map_or(state::MAX_PAGE_SIZE, |limit| limit as usize);
    let mut collected: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page_number in 0..MAX_PAGES_PER_WALK {
        let mut body = json!({"projectName": project});
        if let Some(order) = sort_order {
            body["sortOrder"] = json!(order);
        }
        if let Some(limit) = max_results {
            body["maxResults"] = json!(limit);
        }
        if let Some(resume) = &token {
            body["nextToken"] = json!(resume);
        }
        let response = list(router, body).await;
        let page = ids_of(case, &response);
        assert!(
            page.len() <= ceiling,
            "case {case:?}: page {page_number} must hold at most {ceiling} ids, \
             got {} in {response}",
            page.len()
        );
        let next = response["nextToken"].as_str().map(str::to_string);
        if next.is_some() {
            assert_eq!(
                page.len(),
                ceiling,
                "case {case:?}: page {page_number} is not the last, so it must be full, \
                 got {response}"
            );
        }
        collected.extend(page);
        match next {
            Some(resume) => token = Some(resume),
            // The walk ended: the last page carried no token.
            None => return collected,
        }
    }
    panic!("case {case:?}: the walk did not terminate within {MAX_PAGES_PER_WALK} pages");
}

/// RULE: a page holds at most `maxResults` ids — at most this service's page ceiling
/// when the caller names none — and the `nextToken` it carries resumes exactly after
/// it, so walking the pages reproduces the project's whole ordering.
///
/// The table walks the page sizes `PageSize` allows
/// (`ListSandboxesForProjectInput$maxResults`, `@range(min: 1, max: 100)`): the
/// smallest this suite walks over a long list, one between, the published maximum, and
/// the member left out — each in start order and reversed, against a project holding
/// more sandboxes than one default page can carry.
#[tokio::test]
async fn list_sandboxes_for_project_pages_at_max_results() {
    let router = emulator();

    let project = "paged-project";
    create_project(&router, project).await;
    // More than one default page, so the default ceiling is exercised as well as the
    // caller's own and at least one walk takes several pages.
    let ascending = start_sandboxes(&router, project, state::MAX_PAGE_SIZE + 5).await;
    let descending: Vec<String> = ascending.iter().rev().cloned().collect();

    let sizes: &[Option<i32>] = &[Some(2), Some(7), Some(100), None];
    for size in sizes {
        for (order_case, sort_order, expected) in [
            ("unspecified sortOrder", None, &ascending),
            ("ASCENDING", Some("ASCENDING"), &ascending),
            ("DESCENDING", Some("DESCENDING"), &descending),
        ] {
            let case = format!("maxResults {size:?} / {order_case}");
            let walked = walk_pages(&router, &case, project, sort_order, *size).await;
            assert_eq!(
                &walked, expected,
                "case {case:?}: paging must reproduce the whole ordering, got {walked:?}"
            );
        }
    }

    // Without a `maxResults` the first page stops at this service's own ceiling and
    // hands out a token for the rest.
    let first = list(&router, json!({"projectName": project})).await;
    assert_eq!(
        ids_of("the default page", &first).len(),
        state::MAX_PAGE_SIZE,
        "the default page holds this service's ceiling of ids, got {first}"
    );
    let token = text(&first, "nextToken");
    assert!(
        !token.is_empty(),
        "a project past the ceiling must carry a nextToken, got {first}"
    );
    let second = list(&router, json!({"projectName": project, "nextToken": token})).await;
    assert_eq!(
        ids_of("the second page", &second),
        ascending[state::MAX_PAGE_SIZE..].to_vec(),
        "the second page resumes after the first, got {second}"
    );
    assert!(
        second["nextToken"].is_null(),
        "the second page is the last one, got {second}"
    );
}

/// RULE: a `maxResults` inside the range the model declares is accepted; one outside it
/// is refused with HTTP 400 `InvalidInputException`.
///
/// The bounds are `PageSize`'s own (`@range(min: 1, max: 100)`), tested at each bound
/// and one past each.
#[tokio::test]
async fn a_max_results_outside_its_declared_range_is_refused() {
    let router = emulator();

    let project = "bounded-page-size";
    create_project(&router, project).await;
    let started = start_sandboxes(&router, project, 3).await;

    for (case, max_results, expected_len) in [
        ("at the minimum", 1, 1),
        ("between the bounds", 2, 2),
        ("at the maximum", 100, started.len()),
    ] {
        let response = list(
            &router,
            json!({"projectName": project, "maxResults": max_results}),
        )
        .await;
        assert_eq!(
            ids_of(case, &response).len(),
            expected_len,
            "case {case:?} (maxResults {max_results}): expected {expected_len} ids, got {response}"
        );
    }

    for (case, max_results) in [
        ("one below the minimum", 0),
        ("one past the maximum", 101),
        ("negative", -1),
    ] {
        let (status, body) = call(
            &router,
            "ListSandboxesForProject",
            json!({"projectName": project, "maxResults": max_results}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?} (maxResults {max_results}): expected 400, got {status} with {body}"
        );
        assert!(
            error_type(&body).contains("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {body}"
        );
    }
}

/// RULE: a `nextToken` naming an entry of the project's list opens the page at that
/// entry; one naming no entry of it is refused with HTTP 400 `InvalidInputException`
/// rather than answered with a page.
#[tokio::test]
async fn a_next_token_naming_no_entry_of_this_projects_list_is_refused() {
    let router = emulator();

    let project = "token-project";
    create_project(&router, project).await;
    let started = start_sandboxes(&router, project, 4).await;
    create_project(&router, "token-project-other").await;
    let other = start_sandboxes(&router, "token-project-other", 1).await;

    // A token naming an entry opens the page at it, for every entry of the list.
    for (index, id) in started.iter().enumerate() {
        let case = format!("resuming at entry {index}");
        let response = list(&router, json!({"projectName": project, "nextToken": id})).await;
        assert_eq!(
            ids_of(&case, &response),
            started[index..].to_vec(),
            "case {case:?}: the page must open at {id:?}, got {response}"
        );
    }

    // The token is resolved against the order the request asks for, so a reversed walk
    // resumes at the entry its own token names.
    let reversed: Vec<String> = started.iter().rev().cloned().collect();
    let response = list(
        &router,
        json!({"projectName": project, "sortOrder": "DESCENDING", "nextToken": reversed[1]}),
    )
    .await;
    assert_eq!(
        ids_of("resuming a descending walk", &response),
        reversed[1..].to_vec(),
        "a descending page must open at the entry its token names, got {response}"
    );

    for (case, token) in [
        ("a token of no resource at all", "not-a-token".to_string()),
        ("another project's sandbox id", other[0].clone()),
        (
            "a truncated id of this project",
            started[0][..started[0].len() - 1].to_string(),
        ),
        ("this project's name alone", project.to_string()),
        (
            "an id of this project with a suffix",
            format!("{}x", started[0]),
        ),
    ] {
        let (status, body) = call(
            &router,
            "ListSandboxesForProject",
            json!({"projectName": project, "nextToken": token}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?} ({token:?}): expected 400, got {status} with {body}"
        );
        assert!(
            error_type(&body).contains("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {body}"
        );
    }
}

/// RULE: every id a page reports names a sandbox of that project the implemented
/// reader hands back — the list reports stored records, not ids it made up.
#[tokio::test]
async fn every_listed_id_resolves_through_batch_get_sandboxes() {
    let router = emulator();

    // (project, how many sandboxes it holds). The project named `sandbox` is the case
    // where a sloppy parse of an id (`<projectName>:<uuid>`) or of the `sandbox/` ARN
    // resource segment goes wrong.
    let cases: &[(&str, usize)] = &[
        ("round-trip-one", 1),
        ("round-trip-five", 5),
        ("sandbox", 2),
    ];

    for (project, count) in cases {
        create_project(&router, project).await;
        let started = start_sandboxes(&router, project, *count).await;
        let listed = ids_of(project, &list(&router, json!({"projectName": project})).await);
        assert_eq!(
            listed, started,
            "case {project:?}: expected {started:?}, got {listed:?}"
        );

        let (status, fetched) = call(&router, "BatchGetSandboxes", json!({"ids": listed})).await;
        assert_eq!(
            status, 200,
            "case {project:?}: BatchGetSandboxes on the listed ids: expected 200, \
             got {status} with {fetched}"
        );
        assert_eq!(
            fetched["sandboxesNotFound"],
            json!([]),
            "case {project:?}: every listed id must resolve, got {fetched}"
        );
        let owners: Vec<String> = fetched["sandboxes"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|sandbox| text(sandbox, "projectName"))
            .collect();
        assert_eq!(
            owners,
            vec![project.to_string(); *count],
            "case {project:?}: every listed sandbox belongs to that project, got {fetched}"
        );
    }
}
