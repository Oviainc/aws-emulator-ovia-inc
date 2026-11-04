// SPDX-License-Identifier: CC0-1.0

//! WHICH listing a page cursor belongs to, driven over every ordered PAIR of this
//! service's listings and over two Regions of each one, end to end over the wire.
//!
//! THE RULE UNDER TEST: a `NextToken` names a position in the one total order the
//! listing that handed it out pages — that operation over the records of the Region its
//! endpoint serves — so only that listing resumes from it, and every other listing
//! refuses it with the modelled `BadRequestException` naming itself. Accepting it
//! instead would answer a position in a different order: an EMPTY page, telling the
//! caller its collection is exhausted while the Region holds records, or the FIRST page
//! again with a fresh cursor, which is a `while NextToken` loop that never ends.
//!
//! The rows differ along what the rule generalises over: the listing that MINTED the
//! cursor and the listing it is PRESENTED to (every ordered pair of the nine listings,
//! over ONE store holding records of every family, so a cursor of each family really
//! exists to replay), and the Region a cursor was minted in against the Region it is
//! presented to (one operation, two endpoints, each holding records of its own). Both
//! token formats of this service are spanned, since the nine listings include the one
//! paged by name (`crate::state::name_page_cursor`).
//!
//! The listings, their creates and the names created are `crate::list_paging_rule`'s
//! own table, and the refusal is `crate::list_cursor_rule`'s, both reused rather than
//! copied; every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, reusing the transcription-job suite's harness
//! (`crate::transcription_jobs`).

use serde_json::json;

use service_com_amazonaws_transcribe_transcribe::service::medical_scribe_availability::ACCOUNT_NOT_AUTHORIZED;

use crate::list_cursor_rule::{refusal, CLIENT_ERROR_STATUS, REFUSAL_CODE};
use crate::list_paging_rule::{listings, CREATED_IN_ORDER};
use crate::transcription_jobs::Transcribe;

/// The whole page any row of this suite asks for: more than the records it created, so
/// a page that is cursored is one with a FURTHER record past it and nothing else.
const WHOLE_PAGE: u64 = 100;

/// Two Regions this emulator serves, each with records of its own: a cursor is minted
/// through the first endpoint and replayed against the second.
///
/// Both are shaped like a Region identifier, which is what this service keys a record
/// by when a request carries a SigV4 credential scope naming one
/// (`crate::state::request_region`).
const MINTING_REGION: &str = "us-west-2";
/// See [`MINTING_REGION`].
const OTHER_REGION: &str = "eu-central-1";

/// A cursor one listing handed out is refused by every OTHER listing of the same store
/// — and still resumes the listing that minted it, which is what tells the refusal of
/// an alien cursor from a refusal of paging.
///
/// One store holding records of EVERY family, so each listing's cursor names a position
/// that exists in its own order and in no other listing's.
#[tokio::test]
async fn a_cursor_one_listing_minted_is_refused_by_every_other() {
    let transcribe = Transcribe::new();
    let listings = listings();
    for listing in &listings {
        listing.create_the_records(&transcribe).await;
    }

    for minter in &listings {
        let expected = minter.expected_order();
        let case = format!("{} first page of one", minter.operation);
        let (names, token) = minter
            .page(&transcribe, json!({"MaxResults": 1}), &case)
            .await;
        assert_eq!(
            names,
            vec![expected[0].clone()],
            "case {case:?}: the first page must hold the first record of this \
             listing's own order, got {names:?}"
        );
        let token = token.unwrap_or_else(|| {
            panic!("case {case:?}: a page with records past it must carry a cursor")
        });

        for presented_to in &listings {
            if presented_to.operation == minter.operation {
                continue;
            }
            let case = format!(
                "{} resumed with the cursor {} handed out ({token:?})",
                presented_to.operation, minter.operation
            );
            let reply = presented_to
                .list(
                    &transcribe,
                    json!({"NextToken": token.clone(), "MaxResults": WHOLE_PAGE}),
                )
                .await;
            reply.refused(&case, CLIENT_ERROR_STATUS, REFUSAL_CODE);
            assert_eq!(
                reply.error_message(&case),
                refusal(presented_to.operation),
                "case {case:?}: the refusal must name the listing that would have \
                 handed a cursor out, got {:?}",
                reply.body
            );
        }

        let case = format!(
            "{} resumed with the cursor it handed out itself",
            minter.operation
        );
        let (names, next) = minter
            .page(
                &transcribe,
                json!({"NextToken": token, "MaxResults": WHOLE_PAGE}),
                &case,
            )
            .await;
        assert_eq!(
            names,
            expected[1..].to_vec(),
            "case {case:?}: the minting listing must resume with the rest of its own \
             family, got {names:?}"
        );
        assert!(
            next.is_none(),
            "case {case:?}: a page holding every remaining record carries no cursor, \
             got {next:?}"
        );
    }
}

