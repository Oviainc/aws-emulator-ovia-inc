// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of WHICH `nextToken` values a listing accepts.
//!
//! The RULE under test, asserted over tables rather than the one token the issue
//! recorded: both list operations are
//! `@paginated(inputToken=nextToken, outputToken=nextToken)` with an OPAQUE,
//! server-issued token
//! (`smithy_query(query="operation", operation="ListDomains,ListSubscriptions", model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`),
//! so a `nextToken` is a resume point ONLY when this service issued it for the very
//! collection being listed; every other value — invented, well-formed but unissued,
//! altered, or issued for a different collection — is refused with a
//! `ValidationException` (400) instead of being answered with a page computed from a
//! cursor the service never handed out.
//!
//! The rows differ along what the rule generalises over: the token's form (not hex, odd
//! length, hex of plausible text, hex of a key the store really holds, a structurally
//! complete cursor), its size (empty through far longer than any cursor), which
//! collection issued it (the other listing, another Region's listing, another domain's
//! listing), and which field was altered (the tag, or the key the tag covers). The
//! accepting half of the rule is asserted too, over every page size from one to one past
//! the collection's size, so "refuse what was not issued" cannot be satisfied by
//! refusing everything.
//!
//! The harness (`Emulator`, `error_code`, `string_at`) is the sibling suites': one way of
//! driving this service through signed restJson1 requests, not two.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::state::{
    domains_page_scope, encode_page_token, subscriptions_page_scope,
};

use crate::domain_subscription_lifecycle::{error_code, string_at, Emulator};

/// The Region every request of a case is signed for, and a second Region whose listings
/// are a DIFFERENT collection of the same kind.
const REGION: &str = "eu-west-1";
const OTHER_REGION: &str = "ap-south-1";

/// Hex of `text`, the way a client fabricating a cursor out of a key it can see would
/// build one. The codec is the crate's own `hex`, not a second copy of it.
fn hex_of(text: &str) -> String {
    hex::encode(text.as_bytes())
}

/// The plaintext inside a hex-encoded cursor, so a case can alter one field of a cursor
/// the service really issued and send the result back.
fn plaintext_of(token: &str) -> String {
    String::from_utf8(hex::decode(token).expect("a hex cursor")).expect("a UTF-8 plaintext")
}

/// A cursor the service issued, with its leading authentication field altered: the same
/// shape and the same resume key, but a tag this service did not compute.
fn with_altered_tag(token: &str) -> String {
    let plaintext = plaintext_of(token);
    let (tag, rest) = plaintext
        .split_once(':')
        .expect("an issued cursor carries a tag field");
    // Flip the first character to one that is still a hex digit, so only the tag's VALUE
    // differs from the one the service computed.
    let flipped = match tag.chars().next() {
        Some('0') => '1',
        _ => '0',
    };
    let altered: String = std::iter::once(flipped).chain(tag.chars().skip(1)).collect();
    hex_of(&format!("{altered}:{rest}"))
}

/// A cursor the service issued, with the resume key its tag covers replaced by
/// `other_key`: a client moving a genuine cursor to a position of its own choosing.
fn with_altered_key(token: &str, other_key: &str) -> String {
    let plaintext = plaintext_of(token);
    let mut fields = plaintext.splitn(3, ':');
    let tag = fields.next().expect("a tag field");
    let scope = fields.next().expect("a scope field");
    hex_of(&format!("{tag}:{scope}:{other_key}"))
}

/// Walk a listing from its first page to its last, `page_size` at a time, feeding back
/// only the tokens the service itself issued. Returns the keys seen, in order, and
/// asserts every page was accepted.
async fn walk(
    emulator: &Emulator,
    case: &str,
    base: &str,
    member: &str,
    key: &str,
    page_size: usize,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let path = match &token {
            None => format!("{base}?maxResults={page_size}"),
            Some(token) => format!("{base}?maxResults={page_size}&nextToken={token}"),
        };
        let (status, page) = emulator.send("GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: page {pages} of size {page_size} must be accepted, got {page:?}"
        );
        let entries = page
            .get(member)
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: {member} in {page:?}"));
        assert!(
            entries.len() <= page_size,
            "case {case:?}: a page of size {page_size} carried {} entries",
            entries.len()
        );
        seen.extend(entries.iter().map(|entry| string_at(entry, key)));
        pages += 1;
        assert!(
            pages <= 20,
            "case {case:?}: paging at size {page_size} did not finish, saw {seen:?}"
        );
        token = page
            .get("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if token.is_none() {
            break;
        }
    }
    seen
}

