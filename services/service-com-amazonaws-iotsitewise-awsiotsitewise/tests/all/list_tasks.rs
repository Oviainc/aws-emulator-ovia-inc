// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListTasks`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a listing enumerates exactly the tasks `CreateTask` recorded for the workspace
//!   the request names, and no task of another workspace;
//! * a workspace this service does not hold is the modelled not-found refusal, not
//!   an empty page, while a held workspace with no task is an empty page;
//! * every summary is read off the stored task — the same members a read of that one
//!   task reports, including the version the newest accepted write minted — and a
//!   listing changes none of them;
//! * `maxResults`/`nextToken` page one stable enumeration: the pages joined are the
//!   whole listing once, in one order, whatever page size the caller asks for, and a
//!   token this listing never issued is refused.
//!
//! The HTTP harness (`call`, `router`, `error_code`, `hex_token`) is the one the
//! asset suite already has; this suite reuses it rather than growing a second.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    ACTIVATION_DELAY, FIRST_TASK_VERSION,
};

use crate::asset_lifecycle::{call, error_code, hex_token, router, REGION};

/// The workspace every suite below lists.
const WORKSPACE: &str = "Plant1";

/// A second workspace holding tasks of its own, so a listing can be shown to report
/// only the tasks of the one it names.
const OTHER_WORKSPACE: &str = "Plant2";

/// A third workspace nobody creates a task in, so an empty page can be told apart
/// from an absent workspace.
const EMPTY_WORKSPACE: &str = "Plant3";

/// `POST|GET /workspaces/{workspaceName}/tasks`, as `OPERATIONS.md` reports the
/// route of both `CreateTask` (POST) and `ListTasks` (GET).
fn tasks_uri(workspace: &str) -> String {
    format!("/workspaces/{workspace}/tasks")
}

/// `GET|PUT /workspaces/{workspaceName}/tasks/{taskName}`.
fn task_uri(workspace: &str, name: &str) -> String {
    format!("/workspaces/{workspace}/tasks/{name}")
}

