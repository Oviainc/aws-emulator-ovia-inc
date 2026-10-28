// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListWorkspaces`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rule these suites pin: `ListWorkspaces` enumerates EVERY workspace the
//! service's state holds, each exactly once, in ONE stable order, cut into pages
//! of at most `maxResults` entries, where `nextToken` is present only while
//! another entry follows and resumes the enumeration exactly where the previous
//! page stopped; a listed workspace carries the values `DescribeWorkspace`
//! reports for it — including the status the clock owes it — and a `nextToken`
//! the operation never issued is refused rather than answered with an empty page.
//!
//! The HTTP harness (`call`, `error_code`, `hex_token`, `router`) is the one the
//! asset suite already has and the workspace steps (`create`, `describe`,
//! `default_encryption`, `kms_encryption`, `settle`, `expected_arn`) are the
//! workspace-lifecycle suite's; this suite reuses both rather than growing a
//! second copy.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, error_code, hex_token, router};
use crate::workspace_lifecycle::{
    create, default_encryption, describe, expected_arn, kms_encryption, settle,
};

/// Where every workspace listing goes, as `OPERATIONS.md` reports the route:
/// `GET /workspaces`.
const WORKSPACES: &str = "/workspaces";

/// The page size this operation's `maxResults` documents when a caller sends
/// none (smithy_query query="doc" shape="ListWorkspacesRequest"
/// member="maxResults": "Default: 50").
const DEFAULT_MAX_RESULTS: usize = 50;

/// One row of the fixture: the workspace's name beside the request members it
/// carries beyond the required ones, so the enumeration spans workspaces of
/// several shapes.
struct WorkspaceCase {
    label: &'static str,
    name: String,
    extra: Value,
}

/// A name of exactly `length` characters within `WorkspaceName`'s pattern,
/// prefixed with `seed` so each row is distinct.
///
/// `WorkspaceName` carries @length(min: 1, max: 64) and pattern
/// `^[a-zA-Z0-9_-]+$` (smithy_query query="operation"
/// operation="ListWorkspaces" … "CreateWorkspace"), so the rows below span each
/// published bound of that length.
fn name_of_length(seed: &str, length: usize) -> String {
    let mut name = seed.to_string();
    while name.len() < length {
        name.push('x');
    }
    name.truncate(length);
    name
}

/// The rows: nine workspaces differing along every dimension a
/// `WorkspaceSummary` reports or a create can vary — a description or none, each
/// `EncryptionType` value (`SITEWISE_DEFAULT_ENCRYPTION` | `KMS_BASED_ENCRYPTION`,
/// the enum's whole value list: smithy_query query="shape" shape="EncryptionType"),
/// tags or none, each published bound of the name's length, and names mixing every
/// character class the name's pattern allows — so a page boundary never lands only
/// between two identically shaped workspaces.
fn workspace_cases() -> Vec<WorkspaceCase> {
    vec![
        WorkspaceCase {
            label: "the shortest name the length admits",
            name: "a".to_string(),
            extra: json!({}),
        },
        WorkspaceCase {
            label: "the longest name the length admits",
            name: name_of_length("listed-longest", 64),
            extra: json!({ "workspaceDescription": "the widest name" }),
        },
        WorkspaceCase {
            label: "a name one under the longest",
            name: name_of_length("listed-almost", 63),
            extra: json!({}),
        },
        WorkspaceCase {
            label: "underscores and digits",
            name: "listed_workspace_42".to_string(),
            extra: json!({ "workspaceDescription": "the floor-one workspace" }),
        },
        WorkspaceCase {
            label: "mixed case and hyphens",
            name: "Listed-Workspace-B".to_string(),
            extra: json!({ "tags": { "stage": "test" } }),
        },
        WorkspaceCase {
            label: "a customer managed key",
            name: "listed-kms-key".to_string(),
            extra: json!({
                "encryptionConfiguration": kms_encryption("1111abcd-12ab-34cd-56ef-1234567890ab"),
            }),
        },
        WorkspaceCase {
            label: "a customer managed key and a description",
            name: "listed-kms-described".to_string(),
            extra: json!({
                "encryptionConfiguration": kms_encryption("alias/sitewise-listed"),
                "workspaceDescription": "the key-bearing workspace",
            }),
        },
        WorkspaceCase {
            label: "a digit-only name",
            name: "20261024".to_string(),
            extra: json!({ "tags": { "owner": "plant-two" } }),
        },
        WorkspaceCase {
            label: "a description and tags together",
            name: "listed-ninth".to_string(),
            extra: json!({
                "workspaceDescription": "the ninth workspace",
                "tags": { "stage": "test", "owner": "plant-one" },
            }),
        },
    ]
}