/// Assert `path` is refused as a validation error, for every token a listing did not
/// issue.
async fn assert_refused(emulator: &Emulator, cases: &[(String, String)], base: &str) {
    for (case, token) in cases {
        let (status, body) = emulator
            .send("GET", &format!("{base}?nextToken={token}"), None)
            .await;
        assert_eq!(
            status, 400,
            "case {case:?} (nextToken={token:?}): an unissued token must be refused, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?} (nextToken={token:?}): got {body:?}"
        );
    }
}

#[tokio::test]
async fn list_domains_resumes_only_from_a_cursor_it_issued() {
    let emulator = Emulator::new(REGION);
    let mut created: BTreeSet<String> = BTreeSet::new();
    for index in 0..4 {
        let domain = emulator
            .create_domain(&json!({ "name": format!("clinic-{index}") }))
            .await;
        created.insert(string_at(&domain, "domainId"));
    }
    let first_key = created
        .iter()
        .next()
        .expect("a created domain")
        .to_string();
    let last_key = created.iter().next_back().expect("a created domain").clone();

    // The accepting half of the rule: a cursor this listing issued resumes exactly where
    // the page stopped, at every page size from one to one past the collection's size.
    for page_size in 1..=5 {
        let seen = walk(
            &emulator,
            &format!("page size {page_size}"),
            "/domain",
            "domains",
            "domainId",
            page_size,
        )
        .await;
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            seen.len(),
            "page size {page_size}: a domain was returned twice across pages ({seen:?})"
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<_>>(),
            created,
            "page size {page_size}: paging must cover every persisted domain"
        );
    }

    // The cursor this listing issues, used below as the genuine one every altered row
    // starts from.
    let (status, page) = emulator.send("GET", "/domain?maxResults=1", None).await;
    assert_eq!(status, 200, "ListDomains got {page:?}");
    let issued = page
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a first page of one of four domains carries a token: {page:?}"))
        .to_string();
    assert_ne!(
        issued,
        hex_of(&first_key),
        "an issued cursor must not be a value a client can derive from the key it saw"
    );

    // A subscription of one of those domains, so a row can send a cursor the OTHER
    // listing issued.
    let subscription_id = create_subscription(&emulator, &last_key).await;

    let cases = vec![
        // The token the issue recorded: hex of plausible text, even length, decodes to
        // UTF-8 — and still a cursor nothing issued.
        ("hex of invented text".to_string(), hex_of("dom-z")),
        // Hex of a key the store really holds: the forgery a client can actually build,
        // since every page shows it the keys.
        ("hex of a key the store holds".to_string(), hex_of(&first_key)),
        ("not hex at all".to_string(), "zz-not-a-token".to_string()),
        ("odd-length hex".to_string(), "646f6d2d7".to_string()),
        ("empty".to_string(), String::new()),
        (
            "hex of UTF-8 that is not a cursor's shape".to_string(),
            hex_of("tag:domains"),
        ),
        (
            "a cursor of the subscriptions collection".to_string(),
            encode_page_token(
                &subscriptions_page_scope(REGION, &last_key),
                &subscription_id,
            ),
        ),
        (
            "a cursor of this collection in another Region".to_string(),
            encode_page_token(&domains_page_scope(OTHER_REGION), &first_key),
        ),
        (
            "an issued cursor with its tag altered".to_string(),
            with_altered_tag(&issued),
        ),
        (
            "an issued cursor moved to another key".to_string(),
            with_altered_key(&issued, &last_key),
        ),
        (
            "far longer than any cursor".to_string(),
            "61".repeat(600),
        ),
    ];
    assert_refused(&emulator, &cases, "/domain").await;

    // A refused token changes nothing: the listing still reports every domain.
    let (status, page) = emulator.send("GET", "/domain?maxResults=100", None).await;
    assert_eq!(status, 200, "ListDomains got {page:?}");
    let listed: BTreeSet<String> = page
        .get("domains")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("domains in {page:?}"))
        .iter()
        .map(|entry| string_at(entry, "domainId"))
        .collect();
    assert_eq!(
        listed, created,
        "a refused token must leave the listing itself unchanged"
    );
}

