// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AUTHENTICITY of a `nextToken`, over every paginated
//! listing this service serves.
//!
//! A `nextToken` is opaque: the only positions a listing has are the ones it
//! handed out itself, so a value a caller brings is a position only when this
//! listing really minted it. These tests state that as a rule over a table of
//! values that are shaped exactly like a token of the listing under test — the
//! same prefix, the same listing, the same fixed-width hex fields — and differ
//! from the one it issued only in the position they spell or in the integrity tag
//! that would prove the listing spelled it. Every row must be refused with the
//! modelled HTTP 400, never answered with a page.
//!
//! The rows are built by TAKING APART the token the listing issued, so no test
//! hard-codes a rendering: whatever the service mints, the forgeries below are
//! that token with one field replaced.

use std::collections::BTreeSet;

use serde_json::Value;

use super::app_bundle_ingestion_lifecycle::{encode, Emulator};
use super::pagination_token_lifetime::{
    first_page_token, message, replay, two_records_in_every_listing, Listing,
};

/// A token the service issued, taken apart into the fields it renders.
///
/// The trailing field is the integrity tag, and the two before it are the
/// position and the issue instant; everything ahead of them is the version
/// prefix and the listing name, which is taken as one piece because a listing
/// name may itself contain `-` (a child listing carries its parent's id).
struct Fields {
    prefix: String,
    sequence: String,
    issued_at: String,
    tag: String,
}

/// `token` taken apart into [`Fields`].
fn fields_of(case: &str, token: &str) -> Fields {
    let split = |field: &str, value: &str| {
        value
            .rsplit_once('-')
            .map(|(head, tail)| (head.to_string(), tail.to_string()))
            .unwrap_or_else(|| {
                panic!("case {case:?}: an issued token must carry a {field}, got {token:?}")
            })
    };
    let (fields, tag) = split("tag", token);
    let (fields, issued_at) = split("issue instant", &fields);
    let (prefix, sequence) = split("position", &fields);
    Fields {
        prefix,
        sequence,
        issued_at,
        tag,
    }
}

impl Fields {
    /// These fields re-rendered with `sequence` as the position it names and
    /// `tag` as the trailing tag.
    fn rendered(&self, sequence: &str, tag: &str) -> String {
        format!("{}-{sequence}-{}-{tag}", self.prefix, self.issued_at)
    }

    /// These fields re-rendered with `sequence` as the position and NO tag at
    /// all — the rendering of a token that names a position and offers no proof
    /// the listing handed it out.
    fn untagged(&self, sequence: &str) -> String {
        format!("{}-{sequence}", self.prefix)
    }
}

/// `tag` with its last hex digit changed, so it is a tag of the same shape that
/// belongs to no token.
fn last_digit_changed(tag: &str) -> String {
    let mut digits: Vec<char> = tag.chars().collect();
    if let Some(last) = digits.last_mut() {
        *last = if *last == '0' { '1' } else { '0' };
    }
    digits.into_iter().collect()
}

