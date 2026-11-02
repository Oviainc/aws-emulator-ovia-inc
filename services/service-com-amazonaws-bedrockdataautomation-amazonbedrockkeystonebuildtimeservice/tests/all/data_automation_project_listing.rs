// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `ListDataAutomationProjects`.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed-looking request — so routing, HTTP binding, constraint validation,
//! the handler and response serialization are all exercised, exactly as an AWS SDK
//! `bedrock-data-automation` client would.
//!
//! The rule under test: the listing REPORTS the projects the store holds — every
//! project a create persisted and no project a delete released — projected onto the
//! modelled `DataAutomationProjectSummary`, in a stable order, narrowed by each
//! filter the request carries and cut into pages of at most `maxResults`. The suites
//! are TABLE-driven: each asserts that rule over a set of inputs differing along what
//! it generalises over (project count, stage, type, name form, page size, which
//! filter is named, how a project references the filtered resource), not over one
//! recorded request.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::state;
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{encode_label, error_code, string_at, timestamp_at};

/// The region every case signs for.
const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header for this service's signing name (`bedrock`, per
/// `OPERATIONS.md`).
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTKEY/20240101/{region}/bedrock/aws4_request, \
         SignedHeaders=host, Signature=0123456789abcdef"
    )
}

/// The emulator under test: one shared engine, one mounted service, request
/// authentication off so the suite signs nothing for real.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one request and return its status and decoded JSON body (`Value::Null`
/// for an empty body).
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "bedrock-data-automation.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", authorization(REGION))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("serializable body"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let decoded = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, decoded)
}

/// `ListDataAutomationProjects`, per its modelled `@http`
/// `{method: POST, uri: /data-automation-projects/, code: 200}`.
async fn list_projects(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/data-automation-projects/", body).await
}

/// `CreateDataAutomationProject` -> the `projectArn` it minted. Panics with the case
/// name on any non-201, so a broken create fails where it happened.
async fn create_project(router: &MultiServiceRouter, case: &str, body: &Value) -> String {
    let (status, payload) = call(router, "PUT", "/data-automation-projects/", body).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateDataAutomationProject 201, got {status} with {payload:?}"
    );
    string_at(&payload, "projectArn", case)
}

/// `GetDataAutomationProject`, per its modelled `@http`
/// `{method: POST, uri: /data-automation-projects/{projectArn}/}`.
async fn get_project(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    let uri = format!("/data-automation-projects/{}/", encode_label(arn));
    call(router, "POST", &uri, &json!({})).await
}

/// `DeleteDataAutomationProject`, per its modelled `@http`
/// `{method: DELETE, uri: /data-automation-projects/{projectArn}/}`.
async fn delete_project(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    let uri = format!("/data-automation-projects/{}/", encode_label(arn));
    call(router, "DELETE", &uri, &json!({})).await
}

/// `CreateBlueprint` -> the `blueprintArn` it minted, so a blueprint filter can name
/// a blueprint that really exists.
async fn create_blueprint(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let body = json!({
        "blueprintName": name,
        "type": "DOCUMENT",
        "schema": "{}",
    });
    let (status, payload) = call(router, "PUT", "/blueprints/", &body).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateBlueprint 201, got {status} with {payload:?}"
    );
    let blueprint = payload
        .get("blueprint")
        .unwrap_or_else(|| panic!("case {case:?}: CreateBlueprint returned no blueprint: {payload:?}"));
    string_at(blueprint, "blueprintArn", case)
}

/// The `projects` page of an answered listing, or a panic naming the case: the member
/// is modelled `required`, so it is present even when nothing matched.
fn page_of(payload: &Value, case: &str) -> Vec<Value> {
    payload
        .get("projects")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: the listing carried no projects member: {payload:?}"))
        .clone()
}

/// The `projectArn`s of one answered page, in the order they were serialized.
fn arns_of(payload: &Value, case: &str) -> Vec<String> {
    page_of(payload, case)
        .iter()
        .map(|summary| string_at(summary, "projectArn", case))
        .collect()
}

/// One row of the lifecycle table: a project to create, described by the members a
/// summary reports back.
struct ProjectCase {
    case: &'static str,
    name: &'static str,
    /// `projectStage`, or `None` to leave it absent so the create defaults it.
    stage: Option<&'static str>,
    /// `projectType`, or `None` to leave it absent so the modelled `@default` applies.
    project_type: Option<&'static str>,
    description: Option<&'static str>,
    standard_output: Value,
    tags: usize,
}

