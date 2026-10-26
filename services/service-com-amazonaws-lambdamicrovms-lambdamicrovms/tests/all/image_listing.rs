// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListMicrovmImages`: the listing of the MicroVM images an
//! account holds.
//!
//! RULE: the listing reports the images this account currently holds — one summary per
//! stored image record, in one stable order, each carrying that record's own ARN, name,
//! state, latest active and failed versions and creation instant — restricted to the
//! images whose name CONTAINS `nameFilter` when one is given, paged by the operation's
//! `@paginated` contract, and nothing else: an image the clock has already removed is
//! gone from it and a name never created is absent.
//!
//! Each test states that rule and exercises it over a SET of inputs; the values any one
//! row happens to carry are examples, not the specification. The wire harness
//! (`emulator`, `call`, `image_body`, …) is the one `microvm_lifecycle` owns.

use serde_json::{json, Value};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::MAX_LIST_PAGE;
use smithy_server_runtime::MultiServiceRouter;

use crate::microvm_image_and_instance_paths::{delete_image, get_image};
use crate::microvm_lifecycle::{call, emulator, expected_image_arn, image_body, PAST_THE_SPAN};

/// `GET /2025-09-09/microvm-images`, with whatever `query` adds to the query string.
async fn list_images(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    let uri = match query.is_empty() {
        true => "/2025-09-09/microvm-images".to_string(),
        false => format!("/2025-09-09/microvm-images?{query}"),
    };
    call(router, "GET", &uri, None).await
}

/// Create an image named `name` with only the required members, asserting the create.
async fn create_image(router: &MultiServiceRouter, name: &str) {
    let (status, body) = call(
        router,
        "POST",
        "/2025-09-09/microvm-images",
        Some(image_body(name, json!({}))),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {name:?}: create expected 201, got {status} {body}"
    );
}

/// The `items` of one page.
fn page_items(body: &Value, case: &str) -> Vec<Value> {
    body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: items is a list, got {body}"))
        .clone()
}

