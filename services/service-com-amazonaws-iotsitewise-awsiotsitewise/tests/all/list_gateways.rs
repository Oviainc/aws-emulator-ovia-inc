// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListGateways`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rule these suites pin: `ListGateways` enumerates EVERY gateway the service's
//! state holds, each exactly once, in ONE stable order, cut into pages of at most
//! `maxResults` entries, where `nextToken` is present only while another entry
//! follows and resumes the enumeration exactly where the previous page stopped; a
//! listed gateway carries the same values `DescribeGateway` reports for it, and a
//! `nextToken` the operation never issued is refused rather than answered with an
//! empty page.
//!
//! The HTTP harness (`call`, `error_code`, `is_modelled_id`, `hex_token`) is the one
//! the asset suite already has, and the route const and the `GatewayPlatform`
//! builders are the gateway suite's; this suite reuses both rather than growing a
//! second copy of either.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, error_code, hex_token, is_modelled_id, router};
use crate::gateway_lifecycle::{greengrass, greengrass_v2, siemens_ie, GATEWAYS};

/// Create one gateway and return its id.
async fn create_gateway(
    router: &MultiServiceRouter,
    name: &str,
    platform: Value,
    version: Option<&str>,
) -> String {
    let mut body = json!({ "gatewayName": name });
    let object = body.as_object_mut().expect("an object body");
    object.insert("gatewayPlatform".to_string(), platform);
    if let Some(version) = version {
        object.insert("gatewayVersion".to_string(), json!(version));
    }
    let (status, created) = call(router, "POST", GATEWAYS, Some(body)).await;
    assert_eq!(
        status, 201,
        "case {name:?}: CreateGateway expected 201, got {status} {created}"
    );
    let id = created["gatewayId"].as_str().unwrap_or("").to_string();
    assert!(
        is_modelled_id(&id),
        "case {name:?}: gatewayId must be a modelled UUID, got {created}"
    );
    id
}

/// One row of the fixture: the gateway's name, the platform it runs on and the
/// `gatewayVersion` it was created with (or none, which defaults to 2).
struct GatewayCase {
    name: &'static str,
    platform: Value,
    version: Option<&'static str>,
}

/// The rows: nine gateways spanning every platform type `GatewayPlatform` declares,
/// with and without an explicit `gatewayVersion`, so a page boundary never lands
/// only between two identically shaped gateways.
fn gateway_cases() -> Vec<GatewayCase> {
    vec![
        GatewayCase {
            name: "edge-a",
            platform: greengrass_v2("list-thing-a", None),
            version: None,
        },
        GatewayCase {
            name: "edge-b",
            platform: greengrass("list-group-b"),
            version: Some("2"),
        },
        GatewayCase {
            name: "edge-c",
            platform: siemens_ie("list-siemens-c"),
            version: Some("3"),
        },
        GatewayCase {
            name: "edge-d",
            platform: greengrass_v2("list-thing-d", Some("LINUX_AMD64")),
            version: Some("3"),
        },
        GatewayCase {
            name: "edge-e",
            platform: greengrass("list-group-e"),
            version: None,
        },
        GatewayCase {
            name: "edge-f",
            platform: siemens_ie("list-siemens-f"),
            version: None,
        },
        GatewayCase {
            name: "edge-g",
            platform: greengrass_v2("list-thing-g", Some("WINDOWS_AMD64")),
            version: Some("3"),
        },
        GatewayCase {
            name: "edge-h",
            platform: greengrass("list-group-h"),
            version: Some("2"),
        },
        GatewayCase {
            name: "edge-i",
            platform: siemens_ie("list-siemens-i"),
            version: Some("3"),
        },
    ]
}

