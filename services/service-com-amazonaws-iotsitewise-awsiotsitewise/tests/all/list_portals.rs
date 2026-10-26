// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListPortals`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rule these suites pin: `ListPortals` enumerates EVERY portal the service's
//! state holds, each exactly once, in ONE stable order, cut into pages of at most
//! `maxResults` entries, where `nextToken` is present only while another entry
//! follows and resumes the enumeration exactly where the previous page stopped; a
//! listed portal carries the same values `DescribePortal` reports for it, and a
//! `nextToken` the operation never issued is refused rather than answered with an
//! empty page.
//!
//! The HTTP harness (`call`, `error_code`, `hex_token`, `is_modelled_id`) is the one
//! the asset suite already has; this suite reuses it rather than growing a second
//! copy.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::ACTIVATION_DELAY;

use crate::asset_lifecycle::{call, error_code, hex_token, is_modelled_id, router};

/// Where every portal request goes, as `OPERATIONS.md` reports the route:
/// `POST /portals`, `GET /portals`, `GET /portals/{portalId}`.
const PORTALS: &str = "/portals";

/// A service-role ARN within the modelled `IamArn` pattern.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::123456789012:role/{name}")
}

/// One row of the fixture: the portal's name beside the request members it carries
/// beyond the required ones, so the enumeration spans portals of several shapes.
struct PortalCase {
    name: &'static str,
    extra: Value,
}

/// The rows: nine portals differing along every dimension a `PortalSummary` reports
/// — a description or none, each `PortalType` value (`SITEWISE_PORTAL_V1` |
/// `SITEWISE_PORTAL_V2`, the enum's whole value list: smithy_query query="shape"
/// shape="PortalType") or none at all, and each `AuthMode` value (`IAM` | `SSO`,
/// likewise the whole list) — so a page boundary never lands only between two
/// identically shaped portals.
fn portal_cases() -> Vec<PortalCase> {
    vec![
        PortalCase {
            name: "listed-a",
            extra: json!({}),
        },
        PortalCase {
            name: "listed-b",
            extra: json!({ "portalDescription": "the floor-one portal" }),
        },
        PortalCase {
            name: "listed-c",
            extra: json!({ "portalType": "SITEWISE_PORTAL_V1" }),
        },
        PortalCase {
            name: "listed-d",
            extra: json!({
                "portalType": "SITEWISE_PORTAL_V2",
                "portalDescription": "the AI-aware portal",
            }),
        },
        PortalCase {
            name: "listed-e",
            extra: json!({ "portalAuthMode": "IAM" }),
        },
        PortalCase {
            name: "listed-f",
            extra: json!({
                "portalAuthMode": "SSO",
                "portalType": "SITEWISE_PORTAL_V1",
                "portalDescription": "an SSO classic portal",
            }),
        },
        PortalCase {
            name: "listed-g",
            extra: json!({ "notificationSenderEmail": "alarms@example.com" }),
        },
        PortalCase {
            name: "listed-h",
            extra: json!({
                "portalAuthMode": "IAM",
                "portalType": "SITEWISE_PORTAL_V2",
                "tags": { "stage": "test" },
            }),
        },
        PortalCase {
            name: "listed-i",
            extra: json!({ "portalDescription": "the ninth portal" }),
        },
    ]
}

/// Create one portal and return its id.
async fn create_portal(router: &MultiServiceRouter, case: &PortalCase) -> String {
    let mut body = json!({
        "portalName": format!("portal-{}", case.name),
        "portalContactEmail": "admin@example.com",
        "roleArn": role_arn(&format!("list-role-{}", case.name)),
    });
    let object = body.as_object_mut().expect("an object body");
    for (member, value) in case.extra.as_object().expect("an object of extras") {
        object.insert(member.clone(), value.clone());
    }

    let (status, created) = call(router, "POST", PORTALS, Some(body)).await;
    assert_eq!(
        status, 202,
        "case {:?}: CreatePortal expected 202, got {status} {created}",
        case.name
    );
    let id = created["portalId"].as_str().unwrap_or("").to_string();
    assert!(
        is_modelled_id(&id),
        "case {:?}: portalId must be a modelled UUID, got {created}",
        case.name
    );
    id
}

/// Create the whole fixture and return the ids, in creation order.
async fn create_fixture(router: &MultiServiceRouter) -> Vec<String> {
    let mut ids = Vec::new();
    for case in portal_cases() {
        ids.push(create_portal(router, &case).await);
    }
    ids
}

/// Walk the whole enumeration at `page_size` (or with no `maxResults` at all when
/// `page_size` is `None`), returning the ids in the order the pages reported them.
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
            PORTALS.to_string()
        } else {
            format!("{PORTALS}?{}", query.join("&"))
        };

        let (status, body) = call(router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: page {pages} expected 200, got {status} {body}"
        );
        let summaries = body["portalSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: portalSummaries must be a list, got {body}"));
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
        "case {case:?}: expected {expected_pages} pages over {total} portals, walked {pages}"
    );
    seen
}