/// Create one workspace and return its name.
async fn create_workspace(router: &MultiServiceRouter, case: &WorkspaceCase) -> String {
    let mut body = json!({
        "workspaceName": case.name,
        "encryptionConfiguration": default_encryption(),
    });
    let object = body.as_object_mut().expect("an object body");
    for (member, value) in case.extra.as_object().expect("an object of extras") {
        object.insert(member.clone(), value.clone());
    }

    let (status, created) = create(router, body).await;
    assert_eq!(
        status, 200,
        "case {:?}: CreateWorkspace expected 200, got {status} {created}",
        case.label
    );
    assert_eq!(
        created["workspaceArn"],
        json!(expected_arn(&case.name)),
        "case {:?}: the create must mint the workspace's own ARN, got {created}",
        case.label
    );
    case.name.clone()
}

/// Create the whole fixture and return the names, in creation order.
async fn create_fixture(router: &MultiServiceRouter) -> Vec<String> {
    let mut names = Vec::new();
    for case in workspace_cases() {
        names.push(create_workspace(router, &case).await);
    }
    names
}

/// Walk the whole enumeration at `page_size` (or with no `maxResults` at all when
/// `page_size` is `None`), returning the names in the order the pages reported
/// them.
async fn walk(router: &MultiServiceRouter, page_size: Option<usize>, total: usize) -> Vec<String> {
    let case = match page_size {
        Some(size) => format!("maxResults={size}"),
        None => "no maxResults".to_string(),
    };
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0_usize;
    loop {
        let mut query: Vec<String> = Vec::new();
        if let Some(size) = page_size {
            query.push(format!("maxResults={size}"));
        }
        if let Some(token) = &token {
            query.push(format!("nextToken={token}"));
        }
        let uri = if query.is_empty() {
            WORKSPACES.to_string()
        } else {
            format!("{WORKSPACES}?{}", query.join("&"))
        };

        let (status, body) = call(router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: page {pages} expected 200, got {status} {body}"
        );
        let summaries = body["workspaceSummaries"].as_array().unwrap_or_else(|| {
            panic!("case {case:?}: workspaceSummaries must be a list, got {body}")
        });
        let limit = page_size.unwrap_or(DEFAULT_MAX_RESULTS);
        assert!(
            summaries.len() <= limit,
            "case {case:?}: a page holds at most {limit}, got {} in {body}",
            summaries.len()
        );
        for summary in summaries {
            seen.push(
                summary["name"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: a summary without a name: {summary}")
                    })
                    .to_string(),
            );
        }
        pages += 1;
        assert!(
            pages <= total + 1,
            "case {case:?}: paging did not terminate after {pages} pages"
        );

        token = body["nextToken"].as_str().map(str::to_string);
        match &token {
            // "The token for the next set of results, or null if there are no
            // additional results": a token is issued only while the page it
            // resumes would hold something.
            Some(_) => assert_eq!(
                summaries.len(),
                limit,
                "case {case:?}: a page followed by a nextToken must be full, got {body}"
            ),
            None => break,
        }
    }

    let expected_pages = total
        .div_ceil(page_size.unwrap_or(DEFAULT_MAX_RESULTS))
        .max(1);
    assert_eq!(
        pages, expected_pages,
        "case {case:?}: expected {expected_pages} pages over {total} workspaces, walked {pages}"
    );
    seen
}

