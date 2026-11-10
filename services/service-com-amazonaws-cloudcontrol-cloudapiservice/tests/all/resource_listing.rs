// SPDX-License-Identifier: CC0-1.0

//! `ListResources`: which resources a listing reports, in what order, and how a page
//! resumes where the previous one stopped.

use serde_json::{json, Value};

use super::emulator::{code, descriptions, message, text, Emulator};

/// The type whose listing the suite pages over.
const STREAM: &str = "AWS::Kinesis::Stream";

/// A second type, whose resources must never appear in the first type's listing.
const RULE: &str = "AWS::Events::Rule";

/// Create every named resource of `type_name` and wait until all of them are
/// provisioned. The creates are independent, so one span covers them all.
async fn provision_all(emulator: &Emulator, type_name: &str, property: &str, names: &[&str]) {
    let mut accepted = Vec::new();
    for name in names {
        let event = emulator
            .create_ok(type_name, &json!({ property: name }), None)
            .await;
        accepted.push(text(&event, "RequestToken"));
    }
    for request_token in &accepted {
        emulator.settled(request_token).await;
    }
}

#[tokio::test]
async fn a_narrowed_listing_walks_the_inventory_one_window_at_a_time() {
    let emulator = Emulator::new();
    // Every third resource matches the selector, and the pages are narrower than the gap
    // between two matches, so the walk has to cross several windows of the inventory to
    // report them all: a listing reads one window per call rather than the whole type.
    let names: Vec<String> = (0..12).map(|index| format!("stream-{index:02}")).collect();
    let mut accepted = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let stage = if index % 3 == 0 { "prod" } else { "test" };
        let event = emulator
            .create_ok(STREAM, &json!({ "StreamName": name, "Stage": stage }), None)
            .await;
        accepted.push(text(&event, "RequestToken"));
    }
    for request_token in &accepted {
        emulator.settled(request_token).await;
    }

    let selector = json!({ "Stage": "prod" });
    let mut reported: Vec<String> = Vec::new();
    let mut next: Option<String> = None;
    let mut calls = 0;
    loop {
        let listed = emulator
            .cc_ok(
                "ListResources",
                Emulator::list_request(STREAM, Some(2), next.as_deref(), Some(&selector)),
            )
            .await;
        calls += 1;
        reported.extend(
            descriptions(&listed)
                .iter()
                .map(|description| text(description, "Identifier")),
        );
        match listed["NextToken"].as_str() {
            Some(token) => next = Some(token.to_string()),
            None => break,
        }
        assert!(
            calls <= names.len(),
            "the pages never ran out after {calls} calls, got {reported:?}"
        );
    }

    let expected: Vec<String> = names
        .iter()
        .enumerate()
        .filter(|(index, _)| index % 3 == 0)
        .map(|(_, name)| name.clone())
        .collect();
    assert_eq!(
        reported, expected,
        "a narrowed walk must report every matching resource in identifier order, expected \
         {expected:?}, got {reported:?}"
    );
}

#[tokio::test]
async fn a_listing_pages_in_identifier_order_and_resumes_where_it_stopped() {
    let emulator = Emulator::new();
    // Created in an order that is not identifier order, so an ordered listing cannot be
    // insertion order by accident.
    let names = ["echo", "alpha", "delta", "bravo", "charlie"];
    provision_all(&emulator, STREAM, "StreamName", &names).await;
    provision_all(&emulator, RULE, "RuleName", &["alpha", "zulu"]).await;

    let mut expected: Vec<String> = names.iter().map(|name| name.to_string()).collect();
    expected.sort();

    // The rows differ along the page size: one per page, a size that divides the set,
    // one that does not, exactly the set, the modelled maximum, and no `MaxResults` at
    // all.
    for page_size in [Some(1), Some(2), Some(3), Some(5), Some(100), None] {
        let walked = emulator.walk(STREAM, page_size).await;
        assert_eq!(
            walked, expected,
            "page size {page_size:?}: expected {expected:?}, got {walked:?}"
        );
    }

    // A page that is not the last one carries a token; the last one does not.
    let first = emulator.list(STREAM, Some(2), None).await;
    assert_eq!(
        descriptions(&first).len(),
        2,
        "a page of two must carry two descriptions, got {first:?}"
    );
    assert!(
        first["NextToken"].as_str().is_some(),
        "a page with more to come must carry a NextToken, got {first:?}"
    );
    assert_eq!(
        text(&first, "TypeName"),
        STREAM,
        "the listing must report the type it is of, got {first:?}"
    );
    let whole = emulator.list(STREAM, Some(100), None).await;
    assert!(
        whole["NextToken"].is_null(),
        "the last page must carry no NextToken, got {whole:?}"
    );

    // And the other type's listing is its own.
    let other = emulator.walk(RULE, Some(1)).await;
    assert_eq!(
        other,
        vec!["alpha".to_string(), "zulu".to_string()],
        "the other type's listing must report only its own resources, got {other:?}"
    );
}

