// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage that a `nextToken` is honoured ONLY in the exact spelling
//! this service handed out.
//!
//! The RULE under test: both list operations are
//! `@paginated(inputToken=nextToken, outputToken=nextToken, pageSize=maxResults)` with an
//! OPAQUE, server-issued token
//! (`smithy_query(query="operation", operation="ListDomains,ListSubscriptions", model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`),
//! so a `nextToken` is a resume point only when it is BYTE-IDENTICAL to a cursor this
//! service issued for the collection being listed; any other spelling of the same cursor
//! bytes is a value no response of this service ever carried, and is refused with a
//! `ValidationException` (400) rather than answered with the page that follows.
//!
//! Each test is a TABLE run against BOTH listings, whose rows vary what the rule
//! generalises over — how many of the issued token's hex digits are respelt and where
//! (all of them, the first, the last, every other one) — rather than one respelling
//! picked by hand. The accepting half is asserted in the same walk, so "refuse a
//! respelling" cannot be satisfied by refusing every token.
//!
//! The harness (`Emulator`, `error_code`, `string_at`) is the sibling suites': one way of
//! driving this service through signed restJson1 requests, not two.

use serde_json::{json, Value};

use crate::domain_subscription_lifecycle::{error_code, string_at, Emulator};

/// The Region every request of this suite is signed for.
const REGION: &str = "us-west-2";

/// The body member a refused ConnectHealth call reports its detail text in.
const DETAIL_MEMBER: &str = "Message";

/// The refusal both listings answer for a `nextToken` that is not a cursor this service
/// issued for the collection being listed.
const UNISSUED_TOKEN_DETAIL: &str = "nextToken is not a token this service issued";

/// One paginated listing, as a case drives it: the URI with no query string, the output
/// member carrying the page, and the member of one entry that is its cursor key.
struct Listing {
    operation: &'static str,
    base: String,
    member: &'static str,
    key: &'static str,
}

/// Respell `token` by upper-casing the hex digits whose letter positions `select`
/// accepts, leaving every other character as issued.
fn respell(token: &str, select: impl Fn(usize) -> bool) -> String {
    let mut letters = 0usize;
    token
        .chars()
        .map(|digit| {
            if digit.is_ascii_alphabetic() {
                let position = letters;
                letters += 1;
                if select(position) {
                    return digit.to_ascii_uppercase();
                }
            }
            digit
        })
        .collect()
}

/// Every respelling of `token` a case sends back: the label, and the token as respelt.
///
/// A respelling exists only for a digit that is a letter, so the table is built from how
/// many of those the issued token carries.
fn respellings(case: &str, token: &str) -> Vec<(String, String)> {
    let letters = token.chars().filter(char::is_ascii_alphabetic).count();
    assert!(
        letters >= 2,
        "case {case:?}: an issued cursor must carry letter digits to respell, \
         {token:?} carries {letters}"
    );
    vec![
        ("every digit respelt".to_string(), token.to_ascii_uppercase()),
        (
            "the first letter digit respelt".to_string(),
            respell(token, |position| position == 0),
        ),
        (
            "the last letter digit respelt".to_string(),
            respell(token, |position| position == letters - 1),
        ),
        (
            "every other letter digit respelt".to_string(),
            respell(token, |position| position % 2 == 0),
        ),
    ]
}

/// The detail text of a refusal, failing with the whole body when it carries none.
fn detail_text(case: &str, body: &Value) -> String {
    body.get(DETAIL_MEMBER)
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: a refusal must carry a {DETAIL_MEMBER} member, got {body}")
        })
        .to_string()
}

/// The cursor keys one page reported, in the order it reported them.
fn keys(case: &str, listing: &Listing, body: &Value) -> Vec<String> {
    body.get(listing.member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: {} must carry a {} list, got {body}",
                listing.operation, listing.member
            )
        })
        .iter()
        .map(|entry| string_at(entry, listing.key))
        .collect()
}