/// The ARN form this emulator mints for a task: the workspace as well as the name,
/// since the name is unique only within the workspace.
fn expected_arn(workspace: &str, name: &str) -> String {
    format!(
        "arn:aws:iotsitewise:{REGION}:{}:task/{workspace}/{name}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// An ECR image URI of the private-registry form `EcrUri`'s pattern admits
/// (smithy_query query="shape" shape="ContainerTaskConfiguration": `ecrUri`).
const PRIVATE_IMAGE: &str = "123456789012.dkr.ecr.us-west-2.amazonaws.com/analytics:v1";

/// The same member's public-registry form.
const PUBLIC_IMAGE: &str = "public.ecr.aws/docker/library/python:3.12";

/// An IAM role ARN of the form `IamRoleArn`'s pattern admits (same shape,
/// `taskExecutionRole`).
const EXECUTION_ROLE: &str = "arn:aws:iam::123456789012:role/SiteWiseTaskRole";

/// The `ProcessingType` values, from the enum itself (smithy_query query="shape"
/// shape="ProcessingType").
const PROCESSING_TYPES: [&str; 2] = [
    "GENERIC_COMPUTE_PROCESSING",
    "HARDWARE_ACCELERATED_PROCESSING",
];

/// Three of the `ProcessingUnit` values, from the enum itself (smithy_query
/// query="shape" shape="ProcessingUnit"): the smallest, one in the middle and the
/// largest.
const PROCESSING_UNITS: [&str; 3] = ["UNITS_2", "UNITS_36", "UNITS_96"];

/// The published maximum of `timeoutSeconds` (`@range(min: 60, max: 86400)`).
const TIMEOUT_MAX: i64 = 86_400;

/// The published bounds of `maxResults` (smithy_query query="operation"
/// operation="ListTasks": `@range(min: 1, max: 250)`).
const MAX_RESULTS_MIN: i64 = 1;
/// See [`MAX_RESULTS_MIN`].
const MAX_RESULTS_MAX: i64 = 250;

/// The page size this operation's `maxResults` documents when the caller sends
/// none ("Default: 50"), so a listing shorter than it comes back in ONE page.
const DEFAULT_MAX_RESULTS: usize = 50;

/// A `containerTaskConfiguration` carrying only the members the shape requires.
fn minimal_container(image: &str, processing_type: &str, processing_unit: &str) -> Value {
    json!({
        "containerTaskConfiguration": {
            "ecrUri": image,
            "taskExecutionRole": EXECUTION_ROLE,
            "processingType": processing_type,
            "processingUnit": processing_unit,
        }
    })
}

/// Create `name` through `CreateWorkspace`, so a task created in it names a
/// workspace this service really holds.
async fn create_workspace(router: &MultiServiceRouter, name: &str) {
    let (status, body) = call(
        router,
        "POST",
        "/workspaces",
        Some(json!({
            "workspaceName": name,
            "encryptionConfiguration": { "encryptionType": "SITEWISE_DEFAULT_ENCRYPTION" },
        })),
    )
    .await;
    assert_eq!(status, 200, "create workspace {name}: got {status} {body}");
}

/// The three workspaces the suites work in.
async fn fixture(router: &MultiServiceRouter) {
    for name in [WORKSPACE, OTHER_WORKSPACE, EMPTY_WORKSPACE] {
        create_workspace(router, name).await;
    }
}

/// `CreateTask` in `workspace`, asserting it was accepted.
async fn create_task(
    router: &MultiServiceRouter,
    workspace: &str,
    name: &str,
    description: Option<&str>,
    configuration: Value,
) {
    let mut body = json!({ "taskName": name, "taskConfiguration": configuration });
    if let Some(description) = description {
        body.as_object_mut()
            .expect("an object body")
            .insert("description".to_string(), json!(description));
    }
    let (status, created) = call(router, "POST", &tasks_uri(workspace), Some(body)).await;
    assert_eq!(
        status, 200,
        "create task {name} in {workspace}: expected 200, got {status} {created}"
    );
}

/// The tasks the suites create: rows differing along the members a summary reports
/// — a description and none, both registry forms of the image, each processing type
/// and three processing units, with and without the optional timeout — so what a
/// listing reports cannot be an echo of one task's shape.
fn task_cases() -> Vec<(&'static str, Option<&'static str>, Value)> {
    vec![
        (
            "task-minimal",
            None,
            minimal_container(PRIVATE_IMAGE, PROCESSING_TYPES[0], PROCESSING_UNITS[0]),
        ),
        (
            "task-described",
            Some("a described task"),
            minimal_container(PRIVATE_IMAGE, PROCESSING_TYPES[0], PROCESSING_UNITS[1]),
        ),
        (
            "task-public-image",
            Some("an image from the public registry"),
            minimal_container(PUBLIC_IMAGE, PROCESSING_TYPES[1], PROCESSING_UNITS[2]),
        ),
        (
            "Task-Mixed-Case",
            Some("a name the modelled pattern admits"),
            minimal_container(PUBLIC_IMAGE, PROCESSING_TYPES[0], PROCESSING_UNITS[2]),
        ),
        (
            "task_with_underscores",
            Some("a name of the underscore form the pattern admits"),
            minimal_container(PRIVATE_IMAGE, PROCESSING_TYPES[1], PROCESSING_UNITS[1]),
        ),
        ("task-with-a-timeout", Some("a bounded run"), {
            json!({
                "containerTaskConfiguration": {
                    "ecrUri": PRIVATE_IMAGE,
                    "taskExecutionRole": EXECUTION_ROLE,
                    "processingType": PROCESSING_TYPES[0],
                    "processingUnit": PROCESSING_UNITS[0],
                    "timeoutSeconds": TIMEOUT_MAX,
                }
            })
        }),
        (
            "t",
            Some("a name at the modelled minimum length"),
            minimal_container(PRIVATE_IMAGE, PROCESSING_TYPES[1], PROCESSING_UNITS[0]),
        ),
    ]
}

/// Create every row of [`task_cases`] in `workspace`, and the names created.
async fn create_task_cases(router: &MultiServiceRouter, workspace: &str) -> Vec<String> {
    let mut names = Vec::new();
    for (name, description, configuration) in task_cases() {
        create_task(router, workspace, name, description, configuration).await;
        names.push(name.to_string());
    }
    names
}

/// `ListTasks` on `workspace` with the raw query string `query` (empty for none).
async fn list_raw(router: &MultiServiceRouter, workspace: &str, query: &str) -> (u16, Value) {
    let uri = if query.is_empty() {
        tasks_uri(workspace)
    } else {
        format!("{}?{query}", tasks_uri(workspace))
    };
    call(router, "GET", &uri, None).await
}

/// `ListTasks`, asserting it answered and that the required `taskSummaries` member
/// is a list, and the body it answered with.
async fn listed(router: &MultiServiceRouter, workspace: &str, query: &str) -> Value {
    let (status, body) = list_raw(router, workspace, query).await;
    assert_eq!(
        status, 200,
        "list {workspace} ?{query}: expected 200, got {status} {body}"
    );
    assert!(
        body["taskSummaries"].is_array(),
        "list {workspace} ?{query}: taskSummaries is a required list, got {body}"
    );
    body
}

/// The summaries of a listing body.
fn summaries(body: &Value) -> &Vec<Value> {
    body["taskSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("a listing holds a taskSummaries list, got {body}"))
}

/// The task names a listing body reports, in the order it reported them.
fn names(body: &Value) -> Vec<String> {
    summaries(body)
        .iter()
        .map(|summary| {
            summary["taskName"]
                .as_str()
                .unwrap_or_else(|| panic!("a summary carries a taskName, got {summary}"))
                .to_string()
        })
        .collect()
}

/// Every page of a listing of `workspace`, walked with `page_size` when given and
/// with no `maxResults` otherwise: the names in order, and how many pages it took.
async fn walk(
    router: &MultiServiceRouter,
    workspace: &str,
    page_size: Option<i64>,
    expected_total: usize,
) -> (Vec<String>, usize) {
    let case = match page_size {
        Some(size) => format!("maxResults={size}"),
        None => "the default page size".to_string(),
    };
    let mut collected: Vec<String> = Vec::new();
    let mut pages = 0usize;
    let mut token: Option<String> = None;
    loop {
        let mut query: Vec<String> = Vec::new();
        if let Some(size) = page_size {
            query.push(format!("maxResults={size}"));
        }
        if let Some(token) = &token {
            query.push(format!("nextToken={token}"));
        }
        let body = listed(router, workspace, &query.join("&")).await;
        let page = names(&body);
        pages += 1;
        if let Some(size) = page_size {
            assert!(
                page.len() as i64 <= size,
                "case {case:?}: page {pages} holds {} summaries, more than asked: {body}",
                page.len()
            );
        }
        collected.extend(page);
        match body["nextToken"].as_str() {
            Some(next) => {
                // A page followed by a token has to leave something to resume from.
                assert!(
                    collected.len() < expected_total,
                    "case {case:?}: page {pages} ends the listing yet carries {next}"
                );
                token = Some(next.to_string());
            }
            None => break,
        }
        assert!(
            pages <= expected_total + 2,
            "case {case:?}: paging did not terminate after {pages} pages"
        );
    }
    (collected, pages)
}

#[tokio::test]
async fn a_listing_reports_the_tasks_of_the_workspace_it_names() {
    let router = router();
    fixture(&router).await;

    let here = create_task_cases(&router, WORKSPACE).await;
    // The same names in another workspace as well as two of its own: a task name is
    // unique only within its workspace, so a listing that keyed on the name alone
    // would report these too.
    let mut there = create_task_cases(&router, OTHER_WORKSPACE).await;
    for name in ["only-in-plant2", "also-only-in-plant2"] {
        create_task(
            &router,
            OTHER_WORKSPACE,
            name,
            None,
            minimal_container(PRIVATE_IMAGE, PROCESSING_TYPES[0], PROCESSING_UNITS[0]),
        )
        .await;
        there.push(name.to_string());
    }

    for (case, workspace, expected) in [
        ("the workspace the tasks were created in", WORKSPACE, &here),
        (
            "another workspace with tasks of its own",
            OTHER_WORKSPACE,
            &there,
        ),
    ] {
        let body = listed(&router, workspace, "maxResults=250").await;
        let reported: BTreeSet<String> = names(&body).into_iter().collect();
        let wanted: BTreeSet<String> = expected.iter().cloned().collect();
        assert_eq!(
            reported, wanted,
            "case {case:?}: a listing reports exactly that workspace's tasks, got {body}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: a listing that reported everything carries no nextToken, got {body}"
        );
    }

    // A held workspace nobody created a task in is an EMPTY page — absence of tasks
    // is not absence of the workspace.
    let empty = listed(&router, EMPTY_WORKSPACE, "").await;
    assert!(
        summaries(&empty).is_empty() && empty["nextToken"].is_null(),
        "a workspace with no task answers an empty page, got {empty}"
    );

    // A workspace this service does not hold, in every form of a name no
    // `CreateWorkspace` minted — including ones differing from a held name only in
    // case, since a workspace is named exactly. The workspace the PATH names is
    // resolved before the paging arguments are read, so a request that is wrong in
    // both ways still reports the resource that is absent.
    for name in ["Plant9", "plant1", "PLANT1", "Plant1-copy"] {
        for query in ["", "maxResults=1", "nextToken=not-hex", "maxResults=250"] {
            let (status, body) = list_raw(&router, name, query).await;
            assert_eq!(
                status, 404,
                "workspace {name:?} ?{query}: expected 404, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "workspace {name:?} ?{query}: unexpected error code in {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_summary_is_the_stored_task_a_read_of_that_task_reports() {
    let router = router();
    fixture(&router).await;
    let created = create_task_cases(&router, WORKSPACE).await;

    // Read straight after being created, every task still owes the transition the
    // clock makes due, so the listing reports the transient state rather than a
    // task that was never pending.
    let fresh = listed(&router, WORKSPACE, "maxResults=250").await;
    for summary in summaries(&fresh) {
        let name = summary["taskName"].as_str().unwrap_or_default();
        assert_eq!(
            summary["status"]["state"].as_str(),
            Some("CREATING"),
            "case {name}: a task read straight after being created is still settling, \
             got {summary}"
        );
    }

    // A listing REPORTS: two listings of one unchanged store agree, and neither
    // moves a task on.
    let again = listed(&router, WORKSPACE, "maxResults=250").await;
    assert_eq!(
        names(&again),
        names(&fresh),
        "two listings of one store must report the same tasks in the same order"
    );

    tokio::time::sleep(ACTIVATION_DELAY + std::time::Duration::from_millis(200)).await;

    let settled = listed(&router, WORKSPACE, "maxResults=250").await;
    assert_eq!(
        summaries(&settled).len(),
        created.len(),
        "every created task is listed once, got {settled}"
    );
    for summary in summaries(&settled) {
        let name = summary["taskName"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            summary["taskArn"].as_str(),
            Some(expected_arn(WORKSPACE, &name).as_str()),
            "case {name}: the summary reports the ARN the task was minted with, got {summary}"
        );
        assert_eq!(
            summary["version"].as_str(),
            Some(FIRST_TASK_VERSION.to_string().as_str()),
            "case {name}: a task no update has touched is at its first version, got {summary}"
        );
        assert_eq!(
            summary["status"]["state"].as_str(),
            Some("ACTIVE"),
            "case {name}: the transition the create owed has fallen due, got {summary}"
        );

        // The same task, read on its own: a summary and a read of one stored task
        // cannot disagree about any member they share.
        let (status, read) = call(&router, "GET", &task_uri(WORKSPACE, &name), None).await;
        assert_eq!(status, 200, "case {name}: describe got {status} {read}");
        for member in [
            "taskName",
            "taskArn",
            "version",
            "description",
            "createdAt",
            "updatedAt",
        ] {
            assert_eq!(
                summary[member], read[member],
                "case {name}: the summary's {member} {} disagrees with the read's {}",
                summary[member], read[member]
            );
        }
        assert_eq!(
            summary["status"], read["status"],
            "case {name}: the summary's status {} disagrees with the read's {}",
            summary["status"], read["status"]
        );
    }

    // An accepted write moves what the listing reports for that task and nothing
    // else: the version it minted and the instant it landed.
    let updated_name = &created[0];
    let (status, updated) = call(
        &router,
        "PUT",
        &task_uri(WORKSPACE, updated_name),
        Some(json!({
            "description": "the description the update wrote",
            "taskConfiguration":
                minimal_container(PUBLIC_IMAGE, PROCESSING_TYPES[1], PROCESSING_UNITS[2]),
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "update {updated_name}: expected 200, got {status} {updated}"
    );

    let after = listed(&router, WORKSPACE, "maxResults=250").await;
    for summary in summaries(&after) {
        let name = summary["taskName"].as_str().unwrap_or_default();
        let expected_version = if name == updated_name {
            FIRST_TASK_VERSION + 1
        } else {
            FIRST_TASK_VERSION
        };
        assert_eq!(
            summary["version"].as_str(),
            Some(expected_version.to_string().as_str()),
            "case {name}: expected version {expected_version} after the update, got {summary}"
        );
        if name == updated_name {
            assert_eq!(
                summary["description"].as_str(),
                Some("the description the update wrote"),
                "case {name}: the summary reports the description the update wrote, got {summary}"
            );
            let created_at = summary["createdAt"].as_f64().unwrap_or(-1.0);
            let updated_at = summary["updatedAt"].as_f64().unwrap_or(-1.0);
            assert!(
                updated_at >= created_at && created_at > 0.0,
                "case {name}: updatedAt {updated_at} precedes createdAt {created_at}"
            );
        }
    }
}

#[tokio::test]
async fn pagination_walks_one_stable_enumeration_whatever_the_page_size() {
    let router = router();
    fixture(&router).await;
    let created = create_task_cases(&router, WORKSPACE).await;
    let total = created.len();
    assert!(
        total < DEFAULT_MAX_RESULTS,
        "the fixture has to be shorter than the default page size, got {total}"
    );

    // The whole listing in one page is the order every paged walk has to reproduce.
    let baseline = names(&listed(&router, WORKSPACE, "maxResults=250").await);
    assert_eq!(
        baseline.len(),
        total,
        "the unpaged listing holds every created task"
    );

    // Rows: a page smaller than the listing, sizes that divide it and sizes that do
    // not, exactly the listing, more than it, the published maximum, and no
    // `maxResults` at all.
    let cases: [Option<i64>; 7] = [
        Some(1),
        Some(2),
        Some(3),
        Some(total as i64),
        Some(total as i64 + 1),
        Some(MAX_RESULTS_MAX),
        None,
    ];
    for page_size in cases {
        let case = format!("maxResults={page_size:?}");
        let (walked, pages) = walk(&router, WORKSPACE, page_size, total).await;
        assert_eq!(
            walked, baseline,
            "case {case:?}: the pages joined must be the whole listing once, in one order, \
             got {walked:?}"
        );
        let expected_pages = match page_size {
            Some(size) => total.div_ceil(size as usize),
            None => 1,
        };
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, walked {pages}"
        );
    }

    // The token a page carries names the task the NEXT page starts at, in the form
    // this listing issues: the hex of that task's name, built by an oracle that does
    // not share the handler's encoder.
    let first = listed(&router, WORKSPACE, "maxResults=1").await;
    assert_eq!(
        first["nextToken"].as_str(),
        Some(hex_token(&baseline[1]).as_str()),
        "the first page's token resumes at the second task, got {first}"
    );

    // A token this listing never issued is refused rather than answered with an
    // empty page: unhexable text, hex of a name no task of this workspace carries,
    // and hex of the name of a task of ANOTHER workspace.
    create_task(
        &router,
        OTHER_WORKSPACE,
        "only-in-plant2",
        None,
        minimal_container(PRIVATE_IMAGE, PROCESSING_TYPES[0], PROCESSING_UNITS[0]),
    )
    .await;
    for (case, token) in [
        ("not a token this service issues", "not-hex".to_string()),
        ("hex of an unknown name", hex_token("no-such-task")),
        (
            "hex of another workspace's task",
            hex_token("only-in-plant2"),
        ),
    ] {
        let (status, body) = list_raw(&router, WORKSPACE, &format!("nextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {token}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // `maxResults` carries @range 1..250 (smithy_query query="operation"
    // operation="ListTasks"): each published bound is served and one past it on
    // either side is refused.
    for (case, max_results, served) in [
        ("the range minimum", MAX_RESULTS_MIN, true),
        ("the range maximum", MAX_RESULTS_MAX, true),
        ("one under the minimum", MAX_RESULTS_MIN - 1, false),
        ("one past the maximum", MAX_RESULTS_MAX + 1, false),
    ] {
        let (status, body) =
            list_raw(&router, WORKSPACE, &format!("maxResults={max_results}")).await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?} (maxResults={max_results}): an in-range page size must be served, \
                 got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?} (maxResults={max_results}): an out-of-range page size must be \
                 refused, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case {case:?} (maxResults={max_results}): unexpected error code in {body}"
            );
        }
    }
}
