// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `DeleteReusableDelegationSet`.
//!
//! The rules under test, each asserted over a SET of inputs rather than the one
//! request that reported the gap:
//!
//! 1. A reusable delegation set is deletable exactly while NO hosted zone is
//!    created in it: the delete removes the set and answers an empty HTTP 200,
//!    and refuses with `DelegationSetInUse` (HTTP 400) — changing nothing at all
//!    — while any zone still names it. The zones are counted at the moment of
//!    the call, so a set whose zones are deleted becomes deletable again, and a
//!    set is deletable from the start when nothing was ever created in it.
//! 2. A delegation set id that resolves to no set is `NoSuchDelegationSet`
//!    (HTTP 400) whose message is that id ALONE, in its bare form, with no fixed
//!    text around it — the same sentence from every operation that names a
//!    delegation set id, however the request spelled it. A set that was deleted
//!    is such an id, so the delete is reported, not silently repeated.
//!
//! The harness (`router`, `call`, `Reply`) and every request builder come from
//! the suites that own those operations, so these tests drive the service
//! through the same router a socket-served request goes through.

use smithy_server_runtime::MultiServiceRouter;

use crate::hosted_zone_count::delete_zone;
use crate::hosted_zone_records::{call, router, Reply};
use crate::refusal_message_rules::message_of;
use crate::reusable_delegation_sets::{
    bare_set_id, create_set, delegation_set_element, get_set, set_count, set_of,
};
use crate::vpc_associations::{bare_zone_id, create_zone};

/// The one sentence `DelegationSetInUse` reports, which names neither the set
/// nor the zones still created in it.
///
/// `DeleteReusableDelegationSet`'s own documentation states the rule it reports:
/// "You can delete a reusable delegation set only if it isn't associated with
/// any hosted zones."
const IN_USE_MESSAGE: &str = "The specified delegation set contains associated hosted zones.";

/// Send one `DeleteReusableDelegationSet` request, with the `{Id}` label written
/// exactly as `id` spells it.
///
/// `pub(crate)` so a sibling suite that needs a set out of the way drives the
/// same request rather than growing a second copy of it.
pub(crate) async fn delete_set(router: &MultiServiceRouter, id: &str) -> Reply {
    call(
        router,
        "DELETE",
        &format!("/2013-04-01/delegationset/{id}"),
        "",
    )
    .await
}

/// Create a reusable delegation set and return `(the prefixed Id, the bare id)`.
async fn set_ids(router: &MultiServiceRouter, caller_reference: &str) -> (String, String) {
    let created = create_set(router, caller_reference, None).await;
    assert_eq!(
        created.status, 201,
        "CreateReusableDelegationSet({caller_reference}) must answer 201, got {} with body {}",
        created.status, created.body
    );
    let id = set_of(&created, caller_reference).id;
    let bare = bare_set_id(&id).to_string();
    (id, bare)
}

// ---------------------------------------------------------------------------
// Rule 1: deletable exactly while no hosted zone is created in the set
// ---------------------------------------------------------------------------

/// How the row's delete writes the `{Id}` label.
#[derive(Debug, Clone, Copy)]
enum Spelling {
    /// The bare (`N…`) id.
    Bare,
    /// The `/delegationset/`-prefixed `Id` a response handed back, URI-encoded
    /// into the path label.
    Prefixed,
}

/// One row: a set, the hosted zones created in it, and how the delete addresses
/// it.
struct DeleteRow {
    case: &'static str,
    caller_reference: &'static str,
    /// The names of the zones created in the set, in creation order.
    zones: &'static [&'static str],
    spelling: Spelling,
}

