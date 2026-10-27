// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListDashboards`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rule these suites pin: `ListDashboards` enumerates EVERY dashboard the
//! service's state holds for the project the request names and no other, each
//! exactly once, in ONE stable order, cut into pages of at most `maxResults`
//! entries, where `nextToken` is present only while another entry follows and
//! resumes the enumeration exactly where the previous page stopped; a listed
//! dashboard carries the same values `DescribeDashboard` reports for it, a
//! `nextToken` the operation never issued is refused rather than answered with an
//! empty page, and a `projectId` this store holds no project for is refused rather
//! than answered with an empty page.
//!
//! The HTTP harness (`call`, `error_code`, `hex_token`, `router`) is the one the
//! asset suite already has, and the project fixture is the one the project-assets
//! suite already has; this suite reuses both rather than growing a second copy.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{not_found_message, RESOURCE_PROJECT};

use crate::asset_lifecycle::{call, error_code, hex_token, router};
// The definition literal and the unknown-id builders are the ones the dashboard
// lifecycle suite already has; this suite imports them rather than growing a second
// copy of either.
use crate::dashboard_lifecycle::{definition_of, modelled_id};
use crate::project_assets::create_project;

/// Where every dashboard request goes, as `OPERATIONS.md` reports the routes:
/// `POST /dashboards`, `GET /dashboards?projectId=...`,
/// `GET /dashboards/{dashboardId}`.
const DASHBOARDS: &str = "/dashboards";

/// The page size this operation documents when the caller sends no `maxResults`.
/// Source: `smithy_query(query="operation", operation="ListDashboards")` —
/// `maxResults`, "Default: 50".
const DEFAULT_MAX_RESULTS: usize = 50;

/// One row of the fixture: which project the dashboard is created in, beside the
/// request members it carries past the required ones, so the enumeration spans
/// dashboards of several shapes.
struct DashboardCase {
    label: &'static str,
    /// The index into the fixture's projects this dashboard is created in.
    project: usize,
    extra: Value,
}

/// The rows: nine dashboards in the first project and three in the second,
/// differing along every dimension a `DashboardSummary` reports (smithy_query
/// query="shape" shape="DashboardSummary") — a description or none, a description
/// at its published floor of one character, a name of one character and a name of
/// 256 (the published bounds of `dashboardName`, len 1..256), definition literals
/// of several sizes, and tags or none — so a page boundary never lands only
/// between two identically shaped dashboards.
fn dashboard_cases() -> Vec<DashboardCase> {
    vec![
        DashboardCase {
            label: "plain",
            project: 0,
            extra: json!({}),
        },
        DashboardCase {
            label: "described",
            project: 0,
            extra: json!({ "dashboardDescription": "the floor-one dashboard" }),
        },
        DashboardCase {
            label: "one-character description",
            project: 0,
            extra: json!({ "dashboardDescription": "d" }),
        },
        DashboardCase {
            label: "one-character name",
            project: 0,
            extra: json!({ "dashboardName": "d" }),
        },
        DashboardCase {
            label: "name at its published maximum",
            project: 0,
            extra: json!({ "dashboardName": "n".repeat(256) }),
        },
        DashboardCase {
            label: "a larger definition",
            project: 0,
            extra: json!({ "dashboardDefinition": definition_of(4_096) }),
        },
        DashboardCase {
            label: "tagged",
            project: 0,
            extra: json!({ "tags": { "stage": "test" } }),
        },
        DashboardCase {
            label: "described and tagged",
            project: 0,
            extra: json!({
                "dashboardDescription": "an inventory view",
                "tags": { "stage": "test", "team": "ops" },
            }),
        },
        DashboardCase {
            label: "the ninth",
            project: 0,
            extra: json!({ "dashboardDescription": "the ninth dashboard" }),
        },
        DashboardCase {
            label: "other project, plain",
            project: 1,
            extra: json!({}),
        },
        DashboardCase {
            label: "other project, described",
            project: 1,
            extra: json!({ "dashboardDescription": "a dashboard of the other project" }),
        },
        DashboardCase {
            label: "other project, larger definition",
            project: 1,
            extra: json!({ "dashboardDefinition": definition_of(2_048) }),
        },
    ]
}

/// The create body one row sends: the required members, with the row's extras
/// layered over them.
fn create_body(case: &DashboardCase, project_id: &str) -> Value {
    let mut body = Map::new();
    body.insert(
        "projectId".to_string(),
        Value::String(project_id.to_string()),
    );
    body.insert(
        "dashboardName".to_string(),
        Value::String(format!("dashboard-{}", case.label)),
    );
    body.insert(
        "dashboardDefinition".to_string(),
        Value::String(definition_of(64)),
    );
    for (member, value) in case.extra.as_object().expect("an object of extras") {
        body.insert(member.clone(), value.clone());
    }
    Value::Object(body)
}