/// Create the whole fixture and return the ids, in creation order.
async fn create_fixture(router: &MultiServiceRouter) -> Vec<String> {
    let mut ids = Vec::new();
    for case in gateway_cases() {
        ids.push(create_gateway(router, case.name, case.platform, case.version).await);
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
            GATEWAYS.to_string()
        } else {
            format!("{GATEWAYS}?{}", query.join("&"))
        };

        let (status, body) = call(router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: page {pages} expected 200, got {status} {body}"
        );
        let summaries = body["gatewaySummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: gatewaySummaries must be a list, got {body}"));
        // The documented default when the caller sends none is 50.
        let limit = page_size.unwrap_or(50);
        assert!(
            summaries.len() <= limit,
            "case {case:?}: a page holds at most {limit}, got {} in {body}",
            summaries.len()
        );
        for summary in summaries {
            seen.push(
                summary["gatewayId"]
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
        "case {case:?}: expected {expected_pages} pages over {total} gateways, walked {pages}"
    );
    seen
}

#[tokio::test]
async fn list_gateways_pages_through_every_stored_gateway_once() {
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
        "the enumeration must cover exactly the created gateways, saw {reference:?}"
    );

    // Rows differ in page size: the published `maxResults` bounds (1 and 250), sizes
    // that divide the fixture and sizes that do not, a size past the fixture, and no
    // `maxResults` at all (the documented default of 50).
    let page_sizes = [Some(1_usize), Some(2), Some(3), Some(4), Some(9), Some(10), Some(250), None];
    for page_size in page_sizes {
        let seen = walk(&router, page_size, total).await;
        assert_eq!(
            seen, reference,
            "case {page_size:?}: paging must report the one stable order, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_listed_gateway_carries_what_describe_gateway_reports() {
    let router = router();
    let created = create_fixture(&router).await;

    let (status, body) = call(&router, "GET", &format!("{GATEWAYS}?maxResults=250"), None).await;
    assert_eq!(status, 200, "ListGateways expected 200, got {status} {body}");
    let summaries = body["gatewaySummaries"].as_array().expect("gatewaySummaries");
    assert_eq!(
        summaries.len(),
        created.len(),
        "every created gateway must be listed, got {body}"
    );

    for summary in summaries {
        let id = summary["gatewayId"].as_str().unwrap_or("");
        let (status, described) = call(&router, "GET", &format!("{GATEWAYS}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {id:?}: DescribeGateway expected 200, got {status} {described}"
        );
        for member in [
            "gatewayId",
            "gatewayName",
            "gatewayPlatform",
            "gatewayVersion",
            "gatewayCapabilitySummaries",
            "creationDate",
            "lastUpdateDate",
        ] {
            assert_eq!(
                summary[member], described[member],
                "case {id:?}: the summary's {member} must be what DescribeGateway reports: \
                 {summary} vs {described}"
            );
        }
    }
}

#[tokio::test]
async fn listing_gateways_does_not_change_what_a_later_caller_sees() {
    let router = router();

    // A store with no gateway reports an empty list and no token, rather than a
    // fabricated entry or a token that resumes nothing.
    let (status, empty) = call(&router, "GET", GATEWAYS, None).await;
    assert_eq!(status, 200, "an empty store expected 200, got {status} {empty}");
    assert_eq!(
        empty["gatewaySummaries"],
        json!([]),
        "an empty store lists no gateway, got {empty}"
    );
    assert!(
        empty["nextToken"].is_null(),
        "an empty store issues no nextToken, got {empty}"
    );

    let _ = create_fixture(&router).await;

    // Rows differ in page size; each read twice. A read never moves the enumeration.
    for page_size in [1_usize, 5, 250] {
        let uri = format!("{GATEWAYS}?maxResults={page_size}");
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
    // no gateway has, hex of a string that is no id, and an odd-length hex run.
    let forged = [
        ("not hex at all", "zzzz".to_string()),
        (
            "hex of an unknown id",
            hex_token("11111111-2222-3333-4444-555555555555"),
        ),
        ("hex of something that is no id", hex_token("edge-a")),
        ("odd-length hex", "abc".to_string()),
    ];
    for (case, token) in forged {
        let uri = format!("{GATEWAYS}?nextToken={token}");
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
    // operation="ListGateways"): each published bound is served and one past it on
    // either side is refused.
    let cases: [(&str, i64, bool); 4] = [
        ("the range minimum", 1, true),
        ("the range maximum", 250, true),
        ("one under the minimum", 0, false),
        ("one past the maximum", 251, false),
    ];
    for (case, max_results, served) in cases {
        let uri = format!("{GATEWAYS}?maxResults={max_results}");
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