/// Both listings of this service, each over a collection of three persisted records, so a
/// first page of one always leaves something to resume to.
async fn listings(emulator: &Emulator) -> Vec<Listing> {
    let mut domains: Vec<String> = Vec::new();
    for index in 0..3 {
        let created = emulator
            .create_domain(&json!({ "name": format!("clinic-{index}") }))
            .await;
        domains.push(string_at(&created, "domainId"));
    }
    domains.sort();
    let parent = domains.first().expect("a created domain").clone();
    for _ in 0..3 {
        emulator.create_subscription(&parent).await;
    }

    vec![
        Listing {
            operation: "ListDomains",
            base: "/domain".to_string(),
            member: "domains",
            key: "domainId",
        },
        Listing {
            operation: "ListSubscriptions",
            base: format!("/domains/{parent}/subscriptions"),
            member: "subscriptions",
            key: "subscriptionId",
        },
    ]
}

/// The `nextToken` of a first page of one of `listing`, and the keys that page carried.
async fn first_page(emulator: &Emulator, listing: &Listing) -> (String, Vec<String>) {
    let case = format!("{}: a first page of one", listing.operation);
    let (status, body) = emulator
        .send("GET", &format!("{}?maxResults=1", listing.base), None)
        .await;
    assert_eq!(status, 200, "case {case:?}: got {status} with {body}");
    let token = body
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: a page of one of three carries a cursor: {body}"))
        .to_string();
    (token, keys(&case, listing, &body))
}

/// A respelling names the SAME cursor bytes as the token this service issued — so what
/// separates it from the issued token is nothing but how its digits are written, which is
/// exactly why the spelling is the thing under test.
#[tokio::test]
async fn a_respelt_cursor_carries_the_bytes_of_the_issued_one() {
    let emulator = Emulator::new(REGION);
    for listing in listings(&emulator).await {
        let (issued, _keys) = first_page(&emulator, &listing).await;
        let issued_bytes = hex::decode(&issued).unwrap_or_else(|error| {
            panic!(
                "case {}: an issued cursor is hex, got {issued:?} ({error})",
                listing.operation
            )
        });
        for (label, respelt) in respellings(listing.operation, &issued) {
            let case = format!("{} with {label}", listing.operation);
            assert_ne!(
                respelt, issued,
                "case {case:?}: a respelling must differ from the issued cursor"
            );
            let respelt_bytes = hex::decode(&respelt).unwrap_or_else(|error| {
                panic!("case {case:?}: a respelling is still hex, got {respelt:?} ({error})")
            });
            assert_eq!(
                respelt_bytes, issued_bytes,
                "case {case:?}: expected the issued cursor's bytes, got {respelt_bytes:?}"
            );
        }
    }
}

/// Both listings refuse every respelling of a cursor they issued, with the one refusal
/// this service answers for a token it did not issue — while the cursor AS ISSUED still
/// resumes the page after the one it was handed out with.
#[tokio::test]
async fn both_listings_refuse_a_respelt_cursor_and_resume_the_issued_one() {
    let emulator = Emulator::new(REGION);
    for listing in listings(&emulator).await {
        let (issued, first_keys) = first_page(&emulator, &listing).await;

        for (label, respelt) in respellings(listing.operation, &issued) {
            let case = format!("{} with {label}", listing.operation);
            let path = format!("{}?maxResults=1&nextToken={respelt}", listing.base);
            let (status, body) = emulator.send("GET", &path, None).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a cursor spelling this service never issued must be \
                 refused, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: got {body}"
            );
            let text = detail_text(&case, &body);
            assert_eq!(
                text, UNISSUED_TOKEN_DETAIL,
                "case {case:?}: expected this service's cursor refusal, got {text:?}"
            );
        }

        // The positive control: the cursor as this service spelt it resumes, and resumes
        // strictly after the record the first page reported.
        let case = format!("{} resuming from the cursor as issued", listing.operation);
        let path = format!("{}?maxResults=1&nextToken={issued}", listing.base);
        let (status, body) = emulator.send("GET", &path, None).await;
        assert_eq!(status, 200, "case {case:?}: got {status} with {body}");
        let resumed = keys(&case, &listing, &body);
        assert_eq!(
            resumed.len(),
            1,
            "case {case:?}: expected one record, got {resumed:?}"
        );
        assert!(
            resumed.iter().all(|key| !first_keys.contains(key)),
            "case {case:?}: expected a record after {first_keys:?}, got {resumed:?}"
        );
    }
}
