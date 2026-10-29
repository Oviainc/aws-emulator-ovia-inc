// SPDX-License-Identifier: CC0-1.0

//! HOW a page cursor's position must be SPELLED, driven through every sequence-paged
//! listing of this service in one table, end to end over the wire.
//!
//! THE RULE UNDER TEST: a listing resumes only from a `NextToken` byte-identical to one
//! its own cursor codec would hand out for that position (`crate::state::page_cursor`),
//! so a position written any other way than the one the encoder writes — the position
//! is a `u64`, so that is the canonical decimal spelling: no sign, no leading zeros, no
//! padding and no other radix — is a token this listing never handed out and is refused
//! with the modelled `BadRequestException` naming the listing that would have handed one
//! out. Decoding a forged spelling instead would resume a page from a position no answer
//! of this listing ever carried, and tell the caller nothing about it.
//!
//! The rows differ along what the rule generalises over: the listing (so the record
//! family and the operation its refusal must name), the POSITION the forged token is
//! spelled at (the first sequence minted, one in the middle and the newest — all three
//! in range, so only the spelling can decide), and the FORM the alternative spelling
//! takes (a sign the encoder never writes, leading zeros of several widths, the two
//! combined, surrounding whitespace, a digit separator, another radix, and digits
//! outside ASCII). Every row is paired with the CANONICAL spelling of the very same
//! position, which must still resume the listing strictly past it — the refusal is a
//! refusal of forged spellings, not of paging.
//!
//! The listings, their creates and the names created are `crate::list_paging_rule`'s own
//! table, and the cursor format, the refusal and its status are
//! `crate::list_cursor_rule`'s, all reused rather than copied; every call goes through
//! the same `MultiServiceRouter` the production router dispatches on, reusing the
//! transcription-job suite's harness (`crate::transcription_jobs`).

use serde_json::json;

use crate::list_cursor_rule::{
    minted_cursor, refusal, scoped, sequence_paged, CLIENT_ERROR_STATUS, REFUSAL_CODE,
};
use crate::list_paging_rule::CREATED_IN_ORDER;
use crate::transcription_jobs::Transcribe;

/// One way of writing the position `sequence` that the cursor codec never writes it:
/// the form it takes, and the position as spelled.
struct Spelling {
    form: &'static str,
    position: String,
}

/// Every spelling of `sequence` other than the canonical decimal one the codec writes.
///
/// The first four are the ones Rust's `u64` parser would otherwise accept (an optional
/// leading `+` and any number of leading zeros); the rest span the wider class of
/// positions written some other way than the encoder writes them.
fn forged_spellings(sequence: u64) -> Vec<Spelling> {
    let unicode_digits: String = sequence
        .to_string()
        .chars()
        // The FULLWIDTH DIGIT block: the same digits outside ASCII.
        .map(|digit| char::from_u32(u32::from(digit) - u32::from('0') + 0xFF10).expect("a digit"))
        .collect();
    vec![
        Spelling {
            form: "a leading plus sign",
            position: format!("+{sequence}"),
        },
        Spelling {
            form: "one leading zero",
            position: format!("0{sequence}"),
        },
        Spelling {
            form: "many leading zeros",
            position: format!("00000{sequence}"),
        },
        Spelling {
            form: "a leading plus sign and leading zeros",
            position: format!("+000{sequence}"),
        },
        Spelling {
            form: "a leading space",
            position: format!(" {sequence}"),
        },
        Spelling {
            form: "a trailing space",
            position: format!("{sequence} "),
        },
        Spelling {
            form: "a digit separator",
            position: format!("{sequence}_"),
        },
        Spelling {
            form: "a trailing decimal part",
            position: format!("{sequence}.0"),
        },
        Spelling {
            form: "hexadecimal rather than decimal",
            position: format!("0x{sequence:x}"),
        },
        Spelling {
            form: "the digits written outside ASCII",
            position: unicode_digits,
        },
    ]
}

/// The positions each row is forged at: the first sequence minted, one in the middle of
/// the minted range and the newest. All three are in range and in scope, so the SPELLING
/// is the only thing that can decide the token.
fn minted_positions() -> Vec<u64> {
    let minted = CREATED_IN_ORDER.len() as u64;
    vec![1, minted / 2 + 1, minted]
}

/// The name the record of `sequence` was created under, which is the name its cursor
/// carries.
fn name_of(sequence: u64) -> &'static str {
    let index = usize::try_from(sequence).expect("a sequence fits a usize") - 1;
    CREATED_IN_ORDER[index]
}

/// Every sequence-paged listing refuses a cursor whose position is spelled any way its
/// codec never writes it — while the canonical spelling of that same position still
/// resumes it strictly past that record.
#[tokio::test]
async fn every_sequence_paged_listing_refuses_a_non_canonical_cursor_spelling() {
    for listing in sequence_paged() {
        // A fresh store per listing, so the sequences minted are exactly this family's:
        // 1..=CREATED_IN_ORDER.len(), in creation order.
        let transcribe = Transcribe::new();
        listing.create_the_records(&transcribe).await;

        for sequence in minted_positions() {
            let name = name_of(sequence);

            for Spelling { form, position } in forged_spellings(sequence) {
                let token = scoped(&listing, &format!("{position}|{name}"));
                let case = format!(
                    "{} resumed at position {sequence} written with {form} ({token:?})",
                    listing.operation
                );
                let reply = listing
                    .list(&transcribe, json!({"NextToken": token}))
                    .await;
                reply.refused(&case, CLIENT_ERROR_STATUS, REFUSAL_CODE);
                assert_eq!(
                    reply.error_message(&case),
                    refusal(listing.operation),
                    "case {case:?}: unexpected refusal, got {:?}",
                    reply.body
                );
            }

            // The same position, spelled the one way the codec writes it: accepted, and
            // resuming strictly past that record — the records created before it,
            // newest first.
            let token = minted_cursor(&listing, sequence, name);
            let case = format!(
                "{} resumed at position {sequence} written canonically ({token:?})",
                listing.operation
            );
            let index = usize::try_from(sequence).expect("a sequence fits a usize") - 1;
            let mut expected: Vec<String> = CREATED_IN_ORDER[..index]
                .iter()
                .map(|name| (*name).to_string())
                .collect();
            expected.reverse();
            let (names, next) = listing
                .page(
                    &transcribe,
                    json!({"NextToken": token, "MaxResults": 100}),
                    &case,
                )
                .await;
            assert_eq!(
                names, expected,
                "case {case:?}: the canonical spelling must resume strictly past that \
                 record, got {names:?}"
            );
            assert!(
                next.is_none(),
                "case {case:?}: a page holding every remaining record carries no \
                 cursor, got {next:?}"
            );
        }
    }
}
