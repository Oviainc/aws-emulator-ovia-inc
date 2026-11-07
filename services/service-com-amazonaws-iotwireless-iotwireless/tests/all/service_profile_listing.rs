// SPDX-License-Identifier: CC0-1.0

//! `ListServiceProfiles`, end to end over HTTP.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed `http::Request` — so what is asserted is what an SDK client sees, not
//! a handler return value.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! * paging enumerates every stored profile exactly once, in one stable order, whatever
//!   the page size — and mints a `NextToken` only while a further page exists;
//! * a delete is observable through this reader too, so a removed profile leaves the
//!   list and every other one stays;
//! * a `NextToken` this service did not issue is refused.

use http::{Method, StatusCode};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The ONE wire harness this crate's suites share.
use super::destination_and_wireless_device_lifecycle::{call, error_code, router};

/// `MaxResults`' published range on `ListServiceProfiles`.
/// Source: `smithy_query(query="operation", operation="ListServiceProfiles")`.
const MAX_RESULTS_MIN: i64 = 0;
const MAX_RESULTS_MAX: i64 = 250;

/// How many profiles the paging cases enumerate.
const STORED: usize = 6;

async fn create_profile(router: &MultiServiceRouter, body: Value) -> (StatusCode, Value) {
    call(router, Method::POST, "/service-profiles", Some(body)).await
}

/// Every `Id` the list serves, walked page by page at `max_results` (0 meaning "no
/// caller-supplied page size"), plus the number of pages it took.
async fn walk(router: &MultiServiceRouter, max_results: i64) -> (Vec<String>, usize) {
    let mut ids = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut uri = format!("/service-profiles?maxResults={max_results}");
        if let Some(token) = &token {
            uri.push_str(&format!("&nextToken={token}"));
        }
        let (status, body) = call(router, Method::GET, &uri, None).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case maxResults={max_results}: expected 200, got {status} {body}"
        );
        pages += 1;
        let page = body["ServiceProfileList"]
            .as_array()
            .unwrap_or_else(|| panic!("case maxResults={max_results}: no list in {body}"));
        if max_results > 0 {
            assert!(
                page.len() <= max_results as usize,
                "case maxResults={max_results}: a page returned {} entries",
                page.len()
            );
        }
        for entry in page {
            ids.push(
                entry["Id"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case maxResults={max_results}: no Id in {entry}"))
                    .to_string(),
            );
        }
        token = body["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return (ids, pages);
        }
        assert!(
            pages <= STORED + 2,
            "case maxResults={max_results}: pagination did not terminate"
        );
    }
}

/// Paging serves every stored profile exactly once, in the same order, at every page
/// size — including the published bounds of `MaxResults`.
#[tokio::test]
async fn paging_serves_every_service_profile_exactly_once() {
    let router = router();
    let mut created: Vec<String> = Vec::new();
    for index in 0..STORED {
        let (status, body) = create_profile(
            &router,
            json!({ "Name": format!("Listed{index}"), "LoRaWAN": { "DrMax": 7 } }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {index}: expected 201, got {status} {body}"
        );
        created.push(body["Id"].as_str().expect("an Id").to_string());
    }
    // The list walks one stable id order, which is the order the store keys them in.
    created.sort();

    for max_results in [
        MAX_RESULTS_MIN,
        1,
        2,
        STORED as i64 - 1,
        STORED as i64,
        STORED as i64 + 1,
        MAX_RESULTS_MAX,
    ] {
        let (ids, pages) = walk(&router, max_results).await;
        assert_eq!(
            ids, created,
            "case maxResults={max_results}: expected every profile exactly once in id \
             order, got {ids:?}"
        );
        let expected_pages = if max_results == 0 {
            1
        } else {
            STORED.div_ceil(max_results as usize)
        };
        assert_eq!(
            pages, expected_pages,
            "case maxResults={max_results}: expected {expected_pages} page(s), took {pages}"
        );
    }
}

/// A deleted profile leaves the list, and every profile the delete did not name stays.
#[tokio::test]
async fn a_deleted_profile_leaves_the_list() {
    let router = router();
    let mut remaining: Vec<String> = Vec::new();
    for index in 0..STORED {
        let (_, body) = create_profile(
            &router,
            json!({ "Name": format!("Removable{index}"), "LoRaWAN": {} }),
        )
        .await;
        remaining.push(body["Id"].as_str().expect("an Id").to_string());
    }
    remaining.sort();

    // Delete from the middle, the front and the back, so the surviving order is
    // re-checked around each removal point. The position is taken against the list as it
    // stands when that delete runs.
    for where_from in ["middle", "front", "back"] {
        let position = match where_from {
            "middle" => remaining.len() / 2,
            "front" => 0,
            _ => remaining.len() - 1,
        };
        let id = remaining.remove(position);
        let (status, body) = call(
            &router,
            Method::DELETE,
            &format!("/service-profiles/{id}"),
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "case {id}: expected 204 from the delete, got {status} {body}"
        );

        for max_results in [MAX_RESULTS_MIN, 1, 2] {
            let (ids, _) = walk(&router, max_results).await;
            assert_eq!(
                ids, remaining,
                "case {where_from} {id} maxResults={max_results}: expected the surviving \
                 profiles, got {ids:?}"
            );
        }
    }
}

/// A `NextToken` this service did not issue is a 400, whatever it looks like.
#[tokio::test]
async fn a_next_token_the_service_did_not_issue_is_refused() {
    let router = router();
    let (_, created) = create_profile(&router, json!({ "LoRaWAN": {} })).await;
    let id = created["Id"].as_str().expect("an Id").to_string();

    for bad in ["deadbeef", "not-hex", &hex::encode(&id), "00"] {
        let (status, body) = call(
            &router,
            Method::GET,
            &format!("/service-profiles?nextToken={bad}"),
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "case nextToken={bad:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case nextToken={bad:?}: expected ValidationException, got {body}"
        );
    }

    // An EMPTY token is "no token", so the list is served from the start.
    let (status, body) = call(&router, Method::GET, "/service-profiles?nextToken=", None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an empty nextToken must be served, got {status} {body}"
    );
    assert_eq!(
        body["ServiceProfileList"]
            .as_array()
            .map(|list| list.len())
            .unwrap_or_default(),
        1,
        "an empty nextToken must list from the start, got {body}"
    );
}
