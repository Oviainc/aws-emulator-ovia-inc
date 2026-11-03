// SPDX-License-Identifier: CC0-1.0

//! A `NextToken` is honoured only in the exact spelling the service handed out.
//!
//! The rule under test is one rule for the whole class of paginated listings: a page
//! cursor is opaque and resumable only when it is the token a previous call of the same
//! operation, with the same parameters, actually issued — so any other spelling of the
//! same cursor bytes is input this service never minted, refused as invalid rather than
//! answered with the page that follows.
//!
//! All five list operations take their page through the one shared helper, so every row
//! below is run against all five. Each test is a TABLE whose rows vary what the rule
//! generalises over — the operation, and how many digits of the issued token are
//! respelled and where (all of them, the first, the last, every other one) — rather than
//! one respelling picked by hand. The harness (`Emulator`) is the one `control_catalog.rs`
//! drives the service through: the same `MultiServiceRouter` front door.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::control_catalog::{error_code, Emulator};
use crate::validation_messages::message_of;

/// The sentence AWS refuses an unusable `NextToken` with — the whole message is fixed text,
/// naming neither the submitted token, nor the operation, nor the parameters the token was
/// issued for.
///
/// Spelled out here rather than read off the service's own constant, so a drift in that
/// wording fails this suite instead of being compared against itself. `pub(crate)` so the
/// sibling suite that replays a token under a different filter
/// (`single_resource_filter_paging`) holds the service to the same sentence rather than to a
/// second spelling of it.
pub(crate) const INVALID_NEXT_TOKEN: &str = "The NextToken parameter is not valid or expired.";

/// The five paginated list operations: route and the response member carrying the
/// items. The refusal a token no call issued meets is ONE sentence for the whole
/// service, so no operation name enters it (`OPERATIONS.md`).
const LIST_OPERATIONS: [(&str, &str); 5] = [
    ("/domains", "Domains"),
    ("/objectives", "Objectives"),
    ("/common-controls", "CommonControls"),
    ("/list-controls", "Controls"),
    ("/list-control-mappings", "ControlMappings"),
];

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

/// How many of `token`'s digits are letters, i.e. have a second spelling at all.
fn letter_count(token: &str) -> usize {
    token.chars().filter(char::is_ascii_alphabetic).count()
}

/// Every respelling of `token` this test refuses: the case, and the token as respelt.
fn respellings(token: &str) -> Vec<(String, String)> {
    let letters = letter_count(token);
    assert!(
        letters >= 2,
        "case respellings of {token}: an issued token must carry letter digits to \
         respell (carries {letters})"
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

/// The `NextToken` of a first page of `route`, and the items that page returned.
async fn first_page(emulator: &Emulator, route: &str, items_key: &str) -> (String, Value) {
    let page = emulator.ok(&format!("{route}?maxResults=1"), "{}").await;
    let token = page
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {route}: the seeded catalog holds more than one entry, so a first \
                 page of one must carry a NextToken: {page:?}"
            )
        })
        .to_string();
    let items = page
        .get(items_key)
        .cloned()
        .unwrap_or_else(|| panic!("case {route}: response carries no {items_key}: {page:?}"));
    (token, items)
}

/// A respelling of an issued token names the SAME cursor bytes — so what separates it
/// from the issued token is nothing but how its digits are written.
#[tokio::test]
async fn a_respelt_token_carries_the_same_cursor_bytes_as_the_issued_one() {
    let emulator = Emulator::new();
    for (route, items_key) in LIST_OPERATIONS {
        let (issued, _items) = first_page(&emulator, route, items_key).await;
        let issued_bytes = hex::decode(&issued)
            .unwrap_or_else(|error| panic!("case {route}: an issued token is hex: {error}"));
        for (case, respelt) in respellings(&issued) {
            assert_ne!(
                respelt, issued,
                "case {route} {case}: the respelling must differ from the issued token"
            );
            let respelt_bytes = hex::decode(&respelt).unwrap_or_else(|error| {
                panic!("case {route} {case}: a respelling is still hex: {error}")
            });
            assert_eq!(
                respelt_bytes, issued_bytes,
                "case {route} {case}: expected the issued cursor bytes, got {respelt_bytes:?}"
            );
        }
    }
}

/// Every list operation refuses a respelt token — a token it never issued — with the
/// same `ValidationException` it refuses any other unissued token with, while the token
/// as issued still resumes the page after the one returned.
#[tokio::test]
async fn every_list_operation_refuses_a_respelt_token_and_resumes_the_issued_one() {
    let emulator = Emulator::new();
    // Every sentence the five operations answered with: the refusal is a property of the
    // CLASS, so the set must hold exactly one.
    let mut sentences: BTreeSet<String> = BTreeSet::new();
    for (route, items_key) in LIST_OPERATIONS {
        let (issued, first_items) = first_page(&emulator, route, items_key).await;

        for (case, respelt) in respellings(&issued) {
            let (status, response) = emulator
                .post(&format!("{route}?maxResults=1&nextToken={respelt}"), "{}")
                .await;
            assert_eq!(
                status, 400,
                "case {route} {case}: expected 400, got {status} with {response:?}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {route} {case}: expected a ValidationException, got {response:?}"
            );
            let actual = message_of(&response);
            assert_eq!(
                actual.as_deref(),
                Some(INVALID_NEXT_TOKEN),
                "case {route} {case}: expected the unissued-token refusal, got {actual:?}"
            );
            // No part of the refusal comes from the request or from the operation: the
            // token the caller sent and the operation it called are both absent from it.
            let message = actual.unwrap_or_default();
            assert!(
                !message.contains(&respelt) && !message.contains(&issued),
                "case {route} {case}: the refusal echoes the submitted token, got \
                 {message:?}"
            );
            sentences.insert(message);
        }

        // The positive control: the token as the service spelt it still resumes, and
        // resumes AFTER the entries the first page returned.
        let resumed = emulator
            .ok(&format!("{route}?maxResults=1&nextToken={issued}"), "{}")
            .await;
        let resumed_items = resumed
            .get(items_key)
            .cloned()
            .unwrap_or_else(|| panic!("case {route} resume: no {items_key}: {resumed:?}"));
        assert_ne!(
            resumed_items, first_items,
            "case {route} resume: the page after the issued token must start after the \
             last entry returned"
        );
    }

    assert_eq!(
        sentences.len(),
        1,
        "the unissued-token sentence must be the same for every list operation and every \
         respelling, got {sentences:?}"
    );
}