#[tokio::test]
async fn a_set_is_deletable_exactly_while_no_hosted_zone_is_created_in_it() {
    // The rows differ along HOW MANY zones hold the set (none, one, two,
    // three) — so the refusal has to be re-checked after each zone is taken out
    // rather than once — and along the form the delete names the set in.
    let rows = [
        DeleteRow {
            case: "a set nothing was ever created in, named by its bare id",
            caller_reference: "delete-empty-bare",
            zones: &[],
            spelling: Spelling::Bare,
        },
        DeleteRow {
            case: "a set nothing was ever created in, named by its prefixed id",
            caller_reference: "delete-empty-prefixed",
            zones: &[],
            spelling: Spelling::Prefixed,
        },
        DeleteRow {
            case: "a set one zone was created in",
            caller_reference: "delete-one-zone",
            zones: &["only.example.com"],
            spelling: Spelling::Bare,
        },
        DeleteRow {
            case: "a set two zones were created in, named by its prefixed id",
            caller_reference: "delete-two-zones",
            zones: &["first.example.org", "second.example.net"],
            spelling: Spelling::Prefixed,
        },
        DeleteRow {
            case: "a set three zones were created in",
            caller_reference: "delete-three-zones",
            zones: &["one.example.com", "two.example.com", "three.example.com"],
            spelling: Spelling::Bare,
        },
    ];

    for row in &rows {
        // A router per row, so one row's zones and sets cannot be read as
        // another's.
        let router = router();
        // A second set every delete in this row must leave exactly where it is.
        let (sentinel_id, sentinel_bare) = set_ids(&router, "untouched-by-the-delete").await;
        let sentinel = set_of(&get_set(&router, &sentinel_bare).await, row.case);

        let (prefixed, bare) = set_ids(&router, row.caller_reference).await;
        let held = set_of(&get_set(&router, &bare).await, row.case);
        assert_eq!(
            set_count(&router).await,
            "2",
            "case {:?}: the account must hold the two created sets",
            row.case
        );

        // The zones that hold the set, created naming it in both forms so the
        // count is of the SET rather than of one spelling of its id.
        let mut zone_ids: Vec<String> = Vec::new();
        for (index, name) in row.zones.iter().enumerate() {
            let named = if index % 2 == 0 { &bare } else { &prefixed };
            let zone = create_zone(
                &router,
                name,
                &format!("{}-zone-{index}", row.caller_reference),
                &delegation_set_element(named),
            )
            .await;
            assert_eq!(
                zone.status, 201,
                "case {:?}: CreateHostedZone({name}) must answer 201, got {} with body {}",
                row.case, zone.status, zone.body
            );
            zone_ids.push(bare_zone_id(&zone));
        }

        let requested = match row.spelling {
            Spelling::Bare => bare.clone(),
            Spelling::Prefixed => format!("%2Fdelegationset%2F{bare}"),
        };

        // While ANY zone still names the set the delete is refused, and the
        // refusal is a no-op: the set still reads back as it was and the account
        // still holds both sets. One pass per remaining zone, so a refusal that
        // only looked at the first zone would pass the last pass wrongly.
        for remaining in (1..=zone_ids.len()).rev() {
            let case = format!("{} with {remaining} zone(s) left in it", row.case);
            let refused = delete_set(&router, &requested).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}: expected HTTP 400, got {} with body {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                "DelegationSetInUse",
                "case {case:?}: expected DelegationSetInUse, got body {}",
                refused.body
            );
            let got = message_of(&refused, &case);
            assert_eq!(
                got, IN_USE_MESSAGE,
                "case {case:?}: expected message {IN_USE_MESSAGE:?}, got {got:?}"
            );

            let read = set_of(&get_set(&router, &bare).await, &case);
            assert_eq!(
                read, held,
                "case {case:?}: a refused delete must leave the set readable as it was, got \
                 {read:?} for {held:?}"
            );
            assert_eq!(
                set_count(&router).await,
                "2",
                "case {case:?}: a refused delete must persist nothing"
            );

            // Take one zone out of the set and go round again.
            delete_zone(&router, &zone_ids[remaining - 1], &case).await;
        }

        // With no zone left naming it the set is deleted, and the success
        // carries no delegation set at all.
        let deleted = delete_set(&router, &requested).await;
        assert_eq!(
            deleted.status, 200,
            "case {:?}: expected HTTP 200 from DeleteReusableDelegationSet, got {} with body {}",
            row.case, deleted.status, deleted.body
        );
        assert_eq!(
            deleted.first("DelegationSet/Id"),
            None,
            "case {:?}: the delete's output has no members, got body {}",
            row.case,
            deleted.body
        );

        // The set is GONE: the read and a repeated delete both refuse, and the
        // account holds one set fewer.
        for (operation, reply) in [
            ("GetReusableDelegationSet", get_set(&router, &bare).await),
            (
                "DeleteReusableDelegationSet",
                delete_set(&router, &requested).await,
            ),
        ] {
            let case = format!("{} read back through {operation}", row.case);
            assert_eq!(
                reply.status, 400,
                "case {case:?}: expected HTTP 400, got {} with body {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "NoSuchDelegationSet",
                "case {case:?}: expected NoSuchDelegationSet, got body {}",
                reply.body
            );
            let got = message_of(&reply, &case);
            assert_eq!(
                got, bare,
                "case {case:?}: expected the bare id {bare:?} as the whole message, got {got:?}"
            );
        }
        assert_eq!(
            set_count(&router).await,
            "1",
            "case {:?}: the deleted set must be out of the account's sets",
            row.case
        );

        // The other set is untouched: the delete removed the set it addressed
        // and nothing else.
        let other = set_of(&get_set(&router, &sentinel_bare).await, row.case);
        assert_eq!(
            other, sentinel,
            "case {:?}: the set at {sentinel_id} must be untouched, got {other:?} for \
             {sentinel:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_set_whose_zones_were_deleted_is_deletable_again_and_its_reference_is_free() {
    let router = router();
    let (prefixed, bare) = set_ids(&router, "reusable-after-teardown").await;

    // Provision zones against the set, tear them down, delete the set: the
    // ordering a client drives cleanup in.
    let zone = create_zone(
        &router,
        "teardown.example.com",
        "teardown-zone",
        &delegation_set_element(&prefixed),
    )
    .await;
    assert_eq!(
        zone.status, 201,
        "CreateHostedZone must answer 201, got {} with body {}",
        zone.status, zone.body
    );
    delete_zone(&router, &bare_zone_id(&zone), "the only zone in the set").await;

    let deleted = delete_set(&router, &bare).await;
    assert_eq!(
        deleted.status, 200,
        "the set must be deletable once its zones are gone, got {} with body {}",
        deleted.status, deleted.body
    );

    // The deleted set's `CallerReference` is the set's, so it goes with the set:
    // a later create may use it again, and the set it mints is a NEW one.
    let recreated = create_set(&router, "reusable-after-teardown", None).await;
    assert_eq!(
        recreated.status, 201,
        "the caller reference of a deleted set must be free again, got {} with body {}",
        recreated.status, recreated.body
    );
    let fresh = set_of(&recreated, "the recreated set");
    assert_ne!(
        fresh.id, prefixed,
        "the recreated set must be a new set, got the deleted set's id {prefixed}"
    );
    assert_eq!(
        set_count(&router).await,
        "1",
        "the account must hold exactly the recreated set"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: an id naming no set is reported as that id alone
// ---------------------------------------------------------------------------

/// One id that names no reusable delegation set.
struct MissingRow {
    case: &'static str,
    /// The `{Id}` label as a path-addressed request writes it (URI-encoded
    /// where needed).
    in_path: &'static str,
    /// The same id as a request BODY carries it, before XML escaping.
    plain: &'static str,
    /// The same id as a QUERY STRING carries it, or `None` for an id a query
    /// string cannot carry literally.
    in_query: Option<&'static str>,
    /// The message every refusal must carry: the bare id, and nothing else.
    expected: &'static str,
}

#[tokio::test]
async fn an_id_naming_no_set_is_reported_as_the_bare_id_alone() {
    // The rows differ along the SPELLING of the id (bare, `/delegationset/`-
    // prefixed, lowercase), its length (one character up to the 32 `ResourceId`
    // allows, smithy_query(query="operation",operation="DeleteReusableDelegationSet"))
    // and the kind of resource it looks like (a delegation set, a hosted zone).
    let rows = [
        MissingRow {
            case: "a bare id no set carries",
            in_path: "N0000000000000000",
            plain: "N0000000000000000",
            in_query: Some("N0000000000000000"),
            expected: "N0000000000000000",
        },
        MissingRow {
            case: "that same id, prefixed as a response would hand it back",
            in_path: "%2Fdelegationset%2FN0000000000000000",
            plain: "/delegationset/N0000000000000000",
            in_query: Some("/delegationset/N0000000000000000"),
            expected: "N0000000000000000",
        },
        MissingRow {
            case: "a hosted zone id, which is not a delegation set id",
            in_path: "Z0000000000000000",
            plain: "Z0000000000000000",
            in_query: Some("Z0000000000000000"),
            expected: "Z0000000000000000",
        },
        MissingRow {
            case: "a one-character id",
            in_path: "N",
            plain: "N",
            in_query: Some("N"),
            expected: "N",
        },
        MissingRow {
            case: "a lowercase id, which no minted id takes",
            in_path: "n1pa6795sample",
            plain: "n1pa6795sample",
            in_query: Some("n1pa6795sample"),
            expected: "n1pa6795sample",
        },
        MissingRow {
            // A caller-influenced id goes into the envelope output-encoded, so
            // the refusal reports it AND stays well-formed XML. A query string
            // cannot carry these characters literally, so this row skips the
            // listing.
            case: "an id carrying XML metacharacters",
            in_path: "%3Cid%3E%26%22",
            plain: "<id>&\"",
            in_query: None,
            expected: "<id>&\"",
        },
    ];

    let router = router();
    // One set the account DOES hold, so the refusals below are about the id
    // asked for rather than about an empty store.
    let (_, held) = set_ids(&router, "the-only-set").await;

    for (index, row) in rows.iter().enumerate() {
        // Every operation that names a delegation set id reads the same
        // sentence: the id, bare, on its own.
        let mut replies = vec![
            (
                "DeleteReusableDelegationSet",
                delete_set(&router, row.in_path).await,
            ),
            (
                "GetReusableDelegationSet",
                get_set(&router, row.in_path).await,
            ),
            (
                "CreateHostedZone",
                create_zone(
                    &router,
                    &format!("missing-set-{index}.example.com"),
                    &format!("missing-set-{index}"),
                    &delegation_set_element(row.plain),
                )
                .await,
            ),
        ];
        if let Some(in_query) = row.in_query {
            replies.push((
                "ListHostedZones",
                call(
                    &router,
                    "GET",
                    &format!("/2013-04-01/hostedzone?delegationsetid={in_query}"),
                    "",
                )
                .await,
            ));
        }

        for (operation, reply) in &replies {
            let case = format!("{} through {operation}", row.case);
            assert_eq!(
                reply.status, 400,
                "case {case:?}: expected HTTP 400, got {} with body {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "NoSuchDelegationSet",
                "case {case:?}: expected NoSuchDelegationSet, got body {}",
                reply.body
            );
            let got = message_of(reply, &case);
            assert_eq!(
                got, row.expected,
                "case {case:?}: expected the bare id {:?} as the whole message, got {got:?}",
                row.expected
            );
            // The id is output-encoded into the envelope rather than written
            // into it raw, so the body stays well-formed whatever it carried.
            assert!(
                !reply.body.contains("<id>"),
                "case {case:?}: the id must be encoded into the envelope, got body {}",
                reply.body
            );
        }
    }

    // The set the store does hold is still there, and still deletable.
    let read = get_set(&router, &held).await;
    assert_eq!(
        read.status, 200,
        "the stored set must still read back, got {} with body {}",
        read.status, read.body
    );
    let deleted = delete_set(&router, &held).await;
    assert_eq!(
        deleted.status, 200,
        "the stored set must be deletable, got {} with body {}",
        deleted.status, deleted.body
    );
}