/// The fixture: three projects — two holding dashboards and one holding none — and
/// the dashboard ids the rows minted, grouped by the project they were created in.
struct Fixture {
    projects: Vec<String>,
    /// For each project, the ids created in it, in creation order.
    created: Vec<Vec<String>>,
}

/// Create the three projects and every row's dashboard.
async fn create_fixture(router: &MultiServiceRouter) -> Fixture {
    let mut projects = Vec::new();
    for at in 0..3 {
        projects.push(create_project(router, "list-dashboards", &format!("listed-project-{at}")).await);
    }
    let mut created: Vec<Vec<String>> = vec![Vec::new(); projects.len()];

    for case in dashboard_cases() {
        let project_id = &projects[case.project];
        let (status, body) = call(
            router,
            "POST",
            DASHBOARDS,
            Some(create_body(&case, project_id)),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {:?}: CreateDashboard expected 201, got {status} {body}",
            case.label
        );
        let id = body["dashboardId"].as_str().unwrap_or("").to_string();
        assert!(
            !id.is_empty(),
            "case {:?}: CreateDashboard must report a dashboardId, got {body}",
            case.label
        );
        created[case.project].push(id);
    }

    Fixture { projects, created }
}

/// Walk one project's whole enumeration at `page_size` (or with no `maxResults` at
/// all when `page_size` is `None`), returning the ids in the order the pages
/// reported them.
async fn walk(
    router: &MultiServiceRouter,
    project_id: &str,
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
        let mut query: Vec<String> = vec![format!("projectId={project_id}")];
        if let Some(size) = page_size {
            query.push(format!("maxResults={size}"));
        }
        if let Some(token) = &token {
            query.push(format!("nextToken={token}"));
        }
        let uri = format!("{DASHBOARDS}?{}", query.join("&"));

        let (status, body) = call(router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: page {pages} expected 200, got {status} {body}"
        );
        let summaries = body["dashboardSummaries"].as_array().unwrap_or_else(|| {
            panic!("case {case:?}: dashboardSummaries must be a list, got {body}")
        });
        let limit = page_size.unwrap_or(DEFAULT_MAX_RESULTS);
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

    let expected_pages = total.div_ceil(page_size.unwrap_or(DEFAULT_MAX_RESULTS)).max(1);
    assert_eq!(
        pages, expected_pages,
        "case {case:?}: expected {expected_pages} pages over {total} dashboards, walked {pages}"
    );
    seen
}