impl ProjectCase {
    fn body(&self) -> Value {
        let mut body = json!({
            "projectName": self.name,
            "standardOutputConfiguration": self.standard_output,
        });
        if let Some(stage) = self.stage {
            body["projectStage"] = json!(stage);
        }
        if let Some(project_type) = self.project_type {
            body["projectType"] = json!(project_type);
        }
        if let Some(description) = self.description {
            body["projectDescription"] = json!(description);
        }
        if self.tags > 0 {
            let tags: Vec<Value> = (0..self.tags)
                .map(|i| json!({"key": format!("k{i}"), "value": format!("v{i}")}))
                .collect();
            body["tags"] = json!(tags);
        }
        body
    }

    /// The stage the stored project carries: the one named, else the stage a project
    /// created with none takes.
    fn expected_stage(&self) -> &str {
        self.stage.unwrap_or(state::DEFAULT_PROJECT_STAGE)
    }

    /// The type the stored project carries: the one named, else the member's modelled
    /// `@default("ASYNC")` (source: `smithy_query(operation=
    /// "CreateDataAutomationProject")` -> `projectType`).
    fn expected_type(&self) -> &str {
        self.project_type.unwrap_or("ASYNC")
    }
}

/// The lifecycle table. The rows differ in stage (both values of
/// `smithy_query(shape="DataAutomationProjectStage")` plus the absent case), type
/// (both values of `DataAutomationProjectType` plus the absent case), name form and
/// length (both published bounds of `@length(1..128)` and the whole
/// `^[a-zA-Z0-9-_]+$` class), description, the shape of the standard output
/// configuration and the tag count.
fn lifecycle_cases() -> Vec<ProjectCase> {
    vec![
        ProjectCase {
            case: "minimal",
            name: "a",
            stage: None,
            project_type: None,
            description: None,
            standard_output: json!({}),
            tags: 0,
        },
        ProjectCase {
            case: "development-sync",
            name: "listing-DEV_project-1",
            stage: Some("DEVELOPMENT"),
            project_type: Some("SYNC"),
            description: Some("a development project"),
            standard_output: json!({"document": {"outputFormat": {
                "textFormat": {"types": ["PLAIN_TEXT"]},
                "additionalFileFormat": {"state": "DISABLED"},
            }}}),
            tags: 1,
        },
        ProjectCase {
            case: "live-async",
            name: "listing-live-project",
            stage: Some("LIVE"),
            project_type: Some("ASYNC"),
            description: None,
            standard_output: json!({"image": {"generativeField": {"state": "ENABLED"}}}),
            tags: 3,
        },
        ProjectCase {
            case: "long-name",
            name: "LISTING-0123456789-abcdefghijklmnopqrstuvwxyz-ABCDEFGHIJKLMNOPQRSTUVWXYZ-0123456789-abcdefghijklmnopqrstuvwxyz_end",
            stage: Some("DEVELOPMENT"),
            project_type: None,
            description: Some("a project whose name walks the upper length bound"),
            standard_output: json!({"audio": {"generativeField": {"state": "DISABLED"}}}),
            tags: 0,
        },
    ]
}