#[tokio::test]
async fn a_continuation_token_resumes_only_the_listing_that_minted_it() {
    let emulator = Emulator::new();
    provision_all(
        &emulator,
        STREAM,
        "StreamName",
        &["alpha", "bravo", "charlie"],
    )
    .await;
    provision_all(&emulator, RULE, "RuleName", &["alpha", "bravo", "charlie"]).await;

    let first = emulator.list(STREAM, Some(1), None).await;
    let token = first["NextToken"].as_str().unwrap_or_default().to_string();
    assert!(
        !token.is_empty(),
        "the first of three pages must carry a NextToken, got {first:?}"
    );

    // It resumes THIS listing, after the resource the page stopped at.
    let resumed = emulator.list(STREAM, Some(1), Some(&token)).await;
    let identifiers: Vec<String> = descriptions(&resumed)
        .iter()
        .map(|description| text(description, "Identifier"))
        .collect();
    assert_eq!(
        identifiers,
        vec!["bravo".to_string()],
        "the token must resume after the page it was minted from, got {resumed:?}"
    );

    // A token minted for one type's listing resumes no other type's, even though that
    // type holds a resource of the same identifier.
    let (status, body) = emulator
        .cc(
            "ListResources",
            Emulator::list_request(RULE, Some(1), Some(&token), None),
        )
        .await;
    assert_eq!(
        status, 400,
        "another type's listing: expected HTTP 400, got {status} with body {body:?}"
    );
    assert!(
        code(&body).contains("InvalidRequestException"),
        "another type's listing: expected InvalidRequestException, got {:?}",
        code(&body)
    );
    assert_eq!(
        message(&body),
        "Invalid request provided: NextToken is not a continuation token of this listing",
        "another type's listing: unexpected wording"
    );

    // Nor does any value this store did not mint. The rows differ along how the value
    // fails to be one: not hex at all, hex of the right shape, and a minted token with
    // one character changed.
    let mut tampered = token.clone();
    tampered.pop();
    tampered.push(if token.ends_with('0') { '1' } else { '0' });
    let rows = [
        ("a value a caller wrote", "not-a-token".to_string()),
        ("hex of a minted token's shape", "00".repeat(40)),
        ("a minted token with one character changed", tampered),
    ];
    for (case, candidate) in rows {
        let (status, body) = emulator
            .cc(
                "ListResources",
                Emulator::list_request(STREAM, Some(1), Some(&candidate), None),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body:?}"
        );
        assert!(
            code(&body).contains("InvalidRequestException"),
            "case {case:?}: expected InvalidRequestException, got {:?}",
            code(&body)
        );
    }
}

#[tokio::test]
async fn a_listing_narrowed_by_a_resource_model_reports_the_resources_that_match_it() {
    let emulator = Emulator::new();
    let models = [
        json!({ "StreamName": "alpha", "Stage": "prod", "Size": 1 }),
        json!({ "StreamName": "bravo", "Stage": "prod", "Size": 2 }),
        json!({ "StreamName": "charlie", "Stage": "test", "Size": 1 }),
    ];
    let mut accepted = Vec::new();
    for model in &models {
        let event = emulator.create_ok(STREAM, model, None).await;
        accepted.push(text(&event, "RequestToken"));
    }
    for request_token in &accepted {
        emulator.settled(request_token).await;
    }

    // The rows differ along what the selector names: one property, a numeric property,
    // two at once, one nothing carries, and no property at all.
    let rows: [(&str, Value, Vec<&str>); 5] = [
        (
            "a selector naming one property",
            json!({ "Stage": "prod" }),
            vec!["alpha", "bravo"],
        ),
        (
            "a selector naming a numeric property",
            json!({ "Size": 1 }),
            vec!["alpha", "charlie"],
        ),
        (
            "a selector naming two properties",
            json!({ "Stage": "prod", "Size": 1 }),
            vec!["alpha"],
        ),
        (
            "a selector nothing carries",
            json!({ "Stage": "dev" }),
            vec![],
        ),
        (
            "a selector naming no property",
            json!({}),
            vec!["alpha", "bravo", "charlie"],
        ),
    ];
    for (case, selector, expected) in rows {
        let listed = emulator
            .cc_ok(
                "ListResources",
                Emulator::list_request(STREAM, Some(100), None, Some(&selector)),
            )
            .await;
        let identifiers: Vec<String> = descriptions(&listed)
            .iter()
            .map(|description| text(description, "Identifier"))
            .collect();
        let expected: Vec<String> = expected.iter().map(|name| name.to_string()).collect();
        assert_eq!(
            identifiers, expected,
            "case {case:?}: expected {expected:?}, got {identifiers:?}"
        );
    }

    // A selector that is not a resource model is refused rather than ignored.
    for (case, selector) in [
        ("not JSON at all", "not json"),
        ("a JSON list", "[1,2,3]"),
        ("a bare string", "\"prod\""),
    ] {
        let (status, body) = emulator
            .cc(
                "ListResources",
                json!({ "TypeName": STREAM, "ResourceModel": selector }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body:?}"
        );
        assert!(
            code(&body).contains("InvalidRequestException"),
            "case {case:?}: expected InvalidRequestException, got {:?}",
            code(&body)
        );
        assert!(
            message(&body).starts_with("Invalid request provided: ResourceModel"),
            "case {case:?}: the refusal must name the member it is about, got {:?}",
            message(&body)
        );
    }
}