#[tokio::test]
async fn list_dashboards_pages_through_every_dashboard_of_the_named_project_once() {
    let router = router();
    let fixture = create_fixture(&router).await;

    // Rows differ in the project listed (one with nine dashboards, one with three,
    // one with none) and in page size: the published `maxResults` bounds (1 and
    // 250), sizes that divide the project's dashboards and sizes that do not, a size
    // past them, and no `maxResults` at all (the documented default of 50).
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
    for (at, project_id) in fixture.projects.iter().enumerate() {
        let expected = &fixture.created[at];
        let reference = walk(&router, project_id, Some(250), expected.len()).await;

        let mut sorted_reference = reference.clone();
        sorted_reference.sort();
        let mut sorted_expected = expected.clone();
        sorted_expected.sort();
        assert_eq!(
            sorted_reference, sorted_expected,
            "case project {at}: the enumeration must cover exactly the dashboards created in \
             that project, saw {reference:?}"
        );

        for page_size in page_sizes {
            let seen = walk(&router, project_id, page_size, expected.len()).await;
            assert_eq!(
                seen, reference,
                "case project {at} {page_size:?}: paging must report the one stable order, got \
                 {seen:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_listed_dashboard_carries_what_describe_dashboard_reports() {
    let router = router();
    let fixture = create_fixture(&router).await;

    // Each `DashboardSummary` member beside the `DescribeDashboard` member that
    // reports the same value (smithy_query query="shape" shape="DashboardSummary").
    let members = [
        ("id", "dashboardId"),
        ("name", "dashboardName"),
        ("description", "dashboardDescription"),
        ("creationDate", "dashboardCreationDate"),
        ("lastUpdateDate", "dashboardLastUpdateDate"),
    ];
    for (at, project_id) in fixture.projects.iter().enumerate() {
        let uri = format!("{DASHBOARDS}?projectId={project_id}&maxResults=250");
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case project {at}: ListDashboards expected 200, got {status} {body}"
        );
        let summaries = body["dashboardSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case project {at}: dashboardSummaries must be a list"));
        assert_eq!(
            summaries.len(),
            fixture.created[at].len(),
            "case project {at}: every dashboard created in the project must be listed, got {body}"
        );

        for summary in summaries {
            let id = summary["id"].as_str().unwrap_or("");
            let (status, described) =
                call(&router, "GET", &format!("{DASHBOARDS}/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {id:?}: DescribeDashboard expected 200, got {status} {described}"
            );
            assert_eq!(
                described["projectId"].as_str().unwrap_or(""),
                project_id.as_str(),
                "case {id:?}: a dashboard listed for one project must report it, got {described}"
            );
            for (listed, describes) in members {
                assert_eq!(
                    summary[listed], described[describes],
                    "case {id:?}: the summary's {listed} must be what DescribeDashboard reports \
                     as {describes}: {summary} vs {described}"
                );
            }
        }
    }
}

#[tokio::test]
async fn listing_dashboards_does_not_change_what_a_later_caller_sees() {
    let router = router();
    let fixture = create_fixture(&router).await;

    // The project no row created a dashboard in reports an empty list and no token,
    // rather than a fabricated entry or a token that resumes nothing.
    let empty_project = &fixture.projects[2];
    let (status, empty) = call(
        &router,
        "GET",
        &format!("{DASHBOARDS}?projectId={empty_project}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "a project holding no dashboard expected 200, got {status} {empty}"
    );
    assert_eq!(
        empty["dashboardSummaries"],
        json!([]),
        "a project holding no dashboard lists none, got {empty}"
    );
    assert!(
        empty["nextToken"].is_null(),
        "a project holding no dashboard issues no nextToken, got {empty}"
    );

    // Rows differ in the project listed and in page size; each read twice. A read
    // never moves the enumeration.
    for project_id in &fixture.projects {
        for page_size in [1_usize, 5, 250] {
            let uri = format!("{DASHBOARDS}?projectId={project_id}&maxResults={page_size}");
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
}

#[tokio::test]
async fn a_next_token_this_operation_never_issued_is_refused() {
    let router = router();
    let fixture = create_fixture(&router).await;
    let project_id = &fixture.projects[0];

    // Rows differ in HOW the token is wrong: not hex at all, hex of a well-formed id
    // no dashboard has, hex of a string that is no id, an odd-length hex run, and the
    // hex of a dashboard id that belongs to ANOTHER project — a resume point outside
    // the enumeration the request names.
    let forged = [
        ("not hex at all", "zzzz".to_string()),
        (
            "hex of an unknown id",
            hex_token("11111111-2222-3333-4444-555555555555"),
        ),
        ("hex of something that is no id", hex_token("listed-a")),
        ("odd-length hex", "abc".to_string()),
        (
            "hex of a dashboard of another project",
            hex_token(&fixture.created[1][0]),
        ),
    ];
    for (case, token) in forged {
        let uri = format!("{DASHBOARDS}?projectId={project_id}&nextToken={token}");
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
    let seen = walk(&router, project_id, Some(250), fixture.created[0].len()).await;
    assert_eq!(
        seen.len(),
        fixture.created[0].len(),
        "the refusals must not have changed the enumeration, saw {seen:?}"
    );
}

#[tokio::test]
async fn a_project_this_store_does_not_hold_is_refused() {
    let router = router();
    let fixture = create_fixture(&router).await;

    // Rows differ in the id that resolves to nothing, and in whether the request
    // carries a page size with it.
    let cases = [
        ("an unknown project", modelled_id(0xb0), None),
        ("another unknown project", modelled_id(0xb1), Some(1_usize)),
        ("a third unknown project", modelled_id(0xb2), Some(250)),
    ];
    for (case, project_id, page_size) in cases {
        let mut uri = format!("{DASHBOARDS}?projectId={project_id}");
        if let Some(size) = page_size {
            uri.push_str(&format!("&maxResults={size}"));
        }
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: an unknown project must be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
        assert_eq!(
            body["message"].as_str().unwrap_or(""),
            not_found_message(RESOURCE_PROJECT, &project_id),
            "case {case:?}: unexpected refusal message in {body}"
        );
    }

    // A project this store DOES hold is still served, so the refusal above is about
    // the id and not about the operation.
    let seen = walk(
        &router,
        &fixture.projects[0],
        Some(250),
        fixture.created[0].len(),
    )
    .await;
    assert_eq!(
        seen.len(),
        fixture.created[0].len(),
        "a stored project must still be served, saw {seen:?}"
    );
}

#[tokio::test]
async fn max_results_outside_the_modelled_range_is_refused() {
    let router = router();
    let fixture = create_fixture(&router).await;
    let project_id = &fixture.projects[0];

    // `maxResults` carries @range 1..250 (smithy_query query="operation"
    // operation="ListDashboards"): each published bound is served and one past it on
    // either side is refused.
    let cases: [(&str, i64, bool); 4] = [
        ("the range minimum", 1, true),
        ("the range maximum", 250, true),
        ("one under the minimum", 0, false),
        ("one past the maximum", 251, false),
    ];
    for (case, max_results, served) in cases {
        let uri = format!("{DASHBOARDS}?projectId={project_id}&maxResults={max_results}");
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
