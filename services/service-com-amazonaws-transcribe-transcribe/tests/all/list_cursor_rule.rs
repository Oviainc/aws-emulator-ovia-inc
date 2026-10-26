// SPDX-License-Identifier: CC0-1.0

//! What a listing accepts as a page cursor, driven through every listing of this
//! service in one table, end to end over the wire.
//!
//! THE RULE UNDER TEST: a listing resumes only from a `NextToken` this service itself
//! minted — the whole of what its cursor codec hands out (`crate::state::page_cursor`:
//! a creation sequence this store has actually minted, the separator, and the
//! non-empty record name it was minted with) — and refuses every other token with the
//! modelled `BadRequestException` naming the listing that would have handed one out,
//! instead of silently answering the FIRST page again. A client looping `while
//! NextToken` with a stale, truncated or foreign cursor is told its cursor is invalid
//! rather than re-reading page one forever.
//!
//! The rows differ along what the rule generalises over: the listing (so the record
//! family, the cursor format it hands out and the operation its refusal must name),
//! and the FORM the alien token takes — a bare number with neither separator nor name
//! (in range and out of it), an in-format cursor whose sequence this store never
//! minted, a sequence that is not a `u64` at all, a half of the format missing, and
//! the other codec's format. The accepted side is swept over every minted sequence,
//! the published ends of that range and one past it, and pins what the cursor skips.
//!
//! The listings, their creates and the names created are
//! `crate::list_paging_rule`'s own table, reused rather than copied; every call goes
//! through the same `MultiServiceRouter` the production router dispatches on, reusing
//! the transcription-job suite's harness (`crate::transcription_jobs`).

use serde_json::json;

use crate::list_paging_rule::{listings, Listing, Order, CREATED_IN_ORDER};
use crate::transcription_jobs::Transcribe;

/// The modelled client error every listing of this service refuses an alien cursor
/// with, and the status its awsJson1_1 wire protocol puts on a client error (both
/// spanned by `crate::client_error_wire_status`).
const REFUSAL_CODE: &str = "BadRequestException";
/// See [`REFUSAL_CODE`].
const CLIENT_ERROR_STATUS: u16 = 400;

/// The listings paged by a creation sequence: every one but the Call Analytics
/// categories, whose records carry no sequence and whose cursor is a name
/// (`crate::state::name_page_cursor`).
fn sequence_paged() -> Vec<Listing> {
    listings()
        .into_iter()
        .filter(|listing| listing.order == Order::NewestFirst)
        .collect()
}

/// The refusal text a listing owes an alien cursor, naming the operation whose
/// previous answer the caller should have copied one from.
fn refusal(operation: &str) -> String {
    format!(
        "The NextToken in your request is not valid. Use the NextToken returned by \
         your previous {operation} request."
    )
}

/// One alien token: the form it takes, and the token itself.
struct Forged {
    form: &'static str,
    token: String,
}

/// Every form of token this service never handed out, for a store that has minted
/// `minted` sequences (1..=`minted`) and holds a record named `stored`.
fn forged_tokens(minted: u64, stored: &str) -> Vec<Forged> {
    vec![
        Forged {
            form: "a bare sequence in range, carrying neither separator nor name",
            token: "1".to_string(),
        },
        Forged {
            form: "a bare sequence at the newest record minted",
            token: minted.to_string(),
        },
        Forged {
            form: "a bare sequence far past every sequence minted",
            token: "999999".to_string(),
        },
        Forged {
            form: "a cursor one past the newest sequence minted",
            token: format!("{}|{stored}", minted + 1),
        },
        Forged {
            form: "a cursor far past every sequence minted",
            token: format!("999999|{stored}"),
        },
        Forged {
            form: "a cursor of 0, which is before the first sequence minted",
            token: format!("0|{stored}"),
        },
        Forged {
            form: "a cursor whose sequence is negative",
            token: format!("-1|{stored}"),
        },
        Forged {
            form: "a cursor whose sequence overflows a u64",
            token: format!("99999999999999999999999999|{stored}"),
        },
        Forged {
            form: "a sequence with no name after the separator",
            token: format!("{minted}|"),
        },
        Forged {
            form: "a name with no sequence before the separator",
            token: format!("|{stored}"),
        },
        Forged {
            form: "the separator alone",
            token: "|".to_string(),
        },
        Forged {
            form: "a token carrying no sequence at all",
            token: "not-a-cursor".to_string(),
        },
        Forged {
            form: "the name-keyed cursor format, which a sequence-paged listing never \
                   hands out",
            token: format!("name|{stored}"),
        },
    ]
}

