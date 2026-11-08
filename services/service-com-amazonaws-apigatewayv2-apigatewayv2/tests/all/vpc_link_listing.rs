// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `GetVpcLinks`, the collection reader of this service's VPC
//! links.
//!
//! The rules these cases assert:
//!
//! * **`GetVpcLinks` reports the VPC links the account holds**, each projected exactly as
//!   `GetVpcLink` projects it — so a client that lists and a client that reads one link are
//!   told the same thing about it — and an account holding none is reported as an empty
//!   collection rather than as a failure.
//! * **One call serves at most one bounded page**: the page size is the request's
//!   `MaxResults` clamped to this service's own maximum, and the `NextToken` the answer
//!   carries resumes the walk at the first link that did not fit, so a full walk reports
//!   every link exactly once.
//! * **A `NextToken` this reader did not issue is refused** 400 `BadRequestException`
//!   rather than served as a page of its own.
//!
//! The rows vary what each rule generalises over: the number of links the account holds and
//! the page size asked for, and — for the refusal — the way a token departs from one this
//! reader mints.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::MAX_PAGE_SIZE;

use crate::api_lifecycle::{as_str, call, emulator, error_code};

/// The collection URI `CreateVpcLink` and `GetVpcLinks` are bound to.
const VPC_LINKS: &str = "/v2/vpclinks";

/// Create one VPC link named `name` and answer its `VpcLinkId`.
async fn create_link(emulator: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        VPC_LINKS,
        Some(json!({ "name": name, "subnetIds": ["subnet-0a1b2c3d"] })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {name:?}: expected 201 from CreateVpcLink, got {status} {created}"
    );
    as_str(&created, "vpcLinkId")
}