#[tokio::test]
async fn a_listing_reports_every_stored_project_as_the_modeled_summary() {
    let router = emulator();
    let cases = lifecycle_cases();

    let mut created: Vec<(&str, String)> = Vec::new();
    for spec in &cases {
        created.push((spec.case, create_project(&router, spec.case, &spec.body()).await));
    }

    let (status, payload) = list_projects(&router, &json!({})).await;
    assert_eq!(
        status, 200,
        "an unfiltered listing must answer the modelled 200, got {status} with {payload:?}"
    );
    let page = page_of(&payload, "unfiltered");
    assert_eq!(
        page.len(),
        cases.len(),
        "unfiltered: the listing must report all {} stored projects, got {page:?}",
        cases.len()
    );
    assert!(
        payload.get("nextToken").is_none(),
        "unfiltered: a listing that fits one page must carry no nextToken: {payload:?}"
    );

    for (spec, (case, arn)) in cases.iter().zip(created.iter()) {
        let summary = page
            .iter()
            .find(|summary| string_at(summary, "projectArn", case) == *arn)
            .unwrap_or_else(|| panic!("case {case:?}: {arn} missing from the listing: {page:?}"));
        assert_eq!(
            string_at(summary, "projectName", case),
            spec.name,
            "case {case:?}: wrong projectName in {summary:?}"
        );
        assert_eq!(
            string_at(summary, "projectStage", case),
            spec.expected_stage(),
            "case {case:?}: wrong projectStage in {summary:?}"
        );
        assert_eq!(
            string_at(summary, "projectType", case),
            spec.expected_type(),
            "case {case:?}: wrong projectType in {summary:?}"
        );
        // The summary reports what the resource itself reports: the listing is a
        // projection of the stored record, not a second copy of it.
        let (get_status, fetched) = get_project(&router, arn).await;
        assert_eq!(
            get_status, 200,
            "case {case:?}: GetDataAutomationProject must answer 200, got {get_status} with {fetched:?}"
        );
        let project = fetched
            .get("project")
            .unwrap_or_else(|| panic!("case {case:?}: no project in {fetched:?}"));
        assert_eq!(
            timestamp_at(summary, "creationTime", case),
            timestamp_at(project, "creationTime", case),
            "case {case:?}: the listed creationTime differs from the one the project reports"
        );
        assert!(
            summary.get("status").is_none(),
            "case {case:?}: the modelled summary carries no status member: {summary:?}"
        );
    }

    // The order is stable and non-decreasing in creationTime, so a page boundary is
    // reproducible across calls.
    let times: Vec<(i64, u32)> = page
        .iter()
        .map(|summary| timestamp_at(summary, "creationTime", "order"))
        .collect();
    assert!(
        times.windows(2).all(|pair| pair[0] <= pair[1]),
        "order: the listing must be ordered by creationTime, got {times:?}"
    );
    let (second_status, second) = list_projects(&router, &json!({})).await;
    assert_eq!(second_status, 200, "a repeated listing must answer 200");
    assert_eq!(
        arns_of(&second, "repeat"),
        arns_of(&payload, "repeat"),
        "repeat: two listings of the same store must agree on the order"
    );
}

#[tokio::test]
async fn a_deleted_project_leaves_the_listing_and_the_others_stay() {
    let router = emulator();
    let cases = lifecycle_cases();

    let mut created: Vec<(&str, String)> = Vec::new();
    for spec in &cases {
        created.push((spec.case, create_project(&router, spec.case, &spec.body()).await));
    }

    // Release the projects one at a time; after each delete the listing reports
    // exactly the projects that are still stored.
    let mut remaining: Vec<(&str, String)> = created.clone();
    while let Some((case, arn)) = remaining.pop() {
        let (status, payload) = delete_project(&router, &arn).await;
        assert!(
            (200..300).contains(&status),
            "case {case:?}: DeleteDataAutomationProject must succeed, got {status} with {payload:?}"
        );
        let (status, payload) = list_projects(&router, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the listing must still answer 200, got {status} with {payload:?}"
        );
        let listed: BTreeSet<String> = arns_of(&payload, case).into_iter().collect();
        let expected: BTreeSet<String> = remaining.iter().map(|(_, arn)| arn.clone()).collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: after deleting {arn} the listing must report exactly the {} projects left",
            expected.len()
        );
    }
}

