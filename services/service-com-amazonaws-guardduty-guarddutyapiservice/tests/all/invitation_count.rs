// SPDX-License-Identifier: CC0-1.0

//! `GetInvitationsCount`, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handler are exercised
//! together.
//!
//! The rules under test, each asserted over a SET of inboxes rather than one call:
//!
//! 1. The count IS the size of the current account's inbox: it equals the number of
//!    invitations stored for the endpoint the request addressed, and therefore equals
//!    the number `ListInvitations` reports across all its pages — the two read the
//!    same records.
//! 2. The operation names no detector, so the inbox it counts is the one the Region the
//!    request was signed for serves: another Region's endpoint counts its own inbox,
//!    and an endpoint holding no detector counts zero rather than failing.
//! 3. It REPORTS: counting changes nothing a later caller sees — the invitations it
//!    counted stay PENDING, so an acceptance afterwards still succeeds — and an
//!    invitation that HAS been accepted is excluded from the count, because accepting
//!    takes it out of the inbox.
//!
//! The invitations are seeded through this service's own state module (the
//! `negative_class_rejections` suite owns that helper), because the operation that
//! delivers one — `InviteMembers`, called by ANOTHER account — is a scaffold with an
//! issue of its own: this emulator serves a single account, so no request it can
//! answer puts an invitation in its own inbox.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::detector_lifecycle::{call_in, router_over, Router};
use crate::invitation_listing::{enabled_detector_in, page_invitations};
use crate::negative_class_rejections::seed_invitation;

/// The Region most rows use.
const REGION: &str = "us-east-1";

/// How many invitations one inbox can hold — the store's own ceiling, which is also
/// `Invitations`'s `len 0..50` (`smithy_query(operation="ListInvitations")`), so the
/// largest count this operation can ever report.
const MAX_INVITATIONS: usize =
    service_com_amazonaws_guardduty_guarddutyapiservice::state::MAX_INVITATIONS_PER_DETECTOR;

/// One seeded invitation: the inviting account id, the invitation id and the instant it
/// was sent in epoch milliseconds.
pub(crate) type Seed = (String, String, i64);