/// The `Items` of one `GetVpcLinks` page, with the `NextToken` it carries.
async fn page(
    emulator: &MultiServiceRouter,
    query: &str,
    case: &str,
) -> (Vec<Value>, Option<String>) {
    let (status, body) = call(emulator, "GET", &format!("{VPC_LINKS}{query}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetVpcLinks, got {status} {body}"
    );
    let items = body
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let next_token = body
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (items, next_token)
}

/// RULE: the collection reader reports the links the account holds, each exactly as the
/// single-link reader reports it.
#[tokio::test]
async fn the_listing_reports_every_link_as_the_single_link_reader_does() {
    let emulator = emulator();

    // An account holding no link at all is reported as an empty collection.
    let (items, next_token) = page(&emulator, "", "an account holding no VPC link").await;
    assert!(
        items.is_empty(),
        "expected an empty collection for an account holding no link, got {items:?}"
    );
    assert_eq!(
        next_token, None,
        "expected nothing to resume when the collection is empty, got {next_token:?}"
    );

    // The rows differ along the number of links the account holds, which is what the rule
    // generalises over.
    let mut created = Vec::new();
    for count in 1..=3 {
        let name = format!("listed-{count}");
        created.push(create_link(&emulator, &name).await);
        let (items, _) = page(&emulator, "", &format!("{count} link(s)")).await;
        assert_eq!(
            items.len(),
            created.len(),
            "case \"{count} link(s)\": expected the listing to report every link, got \
             {items:?}"
        );
        for id in &created {
            let listed = items
                .iter()
                .find(|item| item.get("vpcLinkId").and_then(Value::as_str) == Some(id.as_str()))
                .unwrap_or_else(|| {
                    panic!("case \"{count} link(s)\": {id:?} is missing from {items:?}")
                });
            let (status, fetched) =
                call(&emulator, "GET", &format!("{VPC_LINKS}/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {id:?}: expected 200 from GetVpcLink, got {status} {fetched}"
            );
            assert_eq!(
                *listed, fetched,
                "case {id:?}: expected the listing to report what GetVpcLink reports"
            );
        }
    }
}

/// RULE: one call serves at most one bounded page, and the token it answers resumes the
/// walk where that page stopped, so a full walk reports every link exactly once.
#[tokio::test]
async fn a_walk_resumed_by_its_own_token_reports_every_link_once() {
    let emulator = emulator();
    let mut held = Vec::new();
    for index in 0..5 {
        held.push(create_link(&emulator, &format!("paged-{index}")).await);
    }
    held.sort();

    // The rows differ along the page size asked for: below the collection, equal to it,
    // past it, the server-side maximum, and none at all.
    let rows: &[(&str, Option<usize>)] = &[
        ("one at a time", Some(1)),
        ("two at a time", Some(2)),
        ("the whole collection in one page", Some(5)),
        ("a page size past the collection", Some(50)),
        ("the server-side maximum", Some(MAX_PAGE_SIZE)),
        ("no page size at all", None),
    ];
    for (case, max_results) in rows {
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut query = Vec::new();
            if let Some(max) = max_results {
                query.push(format!("maxResults={max}"));
            }
            if let Some(token) = &next_token {
                query.push(format!("nextToken={token}"));
            }
            let query = if query.is_empty() {
                String::new()
            } else {
                format!("?{}", query.join("&"))
            };
            let (items, token) = page(&emulator, &query, case).await;
            if let Some(max) = max_results {
                assert!(
                    items.len() <= *max,
                    "case {case:?}: expected at most {max} per page, got {}",
                    items.len()
                );
            }
            seen.extend(
                items
                    .iter()
                    .map(|item| as_str(item, "vpcLinkId"))
                    .collect::<Vec<String>>(),
            );
            match token {
                Some(token) => next_token = Some(token),
                None => break,
            }
        }
        let mut walked = seen.clone();
        walked.sort();
        assert_eq!(
            walked, held,
            "case {case:?}: expected every link exactly once, got {seen:?}"
        );
    }
}

/// RULE: a `NextToken` this reader did not issue names no position in the collection, so it
/// is refused rather than served as a page.
#[tokio::test]
async fn a_token_this_reader_did_not_issue_is_refused() {
    let emulator = emulator();
    for index in 0..3 {
        create_link(&emulator, &format!("tokened-{index}")).await;
    }
    // A token this reader DID issue, to resume at its second page.
    let (_, issued) = page(&emulator, "?maxResults=1", "a first page of one").await;
    let issued = issued.expect("links remain, so a token was issued");

    // The rows span the ways a value departs from a token this reader mints: text that is
    // not hex, an odd number of hex digits, hex whose bytes are not UTF-8, hex carrying no
    // separator, a token longer than any this service mints, and another collection's own
    // token.
    let (_, other_collection) = {
        let (status, body) = call(&emulator, "GET", "/v2/apis?maxResults=1", None).await;
        assert_eq!(status, 200, "expected 200 from GetApis, got {status} {body}");
        (
            (),
            body.get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string),
        )
    };
    let mut rows: Vec<(&str, String)> = vec![
        ("text that is not hex", "not-a-token".to_string()),
        ("an odd number of hex digits", "abc".to_string()),
        ("hex whose bytes are not UTF-8", "ff".to_string()),
        ("hex carrying no separator", "6e6f736570".to_string()),
        ("a token past any this service mints", "61".repeat(4_096)),
    ];
    if let Some(token) = other_collection {
        rows.push(("another collection's own token", token));
    }
    for (case, token) in rows {
        let (status, body) = call(
            &emulator,
            "GET",
            &format!("{VPC_LINKS}?nextToken={token}"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for a token this reader never issued, got {status} \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
    }

    // The token it DID issue is accepted, so the refusals above are about those tokens and
    // not about tokens in general.
    let (items, _) = page(
        &emulator,
        &format!("?maxResults=1&nextToken={issued}"),
        "its own token",
    )
    .await;
    assert_eq!(
        items.len(),
        1,
        "expected the issuing reader to accept its own token, got {items:?}"
    );
}