#[tokio::test]
async fn the_stage_filter_selects_the_projects_at_that_stage() {
    let router = emulator();

    // Two projects in each stage, so no row can pass by accident of there being one.
    let mut by_stage: Vec<(&str, String)> = Vec::new();
    for stage in ["DEVELOPMENT", "LIVE"] {
        for i in 0..2 {
            let body = json!({
                "projectName": format!("stage-{stage}-{i}"),
                "projectStage": stage,
                "standardOutputConfiguration": {},
            });
            let arn = create_project(&router, stage, &body).await;
            by_stage.push((stage, arn));
        }
    }
    let of_stage = |wanted: &str| -> BTreeSet<String> {
        by_stage
            .iter()
            .filter(|(stage, _)| *stage == wanted)
            .map(|(_, arn)| arn.clone())
            .collect()
    };
    let all: BTreeSet<String> = by_stage.iter().map(|(_, arn)| arn.clone()).collect();

    // Every value of `smithy_query(shape="DataAutomationProjectStageFilter")`
    // (`DEVELOPMENT`, `LIVE`, `ALL`) plus the absent case.
    let cases: Vec<(&str, Value, BTreeSet<String>)> = vec![
        ("absent", json!({}), all.clone()),
        ("ALL", json!({"projectStageFilter": "ALL"}), all.clone()),
        (
            "DEVELOPMENT",
            json!({"projectStageFilter": "DEVELOPMENT"}),
            of_stage("DEVELOPMENT"),
        ),
        (
            "LIVE",
            json!({"projectStageFilter": "LIVE"}),
            of_stage("LIVE"),
        ),
    ];

    for (case, body, expected) in &cases {
        let (status, payload) = list_projects(&router, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a stage-filtered listing must answer 200, got {status} with {payload:?}"
        );
        let listed: BTreeSet<String> = arns_of(&payload, case).into_iter().collect();
        assert_eq!(
            &listed, expected,
            "case {case:?}: the stage filter selected the wrong projects"
        );
    }
}

