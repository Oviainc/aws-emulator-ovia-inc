// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the SPELLING half of this service's page-token rule, over all four
//! of its paginated listings (`ListExports`, `ListTables`, `ListTagsForResource`,
//! `ListExecutions`).
//!
//! The rule under test: a `NextToken` is resumable only when it is, byte for byte, the
//! string the listing itself handed out, so every OTHER spelling of the same token — one
//! that decodes to the identical bytes under the laxer, case-insensitive hex reading — is a
//! string no response of this service ever carried and is refused as a token it never
//! issued: the modelled `ValidationException` (HTTP 400) naming the `NextToken` member with
//! reason `fieldValidationFailed`. [`crate::page_token_refusal`] covers tokens that differ
//! in FORM; this suite covers the ones that differ only in CASE, which decode to a position
//! that really exists in the sequence being paged and so would otherwise resume it.
//!
//! It is asserted over a TABLE crossing each of the four listings with re-spellings that
//! differ along WHICH of an issued token's hex digits are re-cased — every digit, only the
//! first letter, only the last letter, and every other character — each verified to decode
//! to the same bytes as the token the listing did issue, so the case really is a respelling
//! of a genuine position and not just another malformed string. A second test sweeps EVERY
//! single letter position of one issued token, so no position is accepted re-cased, and
//! checks the token as issued still resumes afterwards.
//!
//! The router, the signing, the four-listing table and the shared refusal reader all come
//! from the other suites of this same test target.

use crate::page_token_refusal::{issued_token, refusal, request, seeded, Listing};

/// The hex alphabet the minter writes: `::hex::encode` emits `0-9` and LOWER-case `a-f`
/// only, so a token carrying an upper-case digit is one this service cannot have issued.
const MINTED_HEX_DIGITS: &str = "0123456789abcdef";

/// One re-spelled copy of an issued token: a name for the case, and the string itself.
struct Respelling {
    /// What was re-cased, for the assertion messages.
    case: &'static str,
    /// The re-spelled token.
    token: String,
}

/// `issued` with the hex letter at `letter_index` (counting only the letters, from the
/// front) upper-cased, or `None` when it carries no letter there.
fn upper_case_letter(issued: &str, letter_index: usize) -> Option<String> {
    let position = issued
        .char_indices()
        .filter(|(_, digit)| digit.is_ascii_alphabetic())
        .map(|(at, _)| at)
        .nth(letter_index)?;
    let mut respelt = issued.to_string();
    respelt.replace_range(
        position..position + 1,
        &issued[position..position + 1].to_uppercase(),
    );
    Some(respelt)
}

/// Spellings of `issued` that are not the one it was handed out as: every digit re-cased,
/// only its first letter, only its last letter, and every other character.
///
/// A row whose re-casing changes nothing (a token of digits alone, for which there is no
/// other spelling) is dropped, so every row returned really is a string this service never
/// issued.
fn respellings(issued: &str) -> Vec<Respelling> {
    let letters = issued.chars().filter(char::is_ascii_alphabetic).count();
    let alternating: String = issued
        .chars()
        .enumerate()
        .map(|(at, digit)| {
            if at % 2 == 0 {
                digit
            } else {
                digit.to_ascii_uppercase()
            }
        })
        .collect();
    [
        ("every digit upper-cased", Some(issued.to_uppercase())),
        ("the first letter upper-cased", upper_case_letter(issued, 0)),
        (
            "the last letter upper-cased",
            letters
                .checked_sub(1)
                .and_then(|last| upper_case_letter(issued, last)),
        ),
        ("every other character upper-cased", Some(alternating)),
    ]
    .into_iter()
    .filter_map(|(case, token)| {
        token
            .filter(|token| token != issued)
            .map(|token| Respelling { case, token })
    })
    .collect()
}

