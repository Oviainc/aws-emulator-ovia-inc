// SPDX-License-Identifier: CC0-1.0

//! `ListTunnels` accepts a continuation token only in the exact form it mints.
//!
//! The rule under test: a page cursor is opaque to the caller, so the listing
//! accepts a `nextToken` only when it is identical to the canonical rendering
//! the service itself produces, and refuses every other string — truncated,
//! re-cased, over-padded, re-prefixed or hand-written — as invalid input.
//!
//! Every case is a row in a table: one row per way a client can garble a cursor
//! it was handed, each derived from a token this service really minted over the
//! wire, so the assertions are about the class of forged tokens rather than the
//! one string an issue happened to name.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "eu-west-1";

/// A router carrying only this service, over the engine the test also inspects.
fn mount(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Call one operation the way an SDK does and return `(status, body)`.
async fn call(router: &MultiServiceRouter, operation: &str, input: &Value) -> (u16, Value) {
    let body = serde_json::to_vec(input).expect("serialize the request body");
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("IoTSecuredTunneling.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/IoTSecuredTunneling/\
                 aws4_request, SignedHeaders=host, Signature=abc"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Whether `token` satisfies `NextToken`'s modelled pattern
/// `^[a-zA-Z0-9_=-]{1,4096}$` (from
/// `smithy_query(query="operation", operation="ListTunnels")`), so a row is
/// known to reach the handler instead of stopping at the boundary validator.
fn is_modelled_next_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 4096
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_=-".contains(c))
}

/// Open `count` tunnels and return their ids in the order they were opened.
async fn open_tunnels(router: &MultiServiceRouter, count: usize) -> Vec<String> {
    let mut ids = Vec::with_capacity(count);
    for index in 0..count {
        let input = json!({
            "destinationConfig": {"thingName": format!("thing-{index}"), "services": ["SSH"]}
        });
        let (status, opened) = call(router, "OpenTunnel", &input).await;
        assert_eq!(status, 200, "OpenTunnel #{index} answered {opened:?}");
        ids.push(
            opened["tunnelId"]
                .as_str()
                .unwrap_or_else(|| panic!("OpenTunnel #{index} reported no tunnelId: {opened:?}"))
                .to_string(),
        );
    }
    ids
}

/// The token a listing of page size `page_size` hands back, or `None` when the
/// listing fits in one page.
async fn minted_token(router: &MultiServiceRouter, page_size: i64) -> Option<String> {
    let (status, listed) = call(router, "ListTunnels", &json!({"maxResults": page_size})).await;
    assert_eq!(
        status, 200,
        "page size {page_size}: ListTunnels answered {listed:?}"
    );
    listed
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The ids a listing resumed with `token` reports, newest first.
async fn page_after(router: &MultiServiceRouter, token: &str) -> Vec<String> {
    let (status, listed) = call(router, "ListTunnels", &json!({"nextToken": token})).await;
    assert_eq!(
        status, 200,
        "token {token:?}: a token this service minted must be accepted, answered {listed:?}"
    );
    listed["tunnelSummaries"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|summary| summary["tunnelId"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Every token this service mints is `tnl-` followed by exactly 16 lowercase hex
/// digits, and it resumes the listing exactly where its page stopped.
#[tokio::test]
async fn a_minted_next_token_is_canonical_and_resumes_the_listing() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    // Eleven tunnels, so a creation sequence past 0x0f exists and a minted
    // token carries hex letters as well as digits.
    let opened = open_tunnels(&router, 11).await;
    let newest_first: Vec<String> = opened.iter().rev().cloned().collect();

    // Rows: each page size up to and past the one that pages, so the token is
    // minted at several positions in the listing.
    for page_size in 1..=4_i64 {
        let token = minted_token(&router, page_size)
            .await
            .unwrap_or_else(|| panic!("page size {page_size}: the listing minted no nextToken"));
        let digits = token.strip_prefix("tnl-").unwrap_or_else(|| {
            panic!("page size {page_size}: minted token {token:?} carries no 'tnl-' prefix")
        });
        assert_eq!(
            digits.len(),
            16,
            "page size {page_size}: minted token {token:?} must carry 16 digits, got {}",
            digits.len()
        );
        assert!(
            digits.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "page size {page_size}: minted token {token:?} must be lowercase hex"
        );
        assert!(
            is_modelled_next_token(&token),
            "page size {page_size}: minted token {token:?} must match the modelled pattern"
        );

        let resumed = page_after(&router, &token).await;
        let expected: Vec<String> = newest_first
            .iter()
            .skip(usize::try_from(page_size).unwrap_or(1))
            .cloned()
            .collect();
        assert_eq!(
            resumed, expected,
            "page size {page_size}: the minted token must resume after its page, got {resumed:?}"
        );
    }
}

/// A string the service cannot have minted is refused with a 400
/// `ValidationException` and serves no page, however close to a real cursor it
/// looks.
#[tokio::test]
async fn list_tunnels_refuses_every_non_canonical_next_token() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    open_tunnels(&router, 11).await;
    let token = minted_token(&router, 1)
        .await
        .expect("a listing of eleven tunnels at page size 1 mints a nextToken");
    let digits = token
        .strip_prefix("tnl-")
        .expect("a minted token carries the 'tnl-' prefix")
        .to_string();

    // One row per way a cursor can be garbled, each a function of the token the
    // service really minted: (label, forged token).
    let forged: Vec<(&str, String)> = vec![
        ("the digits re-cased to upper case", format!("tnl-{}", digits.to_ascii_uppercase())),
        ("the leading zeros stripped", format!("tnl-{}", digits.trim_start_matches('0'))),
        ("one extra leading zero", format!("tnl-0{digits}")),
        ("four extra leading zeros", format!("tnl-0000{digits}")),
        ("the last digit dropped", format!("tnl-{}", &digits[..digits.len() - 1])),
        ("every digit dropped", "tnl-".to_string()),
        ("the prefix dropped", digits.clone()),
        ("the prefix re-cased", format!("TNL-{digits}")),
        (
            "a non-hex digit substituted",
            format!("tnl-{}z", &digits[..digits.len() - 1]),
        ),
        ("a trailing '=' appended", format!("{token}=")),
        ("a token-shaped string of another service", "ListTunnels-0".to_string()),
    ];

    for (label, candidate) in forged {
        assert_ne!(
            candidate, token,
            "case {label:?}: the row must differ from the minted token {token:?}"
        );
        assert!(
            is_modelled_next_token(&candidate),
            "case {label:?}: the row must match the modelled pattern so it reaches the handler, \
             got {candidate:?}"
        );
        let (status, body) = call(&router, "ListTunnels", &json!({"nextToken": candidate})).await;
        assert_eq!(
            status, 400,
            "case {label:?} ({candidate:?}): a token this service cannot mint must be refused, \
             answered {body:?}"
        );
        assert_eq!(
            body["__type"].as_str(),
            Some("ValidationException"),
            "case {label:?} ({candidate:?}): answered {body:?}"
        );
        assert!(
            body.get("tunnelSummaries").is_none(),
            "case {label:?} ({candidate:?}): a refusal must serve no page, answered {body:?}"
        );
    }

    // The minted token itself is still accepted, so the refusals above are
    // about the form of the string and not about paging being broken.
    let (status, listed) = call(&router, "ListTunnels", &json!({"nextToken": token})).await;
    assert_eq!(
        status, 200,
        "the minted token {token:?} must still be accepted, answered {listed:?}"
    );
}