/// Build `count` invitations that differ along every member the record carries: the
/// inviting account id across the 12-digit `AccountId` space (`len 12..12` —
/// `smithy_query(shape="AccountId")`), the shape of the invitation id (modelled as a
/// plain `String`) and the instant the invitation was sent.
///
/// The inviting account ids ASCEND with the index, which is the order the inbox reads
/// report them in — shared with the `invitation_delete` suite, so neither suite can
/// drift from the other on what an inbox holds.
pub(crate) fn seeds(count: usize) -> Vec<Seed> {
    (0..count)
        .map(|index| {
            (
                format!("{:012}", 100_000_000_000_u64 + index as u64 * 7),
                if index % 2 == 0 {
                    format!("{index:08x}cafed00dcafed00dcafed00d")
                } else {
                    format!("invitation-{index:04}-with-dashes")
                },
                1_700_000_000_000 + index as i64 * 1_111,
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Rule 1: the count is the size of the stored inbox, and agrees with the list.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_count_is_the_number_of_invitations_the_inbox_holds() {
    // The rows differ along the COUNT the inbox holds: none, one, a handful, one below
    // the store's ceiling and the ceiling itself.
    for size in [0_usize, 1, 2, 7, MAX_INVITATIONS - 1, MAX_INVITATIONS] {
        let case = format!("an inbox holding {size} invitation(s)");
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = router_over(Arc::clone(&engine));
        let detector_id = enabled_detector_in(&router, REGION).await;

        // Before any invitation the count is zero — not absent, not canned, not a 501.
        assert_eq!(
            invitations_count(&router, REGION, &case).await,
            0,
            "case {case:?}: an inbox with no invitation must count 0"
        );

        let rows = seeds(size);
        for (index, (account_id, invitation_id, millis)) in rows.iter().enumerate() {
            seed_invitation(&engine, REGION, &detector_id, account_id, invitation_id, *millis);

            // The count tracks the store on every write, not only at the end.
            let seeded = format!("{case}, after seeding {} of {size}", index + 1);
            assert_eq!(
                invitations_count(&router, REGION, &seeded).await,
                (index + 1) as i64,
                "case {seeded:?}: the count must be the number of invitations stored"
            );
        }

        let counted = invitations_count(&router, REGION, &case).await;
        assert_eq!(
            counted, size as i64,
            "case {case:?}: expected {size}, got {counted}"
        );

        // And it agrees with the invitations `ListInvitations` reports: both read the
        // same records, so a count that disagreed with the listing would be a canned
        // number.
        let listed = page_invitations(&router, REGION, "?maxResults=50", &case).await;
        assert_eq!(
            counted,
            listed.len() as i64,
            "case {case:?}: the count must equal the invitations the list reports ({})",
            listed.len()
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the inbox counted is the one the request's Region serves.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_count_is_of_the_inbox_the_region_the_request_was_signed_for_serves() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));

    // One inbox per Region, holding a different number of invitations.
    let rows: [(&str, usize); 3] = [("us-east-1", 3), ("eu-west-1", 1), ("ap-southeast-2", 0)];

    for (region, size) in rows {
        let detector_id = enabled_detector_in(&router, region).await;
        for (account_id, invitation_id, millis) in seeds(size) {
            seed_invitation(&engine, region, &detector_id, &account_id, &invitation_id, millis);
        }
    }

    for (region, size) in rows {
        let case = format!("the {region} inbox");
        let counted = invitations_count(&router, region, &case).await;
        assert_eq!(
            counted, size as i64,
            "case {case:?}: expected only this Region's invitations ({size}), got {counted}"
        );
    }

    // A Region's endpoint that holds no detector at all counts zero: the operation
    // names no detector, so there is nothing for it to refuse.
    let case = "a Region holding no detector";
    let counted = invitations_count(&router, "us-west-2", case).await;
    assert_eq!(
        counted, 0,
        "case {case:?}: expected 0, got {counted}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: counting reports; only an acceptance takes an invitation out.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn counting_changes_nothing_and_an_accepted_invitation_is_excluded() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let detector_id = enabled_detector_in(&router, REGION).await;

    let rows = seeds(4);
    for (account_id, invitation_id, millis) in &rows {
        seed_invitation(&engine, REGION, &detector_id, account_id, invitation_id, *millis);
    }

    // Repeated counts are stable: nothing the operation did consumed an invitation.
    for round in 0..3 {
        let case = format!("count round {round}");
        let counted = invitations_count(&router, REGION, &case).await;
        assert_eq!(
            counted,
            rows.len() as i64,
            "case {case:?}: counting must not change what a later count sees, got {counted}"
        );
    }

    // Nor did it answer an invitation: each one it counted is still PENDING, so an
    // acceptance succeeds — and only that acceptance takes it out of the count, which
    // is how the accepted invitation is excluded.
    for (index, (account_id, invitation_id, _)) in rows.iter().enumerate() {
        let case = format!("accepting {invitation_id:?} from {account_id}");
        let (status, body) = call_in(
            &router,
            REGION,
            "POST",
            &format!("/detector/{detector_id}/administrator"),
            Some(json!({"administratorId": account_id, "invitationId": invitation_id})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the invitation the count included must still be pending, got {status} {body:?}"
        );

        let expected = (rows.len() - index - 1) as i64;
        let counted = invitations_count(&router, REGION, &case).await;
        assert_eq!(
            counted, expected,
            "case {case:?}: an accepted invitation must be excluded from the count, expected {expected}, got {counted}"
        );
    }
}

// ---------------------------------------------------------------------------
// The call and the helpers.
// ---------------------------------------------------------------------------

/// One `GetInvitationsCount` call — `GET /invitation/count`
/// (`smithy_query(operation="GetInvitationsCount")`) — returning the
/// `invitationsCount` it reported and failing the test if it did not succeed or
/// reported no count at all.
async fn invitations_count(router: &Router, region: &str, case: &str) -> i64 {
    let (status, body) = call_in(router, region, "GET", "/invitation/count", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-invitations-count expected 200, got {status} {body:?}"
    );
    body.get("invitationsCount")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: expected an invitationsCount number, got {body:?}"))
}