#[tokio::test]
async fn list_portals_pages_through_every_stored_portal_once() {
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
        "the enumeration must cover exactly the created portals, saw {reference:?}"
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
        let seen = walk(&router, page_size, total).await;
        assert_eq!(
            seen, reference,
            "case {page_size:?}: paging must report the one stable order, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_listed_portal_carries_what_describe_portal_reports() {
    let router = router();
    let created = create_fixture(&router).await;

    // The status a portal reports is the one the clock owes it, so the comparison is
    // made once every portal has settled: otherwise the transition could fall due
    // BETWEEN the list and the describe and the two would legitimately disagree.
    tokio::time::sleep(ACTIVATION_DELAY + std::time::Duration::from_millis(50)).await;

    let (status, body) = call(&router, "GET", &format!("{PORTALS}?maxResults=250"), None).await;
    assert_eq!(status, 200, "ListPortals expected 200, got {status} {body}");
    let summaries = body["portalSummaries"].as_array().expect("portalSummaries");
    assert_eq!(
        summaries.len(),
        created.len(),
        "every created portal must be listed, got {body}"
    );

    // Each `PortalSummary` member beside the `DescribePortal` member that reports the
    // same value (smithy_query query="shape" shape="PortalSummary").
    let members = [
        ("id", "portalId"),
        ("name", "portalName"),
        ("description", "portalDescription"),
        ("startUrl", "portalStartUrl"),
        ("creationDate", "portalCreationDate"),
        ("lastUpdateDate", "portalLastUpdateDate"),
        ("roleArn", "roleArn"),
        ("status", "portalStatus"),
        ("portalType", "portalType"),
    ];
    for summary in summaries {
        let id = summary["id"].as_str().unwrap_or("");
        let (status, described) = call(&router, "GET", &format!("{PORTALS}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {id:?}: DescribePortal expected 200, got {status} {described}"
        );
        for (listed, describes) in members {
            assert_eq!(
                summary[listed], described[describes],
                "case {id:?}: the summary's {listed} must be what DescribePortal reports as \
                 {describes}: {summary} vs {described}"
            );
        }
    }
}

#[tokio::test]
async fn listing_portals_does_not_change_what_a_later_caller_sees() {
    let router = router();

    // A store with no portal reports an empty list and no token, rather than a
    // fabricated entry or a token that resumes nothing.
    let (status, empty) = call(&router, "GET", PORTALS, None).await;
    assert_eq!(status, 200, "an empty store expected 200, got {status} {empty}");
    assert_eq!(
        empty["portalSummaries"],
        json!([]),
        "an empty store lists no portal, got {empty}"
    );
    assert!(
        empty["nextToken"].is_null(),
        "an empty store issues no nextToken, got {empty}"
    );

    let _ = create_fixture(&router).await;
    // Read past the owed CREATING -> ACTIVE transition, so a state the clock makes
    // due does not make the two reads below differ for a reason that is not a defect.
    tokio::time::sleep(ACTIVATION_DELAY + std::time::Duration::from_millis(50)).await;

    // Rows differ in page size; each read twice. A read never moves the enumeration.
    for page_size in [1_usize, 5, 250] {
        let uri = format!("{PORTALS}?maxResults={page_size}");
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

    // Rows differ in HOW the token is wrong: not hex at all, hex of a well-formed id
    // no portal has, hex of a string that is no id, and an odd-length hex run.
    let forged = [
        ("not hex at all", "zzzz".to_string()),
        (
            "hex of an unknown id",
            hex_token("11111111-2222-3333-4444-555555555555"),
        ),
        ("hex of something that is no id", hex_token("listed-a")),
        ("odd-length hex", "abc".to_string()),
    ];
    for (case, token) in forged {
        let uri = format!("{PORTALS}?nextToken={token}");
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
async fn max_results_outside_the_modelled_range_is_refused() {
    let router = router();
    let _ = create_fixture(&router).await;

    // `maxResults` carries @range 1..250 (smithy_query query="operation"
    // operation="ListPortals"): each published bound is served and one past it on
    // either side is refused.
    let cases: [(&str, i64, bool); 4] = [
        ("the range minimum", 1, true),
        ("the range maximum", 250, true),
        ("one under the minimum", 0, false),
        ("one past the maximum", 251, false),
    ];
    for (case, max_results, served) in cases {
        let uri = format!("{PORTALS}?maxResults={max_results}");
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
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case {case:?} (maxResults={max_results}): unexpected error code in {body}"
            );
        }
    }
}
