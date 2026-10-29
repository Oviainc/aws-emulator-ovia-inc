// SPDX-License-Identifier: CC0-1.0

//! What a listing accepts as a page cursor, driven through every listing of this
//! service in one table, end to end over the wire.
//!
//! THE RULE UNDER TEST: a listing resumes only from a `NextToken` THIS listing itself
//! minted — the whole of what its cursor codec hands out (`crate::state::page_cursor`:
//! the scope that listing mints in, a creation sequence its own family has actually
//! minted, the separator, and the non-empty record name it was minted with) — and
//! refuses every other token with the modelled `BadRequestException` naming the listing
//! that would have handed one out, instead of silently answering the FIRST page again.
//! A client looping `while NextToken` with a stale, truncated or foreign cursor is told
//! its cursor is invalid rather than re-reading page one forever.
//!
//! The rows differ along what the rule generalises over: the listing (so the record
//! family, the cursor format it hands out and the operation its refusal must name),
//! and the FORM the alien token takes — a bare number with neither scope, separator nor
//! name (in range and out of it), the position without the scope, an in-scope cursor
//! whose sequence this listing's family never minted, a sequence that is not a `u64` at
//! all, a half of the position missing, and the other codec's format. The accepted side
//! is swept over every minted sequence, the published ends of that range and one past
//! it, and pins what the cursor skips.
//!
//! A cursor ANOTHER listing of the same store minted is the sibling rule, driven over
//! every pair of listings in `crate::list_cursor_scope_rule`.
//!
//! The listings, their creates and the names created are
//! `crate::list_paging_rule`'s own table, reused rather than copied; every call goes
//! through the same `MultiServiceRouter` the production router dispatches on, reusing
//! the transcription-job suite's harness (`crate::transcription_jobs`).

use serde_json::json;

use service_com_amazonaws_transcribe_transcribe::state::DEFAULT_REGION;

use crate::list_paging_rule::{listings, Listing, Order, CREATED_IN_ORDER};
use crate::transcription_jobs::Transcribe;

/// The modelled client error every listing of this service refuses an alien cursor
/// with, and the status its awsJson1_1 wire protocol puts on a client error (both
/// spanned by `crate::client_error_wire_status`).
///
/// `pub(crate)`, with [`refusal`]: `crate::list_cursor_scope_rule` refuses the cursors
/// of OTHER listings in the same words rather than writing a second copy of them.
pub(crate) const REFUSAL_CODE: &str = "BadRequestException";
/// See [`REFUSAL_CODE`].
pub(crate) const CLIENT_ERROR_STATUS: u16 = 400;

/// The listings paged by a creation sequence: every one but the Call Analytics
/// categories, whose records carry no sequence and whose cursor is a name
/// (`crate::state::name_page_cursor`).
///
/// `pub(crate)`: `crate::list_cursor_spelling_rule` drives the same set of listings.
pub(crate) fn sequence_paged() -> Vec<Listing> {
    listings()
        .into_iter()
        .filter(|listing| listing.order == Order::NewestFirst)
        .collect()
}

/// The refusal text a listing owes an alien cursor, naming the operation whose
/// previous answer the caller should have copied one from.
pub(crate) fn refusal(operation: &str) -> String {
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

/// A token carrying `position` behind the scope `listing` mints its cursors in: that
/// operation and the Region its endpoint serves (these calls carry no SigV4 credential
/// scope, so they are served in `DEFAULT_REGION`).
///
/// `pub(crate)`, with [`minted_cursor`]: `crate::list_cursor_spelling_rule` forges the
/// POSITION behind this very scope rather than writing a second copy of the format.
pub(crate) fn scoped(listing: &Listing, position: &str) -> String {
    format!("{}|{DEFAULT_REGION}|{position}", listing.operation)
}

/// A cursor in the whole of the format `listing` hands one out in: its scope, the
/// creation sequence to resume after, and the name that record was minted with.
pub(crate) fn minted_cursor(listing: &Listing, sequence: u64, name: &str) -> String {
    scoped(listing, &format!("{sequence}|{name}"))
}

/// Every form of token `listing` never handed out, for a store whose family of it has
/// minted `minted` sequences (1..=`minted`) and holds a record named `stored`.
fn forged_tokens(listing: &Listing, minted: u64, stored: &str) -> Vec<Forged> {
    vec![
        Forged {
            form: "a bare sequence in range, carrying neither scope, separator nor name",
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
            form: "the position of a page of this very listing, without the scope it \
                   mints its cursors in",
            token: format!("{minted}|{stored}"),
        },
        Forged {
            form: "an in-scope cursor one past the newest sequence minted",
            token: minted_cursor(listing, minted + 1, stored),
        },
        Forged {
            form: "an in-scope cursor far past every sequence minted",
            token: minted_cursor(listing, 999_999, stored),
        },
        Forged {
            form: "an in-scope cursor of 0, which is before the first sequence minted",
            token: minted_cursor(listing, 0, stored),
        },
        Forged {
            form: "an in-scope cursor whose sequence is negative",
            token: scoped(listing, &format!("-1|{stored}")),
        },
        Forged {
            form: "an in-scope cursor whose sequence overflows a u64",
            token: scoped(listing, &format!("99999999999999999999999999|{stored}")),
        },
        Forged {
            form: "an in-scope sequence with no name after the separator",
            token: scoped(listing, &format!("{minted}|")),
        },
        Forged {
            form: "an in-scope name with no sequence before the separator",
            token: scoped(listing, &format!("|{stored}")),
        },
        Forged {
            form: "the scope with no position behind it",
            token: scoped(listing, ""),
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

        for Forged { form, token } in forged_tokens(&listing, minted, CREATED_IN_ORDER[0]) {
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
            let token = minted_cursor(&listing, sequence, CREATED_IN_ORDER[0]);
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
            let token = minted_cursor(&listing, sequence, CREATED_IN_ORDER[index]);
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
        let token = minted_cursor(&listing, minted + 1, CREATED_IN_ORDER[0]);
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