#[tokio::test]
async fn a_listing_pages_through_every_project_exactly_once() {
    let router = emulator();

    const PROJECTS: usize = 5;
    let mut all: BTreeSet<String> = BTreeSet::new();
    for i in 0..PROJECTS {
        let body = json!({
            "projectName": format!("paged-{i}"),
            "standardOutputConfiguration": {},
        });
        all.insert(create_project(&router, "paged", &body).await);
    }

    // Page sizes inside the modelled `@range(min: 1, max: 1000)`: the lower bound,
    // sizes that do and do not divide the project count, one larger than the whole
    // store, and the absent case that takes the server's own page limit.
    let page_sizes: &[Option<i32>] = &[
        Some(1),
        Some(2),
        Some(3),
        Some(PROJECTS as i32),
        Some(PROJECTS as i32 + 2),
        Some(state::PROJECT_PAGE_LIMIT),
        None,
    ];

    for size in page_sizes {
        let case = format!("maxResults={size:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut body = json!({});
            if let Some(size) = size {
                body["maxResults"] = json!(size);
            }
            if let Some(token) = token.as_deref() {
                body["nextToken"] = json!(token);
            }
            let (status, payload) = list_projects(&router, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: page {pages} must answer 200, got {status} with {payload:?}"
            );
            let arns = arns_of(&payload, &case);
            if let Some(size) = size {
                assert!(
                    arns.len() <= *size as usize,
                    "case {case:?}: page {pages} carries {} summaries, over the requested {size}",
                    arns.len()
                );
            }
            seen.extend(arns);
            pages += 1;
            token = payload
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= PROJECTS + 1,
                "case {case:?}: the walk did not terminate after {pages} pages"
            );
        }
        let walked: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            walked.len(),
            seen.len(),
            "case {case:?}: a project was reported twice across the pages: {seen:?}"
        );
        assert_eq!(
            walked, all,
            "case {case:?}: the walk must report every stored project exactly once"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_the_modeled_validation_error() {
    let router = emulator();
    let body = json!({
        "projectName": "token-check",
        "standardOutputConfiguration": {},
    });
    create_project(&router, "token-check", &body).await;

    // Rows differ in HOW the token is wrong: not base64 at all, base64 of something
    // that is not a cursor, and base64 of a cursor missing its ARN field.
    let cases: &[(&str, &str)] = &[
        ("not-base64", "not-a-token-this-service-issued"),
        ("base64-of-garbage", "aGVsbG8="),
        ("cursor-without-arn", "MTIzOjQ1Njo="),
    ];

    for (case, token) in cases {
        let (status, payload) = list_projects(&router, &json!({"nextToken": token})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a token this service did not issue must be 400, got {status} with {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?}: wrong error code in {payload:?}"
        );
        assert!(
            payload.get("projects").is_none(),
            "case {case:?}: the refusal must not carry a page: {payload:?}"
        );
    }
}

#[tokio::test]
async fn the_blueprint_filter_selects_the_projects_that_reference_that_blueprint() {
    let router = emulator();

    let first = create_blueprint(&router, "first", "listing-filter-bp-1").await;
    let second = create_blueprint(&router, "second", "listing-filter-bp-2").await;

    // The three ways a project's stored configuration can reference a blueprint —
    // `customOutputConfiguration.blueprints`, the same with a revision stated, and
    // `customOutputConfiguration.document.fallbackBlueprints` (source:
    // `smithy_query(shape="CustomOutputConfiguration")`) — plus a project that
    // references none.
    let direct = create_project(
        &router,
        "direct",
        &json!({
            "projectName": "filter-direct",
            "standardOutputConfiguration": {},
            "customOutputConfiguration": {"blueprints": [{"blueprintArn": first}]},
        }),
    )
    .await;
    let versioned = create_project(
        &router,
        "versioned",
        &json!({
            "projectName": "filter-versioned",
            "standardOutputConfiguration": {},
            "customOutputConfiguration": {"blueprints": [{
                "blueprintArn": first,
                "blueprintVersion": "1",
                // The stage the blueprint created above actually holds: a create that
                // names none settles at LIVE (`state::DEFAULT_BLUEPRINT_STAGE`).
                "blueprintStage": "LIVE",
            }]},
        }),
    )
    .await;
    let fallback = create_project(
        &router,
        "fallback",
        &json!({
            "projectName": "filter-fallback",
            "standardOutputConfiguration": {},
            "customOutputConfiguration": {"document": {"fallbackBlueprints": [
                {"blueprintArn": second}
            ]}},
        }),
    )
    .await;
    let unrelated = create_project(
        &router,
        "unrelated",
        &json!({
            "projectName": "filter-unrelated",
            "standardOutputConfiguration": {},
        }),
    )
    .await;

    let cases: Vec<(&str, Value, Vec<&String>)> = vec![
        (
            "absent",
            json!({}),
            vec![&direct, &versioned, &fallback, &unrelated],
        ),
        (
            "by-arn",
            json!({"blueprintFilter": {"blueprintArn": first}}),
            vec![&direct, &versioned],
        ),
        (
            "by-arn-of-the-fallback",
            json!({"blueprintFilter": {"blueprintArn": second}}),
            vec![&fallback],
        ),
        (
            "agreeing-version",
            json!({"blueprintFilter": {"blueprintArn": first, "blueprintVersion": "1"}}),
            vec![&direct, &versioned],
        ),
        (
            "disagreeing-version",
            json!({"blueprintFilter": {"blueprintArn": first, "blueprintVersion": "2"}}),
            vec![&direct],
        ),
        (
            // `create_blueprint` above names no `blueprintStage`, so the blueprint it
            // created is at the stage a create settles at when the request names none:
            // LIVE (`state::DEFAULT_BLUEPRINT_STAGE`).
            "agreeing-stage",
            json!({"blueprintFilter": {"blueprintArn": first, "blueprintStage": "LIVE"}}),
            vec![&direct, &versioned],
        ),
    ];

    for (case, body, expected) in &cases {
        let (status, payload) = list_projects(&router, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a blueprint-filtered listing must answer 200, got {status} with {payload:?}"
        );
        let listed: BTreeSet<String> = arns_of(&payload, case).into_iter().collect();
        let expected: BTreeSet<String> = expected.iter().map(|arn| (*arn).clone()).collect();
        assert_eq!(
            &listed, &expected,
            "case {case:?}: the blueprint filter selected the wrong projects"
        );
    }

    // A filter naming a blueprint of this account that the store does not hold — or
    // holds at another stage, which the filter's `blueprintStage` selects — references
    // a resource that does not exist: the modelled 404.
    let refusals: &[(&str, Value)] = &[
        (
            "unknown-blueprint",
            json!({"blueprintFilter": {
                "blueprintArn": "arn:aws:bedrock:us-east-1:123456789012:blueprint/aaaabbbbcccc"
            }}),
        ),
        (
            "unknown-blueprint-other-account",
            json!({"blueprintFilter": {
                "blueprintArn": "arn:aws:bedrock:eu-west-2:210987654321:blueprint/0123456789abcdef"
            }}),
        ),
        (
            "stage-the-blueprint-is-not-in",
            json!({"blueprintFilter": {"blueprintArn": first, "blueprintStage": "DEVELOPMENT"}}),
        ),
    ];

    for (case, body) in refusals {
        let (status, payload) = list_projects(&router, body).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a filter naming a blueprint that does not exist must be 404, \
             got {status} with {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "ResourceNotFoundException",
            "case {case:?}: wrong error code in {payload:?}"
        );
        assert!(
            payload.get("projects").is_none(),
            "case {case:?}: the refusal must not carry a page: {payload:?}"
        );
    }
}

#[tokio::test]
async fn the_library_filter_selects_the_projects_that_reference_that_library() {
    let router = emulator();

    // Library ARNs of the modelled form
    // `^arn:aws(|-cn|-iso|-iso-[a-z]|-us-gov):bedrock:[a-zA-Z0-9-]*:[0-9]{12}:data-automation-library/[a-zA-Z0-9-]{12,36}$`
    // (source: `smithy_query(shape="DataAutomationLibraryItem")`).
    let first = "arn:aws:bedrock:us-east-1:123456789012:data-automation-library/library-one0";
    let second = "arn:aws:bedrock:eu-west-2:210987654321:data-automation-library/library-two-0123456789";
    let unreferenced =
        "arn:aws:bedrock:us-east-1:123456789012:data-automation-library/library-three";

    let with_first = create_project(
        &router,
        "with-first",
        &json!({
            "projectName": "library-first",
            "standardOutputConfiguration": {},
            "dataAutomationLibraryConfiguration": {"libraries": [{"libraryArn": first}]},
        }),
    )
    .await;
    let with_second = create_project(
        &router,
        "with-second",
        &json!({
            "projectName": "library-second",
            "standardOutputConfiguration": {},
            "dataAutomationLibraryConfiguration": {"libraries": [{"libraryArn": second}]},
        }),
    )
    .await;
    let without = create_project(
        &router,
        "without",
        &json!({
            "projectName": "library-none",
            "standardOutputConfiguration": {},
        }),
    )
    .await;

    let cases: Vec<(&str, Value, Vec<&String>)> = vec![
        (
            "absent",
            json!({}),
            vec![&with_first, &with_second, &without],
        ),
        (
            "first",
            json!({"libraryFilter": {"libraryArn": first}}),
            vec![&with_first],
        ),
        (
            "second",
            json!({"libraryFilter": {"libraryArn": second}}),
            vec![&with_second],
        ),
        (
            "referenced-by-nothing",
            json!({"libraryFilter": {"libraryArn": unreferenced}}),
            vec![],
        ),
    ];

    for (case, body, expected) in &cases {
        let (status, payload) = list_projects(&router, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a library-filtered listing must answer 200, got {status} with {payload:?}"
        );
        let listed: BTreeSet<String> = arns_of(&payload, case).into_iter().collect();
        let expected: BTreeSet<String> = expected.iter().map(|arn| (*arn).clone()).collect();
        assert_eq!(
            &listed, &expected,
            "case {case:?}: the library filter selected the wrong projects"
        );
    }
}

#[tokio::test]
async fn the_resource_owner_selects_whose_projects_are_listed() {
    let router = emulator();

    let mut all: BTreeSet<String> = BTreeSet::new();
    for i in 0..3 {
        let body = json!({
            "projectName": format!("owned-{i}"),
            "standardOutputConfiguration": {},
        });
        all.insert(create_project(&router, "owned", &body).await);
    }

    // Both values of `smithy_query(shape="ResourceOwner")` plus the absent case: every
    // project this emulator stores was created through the API, so it is ACCOUNT-owned
    // and a SERVICE listing reports none of them.
    let cases: Vec<(&str, Value, BTreeSet<String>)> = vec![
        ("absent", json!({}), all.clone()),
        ("ACCOUNT", json!({"resourceOwner": "ACCOUNT"}), all.clone()),
        (
            "SERVICE",
            json!({"resourceOwner": "SERVICE"}),
            BTreeSet::new(),
        ),
    ];

    for (case, body, expected) in &cases {
        let (status, payload) = list_projects(&router, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the listing must answer 200, got {status} with {payload:?}"
        );
        let listed: BTreeSet<String> = arns_of(&payload, case).into_iter().collect();
        assert_eq!(
            &listed, expected,
            "case {case:?}: the resource owner selected the wrong projects"
        );
        assert!(
            payload.get("nextToken").is_none(),
            "case {case:?}: a one-page listing must carry no nextToken: {payload:?}"
        );
    }
}