/// The `name`s of one page, in the order the page carries them.
fn page_names(body: &Value, case: &str) -> Vec<String> {
    page_items(body, case)
        .iter()
        .map(|item| item["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Walk every page of the listing `query` selects at page size `size`, returning the
/// names in the order the walk served them. Fails rather than hanging on a listing that
/// never terminates.
async fn walk(
    router: &MultiServiceRouter,
    query: &str,
    size: Option<usize>,
    limit: usize,
    case: &str,
) -> Vec<String> {
    let cap = size.unwrap_or(MAX_LIST_PAGE);
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;

    for _ in 0..=limit {
        let mut parts: Vec<String> = Vec::new();
        if !query.is_empty() {
            parts.push(query.to_string());
        }
        if let Some(size) = size {
            parts.push(format!("maxResults={size}"));
        }
        if let Some(token) = &token {
            parts.push(format!("nextToken={token}"));
        }
        let (status, body) = list_images(router, &parts.join("&")).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} {body}"
        );

        let page = page_names(&body, case);
        assert!(
            page.len() <= cap,
            "case {case}: a page carries at most {cap} images, got {} ({page:?})",
            page.len()
        );
        seen.extend(page);
        assert!(
            seen.len() <= limit,
            "case {case}: the walk served more than the account holds, got {seen:?}"
        );

        token = body["nextToken"].as_str().map(str::to_string);
        match &token {
            Some(issued) => assert!(
                seen.len() < limit,
                "case {case}: a nextToken ({issued}) is due only while images remain, got it \
                 after {seen:?}"
            ),
            None => break,
        }
    }

    assert!(
        token.is_none(),
        "case {case}: the walk must end with no nextToken, got {token:?}"
    );
    seen
}

/// RULE: however the pages are cut, the listing walks every image the account holds —
/// each exactly once, in one stable order, at most `maxResults` per page — and hands
/// back a `nextToken` only while images remain.
#[tokio::test]
async fn every_held_image_is_served_once_over_every_page_size() {
    let router = emulator();

    // Names differing in length, case and punctuation, created in an order that is NOT
    // the order the listing serves them, so the walk's order is the listing's own.
    let created = ["zulu", "alpha", "m_1", "Beta", "alpha-2", "a"];
    for name in created {
        create_image(&router, name).await;
    }
    let mut expected: Vec<String> = created.iter().map(|name| (*name).to_string()).collect();
    expected.sort_unstable();

    // Rows differ in the page size, which is what the page walk generalises over: no
    // `maxResults` at all, each published bound of `@range(min: 1, max: 50)`, and every
    // size up to the number of images held.
    let mut sizes: Vec<Option<usize>> = vec![None, Some(1), Some(MAX_LIST_PAGE)];
    sizes.extend((2..=created.len()).map(Some));

    for size in sizes {
        let case = format!("maxResults={size:?}");
        let seen = walk(&router, "", size, created.len(), &case).await;
        assert_eq!(
            seen, expected,
            "case {case}: expected every held image exactly once in order, got {seen:?}"
        );
    }
}

/// RULE: `nameFilter` selects the images whose name CONTAINS it, so any substring of a
/// held name selects that image wherever it occurs in the name, and a value no held
/// name contains selects nothing — a terminal empty page, not a refusal.
#[tokio::test]
async fn a_name_filter_selects_every_image_whose_name_contains_it() {
    let router = emulator();

    let held = ["api-worker", "worker", "worker-2", "batch-api", "Worker"];
    for name in held {
        create_image(&router, name).await;
    }

    // Rows differ in WHERE the filter matches the name: the whole name, a prefix, an
    // infix, a suffix, a single character, a case that differs, and a value no name
    // contains.
    let rows: Vec<(&str, &str, Vec<&str>)> = vec![
        ("the whole name of one image", "api-worker", vec!["api-worker"]),
        (
            "a prefix shared by several names",
            "worker",
            vec!["api-worker", "worker", "worker-2"],
        ),
        ("an infix", "-wor", vec!["api-worker"]),
        ("a suffix", "-2", vec!["worker-2"]),
        (
            "a single character several names carry",
            "b",
            vec!["batch-api"],
        ),
        ("a filter whose case differs", "Worker", vec!["Worker"]),
        ("a value no held name contains", "absent", vec![]),
    ];

    for (case, filter, mut expected) in rows {
        expected.sort_unstable();
        let (status, body) = list_images(&router, &format!("nameFilter={filter}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: nameFilter={filter} expected 200, got {status} {body}"
        );
        let mut names = page_names(&body, case);
        names.sort_unstable();
        assert_eq!(
            names, expected,
            "case {case:?}: nameFilter={filter} selects the names containing it, got {body}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: one page exhausted the matches, so no nextToken is due, got {:?}",
            body["nextToken"]
        );
    }

    // The filter only narrows: paging a filtered listing serves the same set as paging
    // it in one page.
    let case = "the filtered walk at one image per page";
    let seen = walk(&router, "nameFilter=worker", Some(1), held.len(), case).await;
    assert_eq!(
        seen,
        vec![
            "api-worker".to_string(),
            "worker".to_string(),
            "worker-2".to_string()
        ],
        "case {case}: the filter narrows every page of the walk, got {seen:?}"
    );
}

/// RULE: each entry is the summary of the stored image record, so every member it
/// carries is the one the same image's `GetMicrovmImage` reports, and reading the
/// listing changes nothing a later caller sees.
#[tokio::test]
async fn each_entry_is_the_stored_record_and_the_read_changes_nothing() {
    let router = emulator();

    // Rows differ in what the image holds and how far its asynchronous build has got,
    // because the entry is the record either way.
    create_image(&router, "just-created").await;
    let (status, created) = call(
        &router,
        "POST",
        "/2025-09-09/microvm-images",
        Some(image_body(
            "tagged-and-built",
            json!({"description": "described", "tags": {"team": "platform"}}),
        )),
    )
    .await;
    assert_eq!(status, 201, "create the tagged image: got {status} {created}");
    tokio::time::sleep(PAST_THE_SPAN).await;

    let (status, first) = list_images(&router, "").await;
    assert_eq!(status, 200, "list the images: got {status} {first}");
    let items = page_items(&first, "every held image");
    assert_eq!(
        items.len(),
        2,
        "the account holds the two images created, got {first}"
    );

    for item in &items {
        let name = item["name"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            item["imageArn"].as_str(),
            Some(expected_image_arn(&name).as_str()),
            "case {name:?}: the entry carries the ARN the create minted, got {item}"
        );

        let (status, image) = get_image(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the listed image reads back, got {status} {image}"
        );
        for member in [
            "imageArn",
            "name",
            "state",
            "latestActiveImageVersion",
            "latestFailedImageVersion",
            "createdAt",
        ] {
            assert_eq!(
                item[member], image[member],
                "case {name:?}: the entry's {member} is the stored record's, got {:?} listed and \
                 {:?} read",
                item[member], image[member]
            );
        }
    }

    // A reporting operation changes nothing: the same listing, read again, is the same.
    let (status, again) = list_images(&router, "").await;
    assert_eq!(status, 200, "re-list the images: got {status} {again}");
    assert_eq!(
        again, first,
        "the listing reports state, so reading it twice reports the same, got {again}"
    );
}

/// RULE: the listing reports what the account holds NOW, so an image is listed from its
/// create until its asynchronous deletion is due and gone from the listing after — and
/// a name created again after that is one entry, carrying the new record.
#[tokio::test]
async fn a_deleted_image_leaves_the_listing_and_its_name_is_free_again() {
    let router = emulator();

    // Rows differ in how the delete addresses the image, which cannot change what the
    // listing reports afterwards.
    let rows: Vec<(&str, &str, bool)> = vec![
        ("by name", "reused-by-name", false),
        ("by ARN", "reused-by-arn", true),
    ];

    for (case, name, by_arn) in rows {
        create_image(&router, name).await;
        let (status, body) = list_images(&router, &format!("nameFilter={name}")).await;
        assert_eq!(status, 200, "case {case:?}: list got {status} {body}");
        assert_eq!(
            page_names(&body, case),
            vec![name.to_string()],
            "case {case:?}: a created image is listed, got {body}"
        );

        let identifier = match by_arn {
            true => expected_image_arn(name),
            false => name.to_string(),
        };
        let (status, deleted) = delete_image(&router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: delete expected 200, got {status} {deleted}"
        );

        // The deletion is asynchronous, so the image is still held — and listed,
        // reported DELETING — until the clock makes the removal due.
        let (status, during) = list_images(&router, &format!("nameFilter={name}")).await;
        assert_eq!(status, 200, "case {case:?}: list got {status} {during}");
        let items = page_items(&during, case);
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: an image whose delete is in flight is still held, got {during}"
        );
        assert_eq!(
            items[0]["state"].as_str(),
            Some("DELETING"),
            "case {case:?}: the listing reports the state the record holds, got {:?}",
            items[0]["state"]
        );

        tokio::time::sleep(PAST_THE_SPAN).await;
        let (status, after) = list_images(&router, &format!("nameFilter={name}")).await;
        assert_eq!(status, 200, "case {case:?}: list got {status} {after}");
        assert_eq!(
            page_names(&after, case),
            Vec::<String>::new(),
            "case {case:?}: the removed image is gone from the listing, got {after}"
        );

        // The name is free again, and the listing reports the NEW record under it once.
        create_image(&router, name).await;
        let (status, reused) = list_images(&router, &format!("nameFilter={name}")).await;
        assert_eq!(status, 200, "case {case:?}: list got {status} {reused}");
        let items = page_items(&reused, case);
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: the re-created name is one entry, got {reused}"
        );
        assert_eq!(
            items[0]["state"].as_str(),
            Some("CREATING"),
            "case {case:?}: the entry is the NEW record, whose build has just started, got {:?}",
            items[0]["state"]
        );
    }
}

