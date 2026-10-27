// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListProjects`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rule these suites pin: `ListProjects` enumerates EVERY project the service's
//! state holds IN the portal the request names — and no project of any other portal
//! — each exactly once, in ONE stable order, cut into pages of at most `maxResults`
//! entries, where `nextToken` is present only while another entry follows and
//! resumes the enumeration exactly where the previous page stopped; a listed project
//! carries the same values `DescribeProject` reports for it, a `nextToken` the
//! operation never issued is refused rather than answered with an empty page, and a
//! `portalId` this service holds no portal for is refused rather than answered with
//! an empty page.
//!
//! The HTTP harness (`call`, `error_code`, `hex_token`, `is_modelled_id`) is the one
//! the asset suite already has, the portal helper the access-policy suite has, the
//! create-request body the project suite has, and the error-message reader the
//! refusal suite has; this suite reuses all of them rather than growing a second
//! copy.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::access_policy_lifecycle::create_portal;
use crate::asset_lifecycle::{call, error_code, hex_token, is_modelled_id, router};
use crate::hierarchies_etags_and_refusals::error_message;
use crate::project_lifecycle::{
    create_body, DESCRIPTION_MAX_LENGTH, NAME_MAX_LENGTH, PROJECTS,
};

/// A well-formed `ID` UUID this suite never stores a portal under.
const UNKNOWN_PORTAL_ID: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";

/// One row of the fixture: the project's name beside the description it carries, if
/// any — the two caller-owned members a `ProjectSummary` reports beyond its id and
/// the two dates (smithy_query query="shape" shape="ProjectSummary").
struct ProjectCase {
    name: String,
    description: Option<String>,
}

/// The rows: nine projects differing along every dimension a `ProjectSummary`
/// reports — a description or none, at the published `Name` and `Description`
/// minimum and maximum lengths, non-ASCII text, and punctuation and spaces — so a
/// page boundary never lands only between two identically shaped projects.
fn project_cases() -> Vec<ProjectCase> {
    vec![
        ProjectCase {
            name: "listed-a".to_string(),
            description: None,
        },
        ProjectCase {
            name: "listed-b".to_string(),
            description: Some("the floor-one project".to_string()),
        },
        // The published minima: `Name` and `Description` are both len 1..
        ProjectCase {
            name: "p".to_string(),
            description: Some("d".to_string()),
        },
        // The published maxima, from `project_lifecycle`'s record of the model.
        ProjectCase {
            name: "n".repeat(NAME_MAX_LENGTH),
            description: Some("d".repeat(DESCRIPTION_MAX_LENGTH)),
        },
        ProjectCase {
            name: "проект-кипятильник".to_string(),
            description: Some("описание на кириллице".to_string()),
        },
        ProjectCase {
            name: "listed f, with spaces".to_string(),
            description: Some("a description, with punctuation: and more!".to_string()),
        },
        ProjectCase {
            name: "listed-g".to_string(),
            description: Some("the seventh project".to_string()),
        },
        ProjectCase {
            name: "listed-h".to_string(),
            description: None,
        },
        ProjectCase {
            name: "listed-i".to_string(),
            description: Some("the ninth project".to_string()),
        },
    ]
}

/// `GET /projects?portalId=...`, plus whatever paging members a row carries.
fn list_uri(portal_id: &str, query: &[String]) -> String {
    let mut parameters = vec![format!("portalId={portal_id}")];
    parameters.extend(query.iter().cloned());
    format!("{PROJECTS}?{}", parameters.join("&"))
}

/// Create one project in `portal_id` and return its id.
async fn create_project(
    router: &MultiServiceRouter,
    portal_id: &str,
    case: &ProjectCase,
) -> String {
    let body = create_body(portal_id, &case.name, case.description.as_deref());

    let (status, created) = call(router, "POST", PROJECTS, Some(body)).await;
    assert_eq!(
        status, 201,
        "case {:?}: CreateProject expected 201, got {status} {created}",
        case.name
    );
    let id = created["projectId"].as_str().unwrap_or("").to_string();
    assert!(
        is_modelled_id(&id),
        "case {:?}: projectId must be a modelled UUID, got {created}",
        case.name
    );
    id
}

/// Create a portal holding the whole fixture, and a SECOND portal holding projects
/// of its own, so every assertion below also shows the enumeration is scoped to the
/// portal the request names. Returns `(portal, its project ids in creation order,
/// other portal, that portal's project ids)`.
async fn create_fixture(router: &MultiServiceRouter) -> (String, Vec<String>, String, Vec<String>) {
    let portal_id = create_portal(router, "list-projects-portal").await;
    let mut ids = Vec::new();
    for case in project_cases() {
        ids.push(create_project(router, &portal_id, &case).await);
    }

    let other_portal_id = create_portal(router, "list-projects-other-portal").await;
    let mut other_ids = Vec::new();
    for name in ["other-a", "other-b", "other-c"] {
        let case = ProjectCase {
            name: name.to_string(),
            description: Some(format!("a project of the other portal: {name}")),
        };
        other_ids.push(create_project(router, &other_portal_id, &case).await);
    }

    (portal_id, ids, other_portal_id, other_ids)
}

