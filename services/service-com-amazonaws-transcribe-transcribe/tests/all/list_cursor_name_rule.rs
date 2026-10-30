// SPDX-License-Identifier: CC0-1.0

//! WHICH NAMES a page cursor may carry as its position, driven through every listing of
//! this service in one table, end to end over the wire.
//!
//! THE RULE UNDER TEST: every cursor of this service carries the NAME of the record it
//! resumes after, minted from a name already stored — and every name this service keys a
//! record by is modelled `@length(min: 1, max: 200)` with `@pattern "^[0-9a-zA-Z._-]+$"`
//! (`smithy_query(query="shape", shape="CategoryName,VocabularyName,\
//! VocabularyFilterName,TranscriptionJobName,CallAnalyticsJobName,ModelName")`). So a
//! token whose name part is outside those constraints names a record no Region of this
//! store could ever hold: it is a token this service never handed out, and the listing
//! refuses it with the modelled `BadRequestException` naming itself, rather than taking
//! it as a resume point. Taking one instead positions a page by a token no answer of
//! this listing ever carried — usually an EMPTY page, which tells a paging client the
//! collection is exhausted while the Region still holds records.
//!
//! The rows differ along what the rule generalises over: the listing (so the record
//! family, the cursor format it hands out — both of this service's formats are spanned,
//! since the nine listings include the one paged by name — and the operation its refusal
//! must name), and the FORM the name part takes. The forbidden-character side is swept
//! EXHAUSTIVELY over every printable ASCII character the pattern leaves out rather than
//! sampled, and the length side over the published `@length` bounds and past them. Every
//! row is paired with the accepted side — a name the constraint allows still resumes the
//! listing strictly past that position — so the refusal is a refusal of forged positions,
//! not of paging.
//!
//! The listings, their creates and the names created are `crate::list_paging_rule`'s own
//! table, and the cursor format, the refusal and its status are
//! `crate::list_cursor_rule`'s, all reused rather than copied; every call goes through
//! the same `MultiServiceRouter` the production router dispatches on, reusing the
//! transcription-job suite's harness (`crate::transcription_jobs`).

use serde_json::json;

use service_com_amazonaws_transcribe_transcribe::state::MAX_RECORD_NAME_LENGTH;

use crate::list_cursor_rule::{minted_cursor, refusal, scoped, CLIENT_ERROR_STATUS, REFUSAL_CODE};
use crate::list_paging_rule::{listings, Listing, Order, CREATED_IN_ORDER};
use crate::transcription_jobs::Transcribe;

/// The whole page every row asks for: more than the records this suite creates, so a
/// page that carries a cursor is one with a further record past it and nothing else.
const WHOLE_PAGE: u64 = 100;

/// The characters every name shape of this service is modelled `@pattern
/// "^[0-9a-zA-Z._-]+$"` with, as a predicate: the ASCII alphanumerics plus these three.
fn pattern_allows(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
}

/// Every printable ASCII character that `@pattern` leaves out, swept exhaustively so no
/// row of the class can be the one that is still honoured.
fn characters_the_pattern_forbids() -> Vec<char> {
    (0x20_u8..0x7f)
        .map(char::from)
        .filter(|character| !pattern_allows(*character))
        .collect()
}

/// A token carrying `name` as the POSITION of a page of `listing`, in whichever of this
/// service's two cursor formats that listing hands out: the name alone for the listing
/// paged by name, and a creation sequence this store HAS minted followed by the name for
/// a sequence-paged one.
///
/// Every other part of the token is the one that listing really mints — its scope, and a
/// sequence in the minted range spelled canonically — so in every row below the NAME is
/// the only part of the token that can decide it.
fn cursor_carrying(listing: &Listing, name: &str) -> String {
    let minted = CREATED_IN_ORDER.len();
    match listing.order {
        Order::NameAscending => scoped(listing, name),
        Order::NewestFirst => scoped(listing, &format!("{minted}|{name}")),
    }
}