/// A cursor minted through one Region's endpoint is refused by the SAME listing served
/// in another Region, which holds records — and its own Region resumes from it.
#[tokio::test]
async fn a_cursor_minted_in_one_region_is_refused_by_the_same_listing_elsewhere() {
    // Each Region gets records under names of its own, so a cursor replayed in the
    // other Region could only ever answer that Region's records: whether it answered
    // an empty page or that Region's first page again, the names tell which.
    let minted_here = &CREATED_IN_ORDER[..2];
    let created_there = &CREATED_IN_ORDER[2..4];

    for listing in listings() {
        let transcribe = Transcribe::new();
        // A listing whose family is not served in both Regions below has no second
        // endpoint to replay its cursor against: the Medical Scribe family is offered
        // only where AWS offers AWS HealthScribe
        // (`service::medical_scribe_availability`), and in either Region here the
        // listing is refused outright — asserted below, with the sentence itself pinned
        // by `crate::medical_scribe_region_availability`. Its cursor scope across
        // LISTINGS is still driven by the sibling test above, which is served in the
        // Region an unsigned request belongs to.
        if !listing.is_served_in(MINTING_REGION) || !listing.is_served_in(OTHER_REGION) {
            for region in [MINTING_REGION, OTHER_REGION] {
                if listing.is_served_in(region) {
                    continue;
                }
                let case = format!("{} is not served in {region}", listing.operation);
                let reply = listing
                    .list_in(&transcribe, json!({"MaxResults": 1}), Some(region))
                    .await;
                reply.refused(&case, CLIENT_ERROR_STATUS, REFUSAL_CODE);
                assert_eq!(
                    reply.error_message(&case),
                    ACCOUNT_NOT_AUTHORIZED,
                    "case {case:?}: expected the refusal of a family this Region does not \
                     serve, got {:?}",
                    reply.body
                );
            }
            continue;
        }
        listing
            .create_records(&transcribe, minted_here, Some(MINTING_REGION))
            .await;
        listing
            .create_records(&transcribe, created_there, Some(OTHER_REGION))
            .await;
        let expected = listing.order_of(minted_here);

        let case = format!("{} in {MINTING_REGION} first page of one", listing.operation);
        let (names, token) = listing
            .page_in(
                &transcribe,
                json!({"MaxResults": 1}),
                Some(MINTING_REGION),
                &case,
            )
            .await;
        assert_eq!(
            names,
            vec![expected[0].clone()],
            "case {case:?}: the first page must hold the first record this Region \
             holds, got {names:?}"
        );
        let token = token.unwrap_or_else(|| {
            panic!("case {case:?}: a page with records past it must carry a cursor")
        });

        let case = format!(
            "{} in {OTHER_REGION} resumed with the cursor {MINTING_REGION} handed out \
             ({token:?})",
            listing.operation
        );
        let reply = listing
            .list_in(
                &transcribe,
                json!({"NextToken": token.clone(), "MaxResults": WHOLE_PAGE}),
                Some(OTHER_REGION),
            )
            .await;
        reply.refused(&case, CLIENT_ERROR_STATUS, REFUSAL_CODE);
        assert_eq!(
            reply.error_message(&case),
            refusal(listing.operation),
            "case {case:?}: the refusal must name the listing that would have handed a \
             cursor out, got {:?}",
            reply.body
        );

        let case = format!(
            "{} in {MINTING_REGION} resumed with the cursor it handed out itself",
            listing.operation
        );
        let (names, next) = listing
            .page_in(
                &transcribe,
                json!({"NextToken": token, "MaxResults": WHOLE_PAGE}),
                Some(MINTING_REGION),
                &case,
            )
            .await;
        assert_eq!(
            names,
            expected[1..].to_vec(),
            "case {case:?}: the minting Region must resume with the rest of its own \
             records, got {names:?}"
        );
        assert!(
            next.is_none(),
            "case {case:?}: a page holding every remaining record carries no cursor, \
             got {next:?}"
        );
    }
}