/// `sequence` with its leading zeros removed: the same NUMBER, rendered as the
/// service never renders it.
fn without_leading_zeros(sequence: &str) -> String {
    let trimmed = sequence.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Every forgery of `issued`: a value shaped like a token of this listing that
/// the listing did not issue. `foreign_tag` is the tag of a token ANOTHER listing
/// issued, so one row carries a tag this service really minted — for other
/// fields.
fn forgeries(listing: &Listing, issued: &str, foreign_tag: &str) -> Vec<(String, String)> {
    let fields = fields_of(listing.case, issued);
    let position = u64::from_str_radix(&fields.sequence, 16).unwrap_or_else(|refusal| {
        panic!(
            "case {:?}: an issued token's position must be hex, got {:?} ({refusal})",
            listing.case, fields.sequence
        )
    });
    let zero = format!("{:016x}", 0_u64);
    let past_the_end = format!("{:016x}", u64::MAX);
    let after_the_issued_one = format!("{:016x}", position.saturating_add(1));

    let rows = vec![
        // The positions no page of this listing ever named: zero, which no record
        // is assigned because the store's counter pre-increments; the top of the
        // range; and the one straight after the position it DID name.
        ("a position of zero on the issued fields", fields.rendered(&zero, &fields.tag)),
        (
            "a position past every sequence the store has assigned, on the issued fields",
            fields.rendered(&past_the_end, &fields.tag),
        ),
        (
            "the position after the one the issued token names",
            fields.rendered(&after_the_issued_one, &fields.tag),
        ),
        // The same positions with no tag at all — the rendering a caller reaches
        // for when it guesses a token is a position written in hex.
        ("a position of zero and no tag", fields.untagged(&zero)),
        (
            "a position past the store's counter and no tag",
            fields.untagged(&past_the_end),
        ),
        ("the issued position with no tag", fields.untagged(&fields.sequence)),
        // The issued fields, with a tag that does not belong to them.
        (
            "the issued fields with the last digit of the tag changed",
            fields.rendered(&fields.sequence, &last_digit_changed(&fields.tag)),
        ),
        (
            "the issued fields with a truncated tag",
            fields.rendered(
                &fields.sequence,
                fields.tag.get(..fields.tag.len().saturating_sub(2)).unwrap_or_default(),
            ),
        ),
        (
            "the issued fields with the tag in upper case",
            fields.rendered(&fields.sequence, &fields.tag.to_uppercase()),
        ),
        (
            "the issued fields with the tag of another listing's token",
            fields.rendered(&fields.sequence, foreign_tag),
        ),
        // The issued position, re-rendered: the same number, so a check on the
        // VALUE alone would read it as the same position.
        (
            "the issued position written without its leading zeros",
            fields.rendered(&without_leading_zeros(&fields.sequence), &fields.tag),
        ),
    ];

    rows.into_iter()
        // A row that renders the issued token itself is no forgery: a tag of
        // decimal digits only is its own upper case.
        .filter(|(_, token)| token != issued)
        .map(|(case, token)| (format!("{} with {case}", listing.case), token))
        .collect()
}

/// RULE: a `nextToken` is a position in a listing only when that listing issued
/// it, so a value shaped like one of its tokens that it never handed out — any
/// position it did not mint, and any re-rendering of one it did — is refused with
/// the modelled HTTP 400 `ValidationException` naming `nextToken`, instead of
/// being read as a position (which answers a forged token with a page) or as the
/// absence of one (which answers it with the first page).
///
/// The table spans WHICH field of an issued token is forged — the position (zero,
/// past the store's counter, the next one along), the tag (absent, altered,
/// truncated, re-cased, another token's) and the rendering of the position — and
/// every paginated listing of the service, because they all mint and read their
/// tokens through one helper.
#[tokio::test]
async fn a_token_shaped_like_one_this_listing_issued_but_never_minted_is_refused() {
    let emulator = Emulator::new();
    let listings = two_records_in_every_listing(&emulator).await;

    // The token each listing really issues for its first page, so every forgery
    // below differs from a genuine token in ONE field.
    let mut issued: Vec<String> = Vec::new();
    for listing in &listings {
        issued.push(first_page_token(&emulator, listing).await);
    }

    for (index, listing) in listings.iter().enumerate() {
        let token = &issued[index];

        // The token exactly as the listing handed it out stays a position: the
        // rows below are about authenticity and nothing else.
        let answer = replay(&emulator, listing, token).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: a listing must honour the token it just issued, got {} with body {}",
            listing.case, answer.status, answer.body
        );

        // A tag this service minted — for another listing's token.
        let foreign = &issued[(index + 1) % issued.len()];
        let foreign_tag = fields_of(listing.case, foreign).tag;

        for (case, forged) in forgeries(listing, token, &foreign_tag) {
            let answer = replay(&emulator, listing, &forged).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400 for {forged:?}, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {}",
                answer.body
            );
            assert_eq!(
                answer.body.get("reason").and_then(Value::as_str),
                Some("fieldValidationFailed"),
                "case {case:?}: expected the refusal to fault the member, got {}",
                answer.body
            );
            let field = answer
                .body
                .get("fieldList")
                .and_then(Value::as_array)
                .and_then(|list| list.first())
                .and_then(|entry| entry.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                field, "nextToken",
                "case {case:?}: expected the refusal to name nextToken, got {field:?} in {}",
                answer.body
            );
            assert!(
                !message(&answer).contains("expired"),
                "case {case:?}: a token this listing never issued must not be reported as \
                 expired, got {:?}",
                message(&answer)
            );
            assert!(
                answer.body.get(listing.items).is_none(),
                "case {case:?}: a refused token must carry no page, got {}",
                answer.body
            );
        }
    }
}

/// RULE: the token a listing DOES issue resumes the walk at the position it
/// names, so following tokens through a listing visits every record exactly once
/// and then stops — the proof the listing carries that makes a forgery refusable
/// leaves a genuine token a position, through the query string and back.
#[tokio::test]
async fn following_the_tokens_a_listing_issues_visits_every_record_once() {
    let emulator = Emulator::new();
    let listings = two_records_in_every_listing(&emulator).await;

    for listing in &listings {
        // The fixture puts TWO records in every listing, and a page holds one, so
        // the walk is a page, a token, a page and no token.
        let expected = 2_usize;
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let query = match &token {
                Some(token) => format!(
                    "{}?maxResults=1&nextToken={}",
                    listing.path,
                    encode(token)
                ),
                None => format!("{}?maxResults=1", listing.path),
            };
            let answer = emulator.call("GET", &query, None).await;
            assert_eq!(
                answer.status, 200,
                "case {:?}: expected HTTP 200 on page {pages}, got {} with body {}",
                listing.case, answer.status, answer.body
            );
            let items = answer
                .body
                .get(listing.items)
                .and_then(Value::as_array)
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: expected {} on page {pages}, got {}",
                        listing.case, listing.items, answer.body
                    )
                });
            assert!(
                items.len() <= 1,
                "case {:?}: a page of maxResults=1 holds at most one record, got {}",
                listing.case,
                answer.body
            );
            seen.extend(items.iter().map(ToString::to_string));
            pages += 1;
            assert!(
                pages <= expected + 1,
                "case {:?}: the walk did not end after {pages} pages",
                listing.case
            );
            token = answer
                .body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }

        assert_eq!(
            seen.len(),
            expected,
            "case {:?}: expected {expected} records across the walk, got {seen:?}",
            listing.case
        );
        let distinct: BTreeSet<&String> = seen.iter().collect();
        assert_eq!(
            distinct.len(),
            seen.len(),
            "case {:?}: expected each record exactly once, got {seen:?}",
            listing.case
        );
    }
}