/// Assert `respelt` is a re-spelling of `issued` — a different string that the lax hex
/// reading decodes to the very same bytes — so refusing it is refusing a spelling and not
/// merely refusing an unreadable value.
fn assert_respelling_of(case: &str, respelt: &str, issued: &str) {
    assert_ne!(
        respelt, issued,
        "case {case:?}: a re-spelling must differ from the token as issued"
    );
    assert_eq!(
        ::hex::decode(respelt).ok(),
        ::hex::decode(issued).ok(),
        "case {case:?}: a re-spelling must decode to the bytes of the issued token {issued:?}, which is what makes it a position this listing really holds"
    );
    assert!(
        issued
            .chars()
            .all(|digit| MINTED_HEX_DIGITS.contains(digit)),
        "case {case:?}: the token as issued must be spelled in the minted alphabet {MINTED_HEX_DIGITS:?}, got {issued:?}"
    );
    assert!(
        respelt
            .chars()
            .any(|digit| !MINTED_HEX_DIGITS.contains(digit)),
        "case {case:?}: a re-spelling must carry a digit the minter never writes, got {respelt:?}"
    );
}

/// Every listing refuses a re-cased copy of a token it really did issue, exactly as it
/// refuses any other token it never issued, and goes on honouring the token as issued.
#[tokio::test]
async fn every_listing_refuses_a_recased_copy_of_a_token_it_issued() {
    let (client, listings) = seeded().await;

    let mut rows = 0_usize;
    for listing in &listings {
        let issued = issued_token(&client, listing).await;
        // How this listing refuses a value that is plainly not one of its tokens: the
        // benchmark every re-spelling's refusal must match, because a non-canonical
        // spelling is a token it never issued and nothing else.
        let unissued = refusal(&client, listing, "not-a-token-this-service-issued").await;

        let respelt = respellings(&issued);
        assert!(
            !respelt.is_empty(),
            "case {:?}: an issued token carries hex letters, so it has other spellings, got {issued:?}",
            listing.operation
        );
        for Respelling { case, token } in respelt {
            let case = format!("{} with {case}", listing.operation);
            assert_respelling_of(&case, &token, &issued);
            let answered = refusal(&client, listing, &token).await;
            assert_eq!(
                answered, unissued,
                "case {case:?}: a re-cased copy of an issued token must be refused exactly as any token this listing never issued, got {answered:?} against {unissued:?}"
            );
            rows += 1;
        }

        // The token as issued is untouched by the refusals: it still resumes this listing.
        let resumed = client
            .ok(listing.operation, &request(listing, Some(1), Some(&issued)))
            .await;
        assert!(
            resumed["Message"].is_null(),
            "case {:?}: the token this listing issued must still resume it, got {resumed}",
            listing.operation
        );
    }
    assert!(
        rows >= listings.len(),
        "every listing must contribute at least one re-spelling, got {rows} rows for {} listings",
        listings.len()
    );
}

/// No single hex letter of an issued token may be re-cased: the sweep upper-cases each
/// letter position in turn, and every one of those strings is refused.
#[tokio::test]
async fn no_single_letter_of_an_issued_token_may_be_recased() {
    let (client, listings) = seeded().await;
    let listing: &Listing = listings
        .first()
        .expect("the shared table names the paginated listings");
    let issued = issued_token(&client, listing).await;
    let unissued = refusal(&client, listing, "not-a-token-this-service-issued").await;

    let mut swept = 0_usize;
    while let Some(respelt) = upper_case_letter(&issued, swept) {
        let case = format!("{} with letter {swept} upper-cased", listing.operation);
        assert_respelling_of(&case, &respelt, &issued);
        let answered = refusal(&client, listing, &respelt).await;
        assert_eq!(
            answered, unissued,
            "case {case:?}: re-casing one letter of an issued token must leave it a token this listing never issued, got {answered:?} against {unissued:?}"
        );
        swept += 1;
    }
    assert!(
        swept > 0,
        "case {:?}: the sweep must have found at least one letter in {issued:?}",
        listing.operation
    );

    // After the whole sweep the position itself is still resumable, so refusing spellings
    // has not consumed or moved the sequence the token names.
    let resumed = client
        .ok(listing.operation, &request(listing, Some(1), Some(&issued)))
        .await;
    assert!(
        resumed["Message"].is_null(),
        "case {:?}: the token this listing issued must still resume it after the sweep, got {resumed}",
        listing.operation
    );
}