/// Walk the whole enumeration of `portal_id` at `page_size` (or with no `maxResults`
/// at all when `page_size` is `None`), returning the ids in the order the pages
/// reported them.
async fn walk(
    router: &MultiServiceRouter,
    portal_id: &str,
    page_size: Option<usize>,
    total: usize,
) -> Vec<String> {
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

        let (status, body) = call(router, "GET", &list_uri(portal_id, &query), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: page {pages} expected 200, got {status} {body}"
        );
        let summaries = body["projectSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: projectSummaries must be a list, got {body}"));
        // The documented default when the caller sends none is 50.
        let limit = page_size.unwrap_or(50);
        assert!(
            summaries.len() <= limit,
            "case {case:?}: a page holds at most {limit}, got {} in {body}",
            summaries.len()
        );
        for summary in summaries {
            seen.push(
                summary["id"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: a summary without an id: {summary}"))
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
            // additional results": a token is issued only while the page it resumes
            // would hold something.
            Some(_) => assert_eq!(
                summaries.len(),
                limit,
                "case {case:?}: a page followed by a nextToken must be full, got {body}"
            ),
            None => break,
        }
    }

    let expected_pages = total.div_ceil(page_size.unwrap_or(50)).max(1);
    assert_eq!(
        pages, expected_pages,
        "case {case:?}: expected {expected_pages} pages over {total} projects, walked {pages}"
    );
    seen
}

#[tokio::test]
async fn list_projects_pages_through_every_project_of_the_named_portal_once() {
    let router = router();
    let (portal_id, created, other_portal_id, other_created) = create_fixture(&router).await;
    let total = created.len();

    // The reference order: the enumeration one unpaged call reports.
    let reference = walk(&router, &portal_id, Some(250), total).await;
    let mut sorted_reference = reference.clone();
    sorted_reference.sort();
    let mut sorted_created = created.clone();
    sorted_created.sort();
    assert_eq!(
        sorted_reference, sorted_created,
        "the enumeration must cover exactly the projects of the portal it names, saw {reference:?}"
    );

    // Rows differ in page size: the published `maxResults` bounds (1 and 250), sizes
    // that divide the fixture and sizes that do not, a size past the fixture, and no
    // `maxResults` at all (the documented default of 50).
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
        let seen = walk(&router, &portal_id, page_size, total).await;
        assert_eq!(
            seen, reference,
            "case {page_size:?}: paging must report the one stable order, got {seen:?}"
        );
    }

    // The other portal's own enumeration: scoped the same way, so neither portal's
    // listing reports a project of the other.
    let other_seen = walk(&router, &other_portal_id, Some(250), other_created.len()).await;
    let mut sorted_other_seen = other_seen.clone();
    sorted_other_seen.sort();
    let mut sorted_other_created = other_created.clone();
    sorted_other_created.sort();
    assert_eq!(
        sorted_other_seen, sorted_other_created,
        "the other portal's listing must cover exactly its own projects, saw {other_seen:?}"
    );
}