/// RULE: an account that holds no image the request selects gets a terminal empty page —
/// `items` is `@required`, so it is projected empty rather than omitted.
#[tokio::test]
async fn an_account_holding_no_selected_image_gets_an_empty_page() {
    let router = emulator();

    // Rows differ in why nothing is selected: the account holds nothing at all, and it
    // holds images none of which the filter selects.
    let rows: Vec<(&str, &str, bool)> = vec![
        ("an empty account, no filter", "", false),
        ("an empty account, a filter", "nameFilter=anything", false),
        ("a populated account, a filter nothing matches", "nameFilter=zzz", true),
    ];

    for (case, query, populate) in rows {
        if populate {
            create_image(&router, "holds-something").await;
        }
        let (status, body) = list_images(&router, query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            body["items"],
            json!([]),
            "case {case:?}: items is projected as an empty list, got {body}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: an exhausted listing issues no nextToken, got {:?}",
            body["nextToken"]
        );
    }
}

/// RULE: `maxResults` is bounded by the range the model publishes, so each bound is
/// served and a value one past either bound is refused with HTTP 400
/// `ValidationException` — `maxResults` is `@range(min: 1, max: 50)`
/// (`smithy_query(query="operation", operation="ListMicrovmImages")`).
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = emulator();
    create_image(&router, "one-image").await;

    let rows: Vec<(&str, usize, u16)> = vec![
        ("one below the published minimum", 0, 400),
        ("the published minimum", 1, 200),
        ("the published maximum", MAX_LIST_PAGE, 200),
        ("one past the published maximum", MAX_LIST_PAGE + 1, 400),
    ];

    for (case, max_results, expected_status) in rows {
        let (status, body) = list_images(&router, &format!("maxResults={max_results}")).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: maxResults={max_results} expected {expected_status}, got {status} \
             {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                body["__type"].as_str(),
                Some("ValidationException"),
                "case {case:?}: expected ValidationException, got {body}"
            );
        }
    }
}

/// RULE: a `nextToken` this listing did not issue positions nothing, so it is refused
/// with HTTP 400 `ValidationException` rather than silently restarting the listing.
#[tokio::test]
async fn a_next_token_this_listing_did_not_issue_is_refused() {
    let router = emulator();
    create_image(&router, "one-image").await;

    // Rows differ in HOW the token fails to be one this listing issued: not the opaque
    // encoding at all, the encoding carrying no cursor of this listing, and another
    // listing's token.
    let rows: Vec<(&str, String)> = vec![
        ("not the opaque encoding", "not-a-token".to_string()),
        ("an empty encoded value", hex_token("")),
        ("an encoded cursor with no marker", hex_token("one-image")),
        (
            "another listing's token",
            hex_token("ListMicrovms/1:mvm-0000000000000000"),
        ),
        (
            "a token of a marker this listing never issued",
            hex_token("ListMicrovmImages/0:one-image"),
        ),
    ];

    for (case, token) in rows {
        let (status, body) = list_images(&router, &format!("nextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: nextToken={token} must be refused, got {status} {body}"
        );
        assert_eq!(
            body["__type"].as_str(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some("nextToken is not a pagination token this service issued"),
            "case {case:?}: expected the refusal to name the token member, got {:?}",
            body["message"]
        );
    }
}

/// A hex-encoded `nextToken` candidate, as the opaque token form this listing issues.
fn hex_token(value: &str) -> String {
    ::hex::encode(value.as_bytes())
}