#[tokio::test]
async fn list_workspaces_pages_through_every_stored_workspace_once() {
    let router = router();
    let created = create_fixture(&router).await;
    let total = created.len();

    // The reference order: the enumeration one unpaged call reports.
    let reference = walk(&router, Some(250), total).await;
    let mut sorted_reference = reference.clone();
    sorted_reference.sort();
    let mut sorted_created = created.clone();
    sorted_created.sort();
    assert_eq!(
        sorted_reference, sorted_created,
        "the enumeration must cover exactly the created workspaces, saw {reference:?}"
    );

    // Rows differ in page size: the published `maxResults` bounds (1 and 250),
    // sizes that divide the fixture and sizes that do not, a size past the
    // fixture, and no `maxResults` at all (the documented default of 50).
    let page_sizes = [
        Some(1_usize),
        Some(2),
        Some(3),
        Some(4),
        Some(9),
        Some(10),
        Some(250),
        None,
    ];
    for page_size in page_sizes {
        let seen = walk(&router, page_size, total).await;
        assert_eq!(
            seen, reference,
            "case {page_size:?}: paging must report the one stable order, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_listed_workspace_carries_what_describe_workspace_reports() {
    let router = router();
    let created = create_fixture(&router).await;

    // The status a workspace reports is the one the clock owes it, so the
    // comparison is made once every workspace has settled: otherwise the
    // transition could fall due BETWEEN the list and the describe and the two
    // would legitimately disagree.
    tokio::time::sleep(settle()).await;

    let (status, body) = call(&router, "GET", &format!("{WORKSPACES}?maxResults=250"), None).await;
    assert_eq!(
        status, 200,
        "ListWorkspaces expected 200, got {status} {body}"
    );
    let summaries = body["workspaceSummaries"]
        .as_array()
        .expect("workspaceSummaries");
    assert_eq!(
        summaries.len(),
        created.len(),
        "every created workspace must be listed, got {body}"
    );

    // Each `WorkspaceSummary` member beside the `DescribeWorkspace` member that
    // reports the same value (smithy_query query="shape"
    // shape="WorkspaceSummary").
    let members = [
        ("name", "workspaceName"),
        ("arn", "workspaceArn"),
        ("status", "workspaceStatus"),
        ("createdAt", "createdAt"),
        ("updatedAt", "updatedAt"),
    ];
    for summary in summaries {
        let name = summary["name"].as_str().unwrap_or("");
        let (status, described) = describe(&router, name).await;
        assert_eq!(
            status, 200,
            "case {name:?}: DescribeWorkspace expected 200, got {status} {described}"
        );
        for (listed, describes) in members {
            assert_eq!(
                summary[listed], described[describes],
                "case {name:?}: the summary's {listed} must be what DescribeWorkspace reports \
                 as {describes}: {summary} vs {described}"
            );
        }
        assert_eq!(
            summary["status"]["state"],
            json!("ACTIVE"),
            "case {name:?}: a settled workspace lists as ACTIVE, got {summary}"
        );
    }
}

#[tokio::test]
async fn a_listing_reports_the_state_the_last_write_left() {
    let router = router();
    // Two workspaces, so the listing has to report each one's own state rather
    // than one state for the page.
    for name in ["updated-workspace", "untouched-workspace"] {
        let (status, created) = create(
            &router,
            json!({
                "workspaceName": name,
                "encryptionConfiguration": default_encryption(),
            }),
        )
        .await;
        assert_eq!(status, 200, "create {name}: got {status} {created}");
    }
    tokio::time::sleep(settle()).await;

    let before = listed(&router).await;

    // An accepted update "moves the workspace to UPDATING until the transition
    // the clock owes falls due", and the listing must report that state and the
    // instant of the write rather than a snapshot taken at create time.
    let (status, updated) = call(
        &router,
        "PUT",
        &format!("{WORKSPACES}/updated-workspace"),
        Some(json!({ "workspaceDescription": "the updated workspace" })),
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdateWorkspace expected 200, got {status} {updated}"
    );

    let during = listed(&router).await;
    let updated_row = summary_of(&during, "updated-workspace");
    assert_eq!(
        updated_row["status"]["state"],
        json!("UPDATING"),
        "case \"a workspace settling an update\": the listing must report UPDATING, got \
         {updated_row}"
    );
    assert_eq!(
        updated_row["createdAt"],
        summary_of(&before, "updated-workspace")["createdAt"],
        "case \"a workspace settling an update\": an update leaves createdAt alone, got \
         {updated_row}"
    );
    assert!(
        epoch(&updated_row, "updatedAt") > epoch(&summary_of(&before, "updated-workspace"), "updatedAt"),
        "case \"a workspace settling an update\": updatedAt must move with the write, got \
         {updated_row}"
    );

    let untouched = summary_of(&during, "untouched-workspace");
    assert_eq!(
        untouched, summary_of(&before, "untouched-workspace"),
        "case \"a workspace no call touched\": its summary must be unchanged, got {untouched}"
    );

    // Once the transition the update owes falls due, the listing reports it.
    tokio::time::sleep(settle()).await;
    let after = listed(&router).await;
    assert_eq!(
        summary_of(&after, "updated-workspace")["status"]["state"],
        json!("ACTIVE"),
        "case \"a settled update\": the listing must report ACTIVE, got {after:?}"
    );
}

/// Every workspace one unpaged listing reports.
async fn listed(router: &MultiServiceRouter) -> Vec<Value> {
    let (status, body) = call(router, "GET", &format!("{WORKSPACES}?maxResults=250"), None).await;
    assert_eq!(
        status, 200,
        "ListWorkspaces expected 200, got {status} {body}"
    );
    body["workspaceSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("workspaceSummaries must be a list, got {body}"))
        .clone()
}

/// The one summary of `name` in a listing.
fn summary_of(summaries: &[Value], name: &str) -> Value {
    let mut found = summaries
        .iter()
        .filter(|summary| summary["name"] == json!(name));
    let summary = found
        .next()
        .unwrap_or_else(|| panic!("case {name:?}: the workspace must be listed, got {summaries:?}"))
        .clone();
    assert!(
        found.next().is_none(),
        "case {name:?}: a workspace is listed exactly once, got {summaries:?}"
    );
    summary
}

/// The epoch seconds a `Timestamp` member of a summary carries.
fn epoch(summary: &Value, member: &str) -> f64 {
    summary[member]
        .as_f64()
        .unwrap_or_else(|| panic!("{member} is not an epoch-seconds timestamp in {summary}"))
}

#[tokio::test]
async fn listing_workspaces_does_not_change_what_a_later_caller_sees() {
    let router = router();

    // A store with no workspace reports an empty list and no token, rather than a
    // fabricated entry or a token that resumes nothing.
    let (status, empty) = call(&router, "GET", WORKSPACES, None).await;
    assert_eq!(
        status, 200,
        "an empty store expected 200, got {status} {empty}"
    );
    assert_eq!(
        empty["workspaceSummaries"],
        json!([]),
        "an empty store lists no workspace, got {empty}"
    );
    assert!(
        empty["nextToken"].is_null(),
        "an empty store issues no nextToken, got {empty}"
    );

    let _ = create_fixture(&router).await;
    // Read past the owed CREATING -> ACTIVE transition, so a state the clock
    // makes due does not make the two reads below differ for a reason that is not
    // a defect.
    tokio::time::sleep(settle()).await;

    // Rows differ in page size; each read twice. A read never moves the
    // enumeration.
    for page_size in [1_usize, 5, 250] {
        let uri = format!("{WORKSPACES}?maxResults={page_size}");
        let (status, first) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case maxResults={page_size}: expected 200, got {status} {first}"
        );
        let (status, again) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case maxResults={page_size}: the second read expected 200, got {status} {again}"
        );
        assert_eq!(
            again, first,
            "case maxResults={page_size}: a read must not change what the next caller sees, \
             got {again} after {first}"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_operation_never_issued_is_refused() {
    let router = router();
    let created = create_fixture(&router).await;

    // Rows differ in HOW the token is wrong: not hex at all, hex of a
    // well-formed name no workspace carries, hex of text no name could be, and an
    // odd-length hex run.
    let forged = [
        ("not hex at all", "zzzz".to_string()),
        ("hex of an unknown name", hex_token("never-created")),
        ("hex of text that is no name", hex_token("not a workspace name")),
        ("odd-length hex", "abc".to_string()),
    ];
    for (case, token) in forged {
        let uri = format!("{WORKSPACES}?nextToken={token}");
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a forged nextToken must be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // The refusals left the enumeration intact.
    let seen = walk(&router, Some(250), created.len()).await;
    assert_eq!(
        seen.len(),
        created.len(),
        "the refusals must not have changed the enumeration, saw {seen:?}"
    );
}

#[tokio::test]
async fn a_next_token_resumes_exactly_where_the_page_stopped() {
    let router = router();
    let created = create_fixture(&router).await;
    let reference = walk(&router, Some(250), created.len()).await;

    // Rows differ in where the cut falls: the first entry, the middle, and the
    // last. The token a page issues is the hex of the resume point — `hex_token`
    // is the harness's independent encoder, so a token the handler minted and one
    // this suite built must agree.
    for taken in [1_usize, created.len() / 2, created.len() - 1] {
        let uri = format!("{WORKSPACES}?maxResults={taken}");
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case maxResults={taken}: expected 200, got {status} {body}"
        );
        assert_eq!(
            body["nextToken"],
            json!(hex_token(&reference[taken])),
            "case maxResults={taken}: the token must resume at the entry after the page, got \
             {body}"
        );

        let (status, resumed) = call(
            &router,
            "GET",
            &format!("{WORKSPACES}?maxResults=250&nextToken={}", hex_token(&reference[taken])),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case maxResults={taken}: resuming expected 200, got {status} {resumed}"
        );
        let rest: Vec<String> = resumed["workspaceSummaries"]
            .as_array()
            .expect("workspaceSummaries")
            .iter()
            .map(|summary| summary["name"].as_str().unwrap_or("").to_string())
            .collect();
        assert_eq!(
            rest,
            reference[taken..].to_vec(),
            "case maxResults={taken}: the resumed page must be the rest of the enumeration, \
             got {rest:?}"
        );
    }
}

#[tokio::test]
async fn max_results_outside_the_modelled_range_is_refused() {
    let router = router();
    let _ = create_fixture(&router).await;

    // `maxResults` carries @range 1..250 (smithy_query query="operation"
    // operation="ListWorkspaces"): each published bound is served and one past it
    // on either side is refused.
    let cases: [(&str, i64, bool); 4] = [
        ("the range minimum", 1, true),
        ("the range maximum", 250, true),
        ("one under the minimum", 0, false),
        ("one past the maximum", 251, false),
    ];
    for (case, max_results, served) in cases {
        let uri = format!("{WORKSPACES}?maxResults={max_results}");
        let (status, body) = call(&router, "GET", &uri, None).await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?} (maxResults={max_results}): an in-range page size must be \
                 served, got {status} {body}"
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