#[tokio::test]
async fn a_listed_project_carries_what_describe_project_reports() {
    let router = router();
    let (portal_id, created, _, _) = create_fixture(&router).await;

    let (status, body) = call(
        &router,
        "GET",
        &list_uri(&portal_id, &["maxResults=250".to_string()]),
        None,
    )
    .await;
    assert_eq!(status, 200, "ListProjects expected 200, got {status} {body}");
    let summaries = body["projectSummaries"]
        .as_array()
        .expect("projectSummaries");
    assert_eq!(
        summaries.len(),
        created.len(),
        "every created project must be listed, got {body}"
    );

    // Each `ProjectSummary` member beside the `DescribeProject` member that reports
    // the same value (smithy_query query="shape" shape="ProjectSummary").
    let members = [
        ("id", "projectId"),
        ("name", "projectName"),
        ("description", "projectDescription"),
        ("creationDate", "projectCreationDate"),
        ("lastUpdateDate", "projectLastUpdateDate"),
    ];
    for summary in summaries {
        let id = summary["id"].as_str().unwrap_or("");
        let (status, described) = call(&router, "GET", &format!("{PROJECTS}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {id:?}: DescribeProject expected 200, got {status} {described}"
        );
        for (listed, describes) in members {
            assert_eq!(
                summary[listed], described[describes],
                "case {id:?}: the summary's {listed} must be what DescribeProject reports as \
                 {describes}: {summary} vs {described}"
            );
        }
        assert_eq!(
            described["portalId"].as_str().unwrap_or(""),
            portal_id,
            "case {id:?}: a listed project must live in the portal the request named, got \
             {described}"
        );
    }
}

#[tokio::test]
async fn listing_projects_does_not_change_what_a_later_caller_sees() {
    let router = router();

    // A portal with no project reports an empty list and no token, rather than a
    // fabricated entry or a token that resumes nothing.
    let empty_portal_id = create_portal(&router, "list-projects-empty-portal").await;
    let (status, empty) = call(&router, "GET", &list_uri(&empty_portal_id, &[]), None).await;
    assert_eq!(
        status, 200,
        "an empty portal expected 200, got {status} {empty}"
    );
    assert_eq!(
        empty["projectSummaries"],
        json!([]),
        "an empty portal lists no project, got {empty}"
    );
    assert!(
        empty["nextToken"].is_null(),
        "an empty portal issues no nextToken, got {empty}"
    );

    let (portal_id, _, _, _) = create_fixture(&router).await;

    // Rows differ in page size; each read twice. A read never moves the enumeration.
    for page_size in [1_usize, 5, 250] {
        let uri = list_uri(&portal_id, &[format!("maxResults={page_size}")]);
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
    let (portal_id, created, other_portal_id, other_created) = create_fixture(&router).await;

    // Rows differ in HOW the token is wrong: not hex at all, hex of a well-formed id
    // no project has, hex of a string that is no id, an odd-length hex run, and the
    // hex of a project of ANOTHER portal — which is not in this enumeration, so it
    // resumes nothing here.
    let forged = [
        ("not hex at all", "zzzz".to_string()),
        ("hex of an unknown id", hex_token(UNKNOWN_PORTAL_ID)),
        ("hex of something that is no id", hex_token("listed-a")),
        ("odd-length hex", "abc".to_string()),
        (
            "hex of a project of another portal",
            hex_token(&other_created[0]),
        ),
    ];
    for (case, token) in forged {
        let uri = list_uri(&portal_id, &[format!("nextToken={token}")]);
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

    // The refusals left both enumerations intact.
    let seen = walk(&router, &portal_id, Some(250), created.len()).await;
    assert_eq!(
        seen.len(),
        created.len(),
        "the refusals must not have changed the enumeration, saw {seen:?}"
    );
    let other_seen = walk(&router, &other_portal_id, Some(250), other_created.len()).await;
    assert_eq!(
        other_seen.len(),
        other_created.len(),
        "the refusals must not have changed the other portal's enumeration, saw {other_seen:?}"
    );
}

#[tokio::test]
async fn a_portal_id_that_resolves_to_no_stored_portal_is_refused() {
    let router = router();
    let (portal_id, created, _, _) = create_fixture(&router).await;

    // Rows differ in WHICH well-formed id is unknown, and in the paging members the
    // request carries beside it: the absence of the portal is what is refused, not
    // the shape of the rest of the request. The expected message is built from the
    // row's own identifier, so no row can pass by matching a hard-coded id.
    let unknown = [
        ("an unknown portal, no paging members", UNKNOWN_PORTAL_ID, vec![]),
        (
            "an unknown portal with maxResults",
            "11111111-2222-4333-8444-555555555555",
            vec!["maxResults=10".to_string()],
        ),
        (
            "an unknown portal with a token",
            "99999999-8888-4777-8666-555555555555",
            vec![format!("nextToken={}", hex_token(&created[0]))],
        ),
    ];
    for (case, unknown_portal_id, query) in unknown {
        let (status, body) = call(&router, "GET", &list_uri(unknown_portal_id, &query), None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: an unknown portalId must be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
        // The wording every not-found refusal in this service carries, written out
        // here by hand as an independent oracle.
        assert_eq!(
            error_message(&body),
            format!("Portal with ID {unknown_portal_id} not found"),
            "case {case:?}: unexpected message in {body}"
        );
    }

    // The refusals wrote nothing: the known portal still enumerates its projects.
    let seen = walk(&router, &portal_id, Some(250), created.len()).await;
    assert_eq!(
        seen.len(),
        created.len(),
        "the refusals must not have changed the enumeration, saw {seen:?}"
    );
}

#[tokio::test]
async fn max_results_outside_the_modelled_range_is_refused() {
    let router = router();
    let (portal_id, _, _, _) = create_fixture(&router).await;

    // `maxResults` carries @range 1..250 (smithy_query query="operation"
    // operation="ListProjects"): each published bound is served and one past it on
    // either side is refused.
    let cases: [(&str, i64, bool); 4] = [
        ("the range minimum", 1, true),
        ("the range maximum", 250, true),
        ("one under the minimum", 0, false),
        ("one past the maximum", 251, false),
    ];
    for (case, max_results, served) in cases {
        let uri = list_uri(&portal_id, &[format!("maxResults={max_results}")]);
        let (status, body) = call(&router, "GET", &uri, None).await;
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
        }
    }
}