/// Every sequence-paged listing refuses every form of cursor it never minted — with
/// the same words, each naming the listing that would have handed one out.
#[tokio::test]
async fn every_sequence_paged_listing_refuses_a_cursor_it_never_minted() {
    let minted = CREATED_IN_ORDER.len() as u64;
    for listing in sequence_paged() {
        // A fresh store per listing, so the sequences minted are exactly this
        // family's: 1..=minted, in creation order.
        let transcribe = Transcribe::new();
        listing.create_the_records(&transcribe).await;

        for Forged { form, token } in forged_tokens(minted, CREATED_IN_ORDER[0]) {
            let case = format!("{} resumed with {form} ({token:?})", listing.operation);
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
    }
}

/// A store that has minted nothing can have handed out no cursor, so it refuses every
/// token — while still answering an empty page to a request that carries none.
#[tokio::test]
async fn a_store_that_minted_nothing_refuses_every_cursor() {
    for listing in sequence_paged() {
        let transcribe = Transcribe::new();
        let case = format!("{} over a store that minted nothing", listing.operation);
        let (names, token) = listing.page(&transcribe, json!({}), &case).await;
        assert!(
            names.is_empty() && token.is_none(),
            "case {case:?}: an empty store lists nothing and hands out no cursor, got \
             {names:?} and {token:?}"
        );

        // In the format a cursor is handed out in, but naming sequences nothing has
        // minted yet.
        for sequence in [1_u64, 5] {
            let token = format!("{sequence}|{}", CREATED_IN_ORDER[0]);
            let case = format!("{case} resumed with {token:?}");
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
    }
}

/// Every sequence this store HAS minted is accepted and resumes strictly past it, one
/// past the newest is not: the sweep over the whole minted range, its two ends
/// included.
///
/// Accepting a cursor is not enough — it must also skip exactly the records at or
/// above it, which is what tells a resumed listing from one restarted at the top.
#[tokio::test]
async fn a_cursor_this_store_minted_resumes_strictly_past_it() {
    let minted = CREATED_IN_ORDER.len() as u64;
    for listing in sequence_paged() {
        let transcribe = Transcribe::new();
        listing.create_the_records(&transcribe).await;

        // The records were created in `CREATED_IN_ORDER`, so the record of sequence
        // `s` is `CREATED_IN_ORDER[s - 1]` and a cursor resuming past `s` leaves the
        // records created before it, newest first.
        for sequence in 1..=minted {
            let index = usize::try_from(sequence).expect("a sequence fits a usize") - 1;
            let token = format!("{sequence}|{}", CREATED_IN_ORDER[index]);
            let case = format!("{} resumed with {token:?}", listing.operation);
            let mut expected: Vec<String> = CREATED_IN_ORDER[..index]
                .iter()
                .map(|name| (*name).to_string())
                .collect();
            expected.reverse();
            let (names, next) = listing
                .page(&transcribe, json!({"NextToken": token, "MaxResults": 100}), &case)
                .await;
            assert_eq!(
                names, expected,
                "case {case:?}: a minted cursor must skip exactly the records at or \
                 above it, got {names:?}"
            );
            assert!(
                next.is_none(),
                "case {case:?}: a page holding every remaining record carries no \
                 cursor, got {next:?}"
            );
        }

        // One past the newest sequence minted is a cursor this store cannot have
        // written, however well-formed it looks.
        let token = format!("{}|{}", minted + 1, CREATED_IN_ORDER[0]);
        let case = format!("{} resumed with {token:?}", listing.operation);
        let reply = listing
            .list(&transcribe, json!({"NextToken": token}))
            .await;
        reply.refused(&case, CLIENT_ERROR_STATUS, REFUSAL_CODE);
    }
}

/// The cursor a listing HANDS OUT is accepted by it, whichever of the two formats that
/// listing pages in: the refusal above is a refusal of alien tokens, not of paging.
#[tokio::test]
async fn the_cursor_a_listing_hands_out_resumes_it() {
    for listing in listings() {
        let transcribe = Transcribe::new();
        listing.create_the_records(&transcribe).await;
        let expected = listing.expected_order();

        let case = format!("{} first page of one", listing.operation);
        let (names, token) = listing
            .page(&transcribe, json!({"MaxResults": 1}), &case)
            .await;
        assert_eq!(
            names,
            vec![expected[0].clone()],
            "case {case:?}: the first page must hold the first record, got {names:?}"
        );
        let token = token.unwrap_or_else(|| {
            panic!("case {case:?}: a page with records past it must carry a cursor")
        });

        let case = format!("{} resumed with the cursor it handed out", listing.operation);
        let (names, _) = listing
            .page(
                &transcribe,
                json!({"NextToken": token, "MaxResults": 100}),
                &case,
            )
            .await;
        assert_eq!(
            names,
            expected[1..].to_vec(),
            "case {case:?}: the resumed page must hold the rest of the family, got \
             {names:?}"
        );
    }
}