#[tokio::test]
async fn list_subscriptions_resumes_only_from_a_cursor_it_issued() {
    let emulator = Emulator::new(REGION);
    let domain_id = string_at(
        &emulator.create_domain(&json!({ "name": "clinic" })).await,
        "domainId",
    );
    let other_domain_id = string_at(
        &emulator.create_domain(&json!({ "name": "other" })).await,
        "domainId",
    );
    let mut created: BTreeSet<String> = BTreeSet::new();
    for _ in 0..3 {
        created.insert(create_subscription(&emulator, &domain_id).await);
    }
    let first_key = created
        .iter()
        .next()
        .expect("a created subscription")
        .to_string();
    let base = format!("/domains/{domain_id}/subscriptions");

    for page_size in 1..=4 {
        let seen = walk(
            &emulator,
            &format!("page size {page_size}"),
            &base,
            "subscriptions",
            "subscriptionId",
            page_size,
        )
        .await;
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<_>>(),
            created,
            "page size {page_size}: paging must cover every persisted subscription ({seen:?})"
        );
    }

    let (status, page) = emulator
        .send("GET", &format!("{base}?maxResults=1"), None)
        .await;
    assert_eq!(status, 200, "ListSubscriptions got {page:?}");
    let issued = page
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a first page of one of three carries a token: {page:?}"))
        .to_string();
    assert_ne!(
        issued,
        hex_of(&first_key),
        "an issued cursor must not be a value a client can derive from the key it saw"
    );

    let domains_cursor = {
        let (status, page) = emulator.send("GET", "/domain?maxResults=1", None).await;
        assert_eq!(status, 200, "ListDomains got {page:?}");
        page.get("nextToken")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("a first page of one of two domains carries a token: {page:?}"))
            .to_string()
    };

    let cases = vec![
        ("hex of invented text".to_string(), hex_of("sub-z")),
        (
            "hex of a key the store holds".to_string(),
            hex_of(&first_key),
        ),
        ("not hex at all".to_string(), "zz-not-a-token".to_string()),
        ("odd-length hex".to_string(), "737562".to_string() + "2"),
        ("empty".to_string(), String::new()),
        (
            "a cursor the domains listing issued".to_string(),
            domains_cursor,
        ),
        (
            "a cursor of another domain's subscriptions".to_string(),
            encode_page_token(
                &subscriptions_page_scope(REGION, &other_domain_id),
                &first_key,
            ),
        ),
        (
            "a cursor of this collection in another Region".to_string(),
            encode_page_token(
                &subscriptions_page_scope(OTHER_REGION, &domain_id),
                &first_key,
            ),
        ),
        (
            "an issued cursor with its tag altered".to_string(),
            with_altered_tag(&issued),
        ),
        (
            "an issued cursor moved to another key".to_string(),
            with_altered_key(&issued, "sub-000000000000000000000"),
        ),
        (
            "far longer than any cursor".to_string(),
            "61".repeat(600),
        ),
    ];
    assert_refused(&emulator, &cases, &base).await;

    // A refused token changes nothing: the listing still reports every subscription.
    let (status, page) = emulator
        .send("GET", &format!("{base}?maxResults=100"), None)
        .await;
    assert_eq!(status, 200, "ListSubscriptions got {page:?}");
    let listed: BTreeSet<String> = page
        .get("subscriptions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("subscriptions in {page:?}"))
        .iter()
        .map(|entry| string_at(entry, "subscriptionId"))
        .collect();
    assert_eq!(
        listed, created,
        "a refused token must leave the listing itself unchanged"
    );
}

/// `CreateSubscription` under `domain_id`, asserting it was accepted, and the created
/// subscription's id.
async fn create_subscription(emulator: &Emulator, domain_id: &str) -> String {
    let (status, subscription) = emulator
        .send(
            "POST",
            &format!("/domains/{domain_id}/subscriptions"),
            Some(&json!({})),
        )
        .await;
    assert_eq!(
        status, 201,
        "CreateSubscription under {domain_id} got {subscription:?}"
    );
    string_at(&subscription, "subscriptionId")
}