/// Names no record of this store could be called, one per form a forged position might
/// take: the form it takes, and the name itself.
fn names_no_record_could_carry() -> Vec<(String, String)> {
    vec![
        (
            "a name holding the separator this service spells a cursor with".to_string(),
            "cat|1".to_string(),
        ),
        ("a name holding a space".to_string(), "cat 1".to_string()),
        ("a name spelled outside ASCII".to_string(), "caté".to_string()),
        (
            "a name holding a path separator".to_string(),
            "cat/1".to_string(),
        ),
        (
            "an ARN where a name belongs".to_string(),
            "arn:aws:transcribe:us-east-1:123456789012:analytics-category/cat.1".to_string(),
        ),
        (
            format!("a name one character past the modelled ceiling of {MAX_RECORD_NAME_LENGTH}"),
            "z".repeat(MAX_RECORD_NAME_LENGTH + 1),
        ),
        (
            "a name far past the modelled ceiling".to_string(),
            "z".repeat(500),
        ),
        ("no name at all behind the position".to_string(), String::new()),
    ]
}

/// Names the constraint DOES allow, which a cursor of this service therefore could have
/// been minted from — at each published bound of `@length(min: 1, max: 200)` and
/// spanning every character class `@pattern` allows.
///
/// Every one of them sorts BELOW each name this suite creates (they all begin with a
/// digit, and the created names all begin with a letter), so a listing paged by name
/// resumes from one of them with its whole family — which is what tells an accepted
/// cursor from a refused one.
fn names_the_constraint_allows() -> Vec<(String, String)> {
    vec![
        ("a name at the modelled floor of one character".to_string(), "0".to_string()),
        (
            format!("a name at the modelled ceiling of {MAX_RECORD_NAME_LENGTH}"),
            "0".repeat(MAX_RECORD_NAME_LENGTH),
        ),
        (
            "a name spanning every character class the pattern allows".to_string(),
            "0.9_aZ-z".to_string(),
        ),
    ]
}

/// Every listing refuses a cursor whose name part is outside the constraint every name
/// this service keys a record by is modelled with — in the same words, each naming the
/// listing that would have handed a cursor out.
#[tokio::test]
async fn every_listing_refuses_a_cursor_whose_name_no_record_could_carry() {
    for listing in listings() {
        // A fresh store per listing, so the sequences its family has minted are exactly
        // 1..=CREATED_IN_ORDER.len() and the sequence every row below carries is one
        // this store really minted.
        let transcribe = Transcribe::new();
        listing.create_the_records(&transcribe).await;

        for (form, name) in names_no_record_could_carry() {
            let token = cursor_carrying(&listing, &name);
            let case = format!("{} resumed from {form} ({token:?})", listing.operation);
            let reply = listing
                .list(&transcribe, json!({"NextToken": token, "MaxResults": WHOLE_PAGE}))
                .await;
            reply.refused(&case, CLIENT_ERROR_STATUS, REFUSAL_CODE);
            assert_eq!(
                reply.error_message(&case),
                refusal(listing.operation),
                "case {case:?}: the refusal must name the listing that would have handed \
                 a cursor out, got {:?}",
                reply.body
            );
        }
    }
}

/// The forbidden-character side of the same rule, swept over EVERY printable ASCII
/// character the name pattern leaves out, for every listing: a cursor whose name part
/// carries one is refused, and the same name with that character dropped — which the
/// pattern allows — is accepted.
#[tokio::test]
async fn every_listing_refuses_each_character_the_name_pattern_forbids() {
    for listing in listings() {
        let transcribe = Transcribe::new();
        listing.create_the_records(&transcribe).await;

        for character in characters_the_pattern_forbids() {
            // The character at the end, in the middle and at the start: the pattern
            // anchors the WHOLE name, so none of the three positions can be a name this
            // service stored.
            for name in [
                format!("0.9{character}"),
                format!("0.{character}9"),
                format!("{character}0.9"),
            ] {
                let token = cursor_carrying(&listing, &name);
                let case = format!(
                    "{} resumed from a name carrying {character:?} ({token:?})",
                    listing.operation
                );
                let reply = listing
                    .list(&transcribe, json!({"NextToken": token, "MaxResults": WHOLE_PAGE}))
                    .await;
                reply.refused(&case, CLIENT_ERROR_STATUS, REFUSAL_CODE);
                assert_eq!(
                    reply.error_message(&case),
                    refusal(listing.operation),
                    "case {case:?}: the refusal must name the listing that would have \
                     handed a cursor out, got {:?}",
                    reply.body
                );
            }
        }

        // The same position with nothing the pattern forbids in it: accepted, so the
        // sweep above refuses the forbidden characters and not the paging.
        let token = cursor_carrying(&listing, "0.9");
        let case = format!(
            "{} resumed from a name the pattern allows ({token:?})",
            listing.operation
        );
        listing
            .list(&transcribe, json!({"NextToken": token, "MaxResults": WHOLE_PAGE}))
            .await
            .ok(&case);
    }
}

/// The accepted side, over both cursor formats: a position whose name the constraint
/// allows is a position this listing could have minted, so it resumes the listing
/// strictly past it — even when no record is named that any more, which is what keeps
/// paging stable across a record deleted between two pages.
#[tokio::test]
async fn a_cursor_whose_name_the_constraint_allows_resumes_the_listing() {
    for listing in listings() {
        let transcribe = Transcribe::new();
        listing.create_the_records(&transcribe).await;
        let expected = listing.expected_order();

        for (form, name) in names_the_constraint_allows() {
            let (token, expected) = match listing.order {
                // The name IS the position: each of these names sorts below every name
                // this suite created, so the page resumed from it holds the whole family
                // in name order.
                Order::NameAscending => (cursor_carrying(&listing, &name), expected.clone()),
                // Only the sequence is interpreted, so the newest sequence minted
                // resumes past the newest record whatever name travels with it.
                Order::NewestFirst => (
                    minted_cursor(&listing, CREATED_IN_ORDER.len() as u64, &name),
                    expected[1..].to_vec(),
                ),
            };
            let case = format!("{} resumed from {form} ({token:?})", listing.operation);
            let (names, next) = listing
                .page(
                    &transcribe,
                    json!({"NextToken": token, "MaxResults": WHOLE_PAGE}),
                    &case,
                )
                .await;
            assert_eq!(
                names, expected,
                "case {case:?}: a cursor the constraint allows must resume strictly past \
                 its position, got {names:?}"
            );
            assert!(
                next.is_none(),
                "case {case:?}: a page holding every remaining record carries no cursor, \
                 got {next:?}"
            );
        }
    }
}

/// The one listing paged by NAME, resumed from a name the constraint allows that sorts
/// ABOVE every record the Region holds: a cursor it could have minted for a category
/// since deleted, so it is accepted and answers the empty tail of the collection rather
/// than a refusal.
#[tokio::test]
async fn the_name_paged_listing_resumes_past_its_last_name_with_an_empty_page() {
    let listing = listings()
        .into_iter()
        .find(|listing| listing.order == Order::NameAscending)
        .expect("one listing of this service is paged by name");
    let transcribe = Transcribe::new();
    listing.create_the_records(&transcribe).await;

    // Every created name begins with a letter, so a name of 'z's sorts above all of
    // them — and it is at the modelled ceiling, which the constraint allows.
    let token = cursor_carrying(&listing, &"z".repeat(MAX_RECORD_NAME_LENGTH));
    let case = format!("{} resumed past its last name ({token:?})", listing.operation);
    let (names, next) = listing
        .page(
            &transcribe,
            json!({"NextToken": token, "MaxResults": WHOLE_PAGE}),
            &case,
        )
        .await;
    assert!(
        names.is_empty(),
        "case {case:?}: a position above every stored name has nothing after it, got \
         {names:?}"
    );
    assert!(
        next.is_none(),
        "case {case:?}: a page with nothing after it carries no cursor, got {next:?}"
    );
}
