// SPDX-License-Identifier: CC0-1.0

//! The TEXT of two Route 53 refusals, end to end over the router.
//!
//! Both rules are about how the refusal's `Message` is BUILT, so each is
//! asserted over a table that spans the class it generalises over rather than
//! the one request that exposed it:
//!
//! * a zone id that names no zone is `NoSuchHostedZone` (HTTP 404) whose message
//!   is REQUEST-DERIVED in AWS's fixed form — `No hosted zone found with ID:
//!   <the bare zone id the request asked about>` — from every zone-addressed
//!   operation, whatever the id's length, whether it ever named a zone, and
//!   whether the request spelled it bare or in the `/hostedzone/`-prefixed form
//!   a response hands back;
//! * the other modelled shape that reports the same fact,
//!   `CreateReusableDelegationSet`'s `HostedZoneNotFound` (HTTP 400), reads that
//!   same sentence, so the wording is a property of the fact rather than of the
//!   operation that reports it;
//! * the request that would take the LAST VPC out of a private zone's
//!   association set is `LastVPCAssociation` (HTTP 400) whose message is a
//!   CONSTANT: one fixed sentence, naming neither the VPC nor the zone of the
//!   request it refuses, identical across requests.
//!
//! The harness (`router`, `call`, `Reply`) comes from `hosted_zone_records`, the
//! delegation-set request builder from `reusable_delegation_sets` and the VPC
//! ones from `vpc_associations`/`vpc_disassociation`, so these tests drive the
//! service through the same router a socket-served request goes through.

use crate::error_envelope::ZoneOperation;
use crate::hosted_zone_records::{call, create_zone_ok, router};
use crate::reusable_delegation_sets::create_set;
use crate::vpc_associations::{
    associations, create_private_zone, get_zone, vpc_block, CREATION_VPC, HOME_REGION,
};
use crate::vpc_disassociation::{associate_ok, disassociate};

/// The fixed form AWS words a missing hosted zone in, with the id the request
/// asked about filled in (`crate::state::no_such_hosted_zone`).
fn missing_zone_message(bare_id: &str) -> String {
    format!("No hosted zone found with ID: {bare_id}")
}

/// The CONSTANT sentence `DisassociateVPCFromHostedZone` refuses the last
/// association of a private zone with
/// (`crate::state::LAST_VPC_ASSOCIATION_MESSAGE`).
const LAST_VPC: &str = "Cannot remove last VPC association for private zone";

/// A `VPC` block every row of the not-found table can send: a well-formed id in
/// a modelled region, so the request is refused for its ZONE and nothing else.
fn sound_vpc_block() -> String {
    vpc_block(Some(CREATION_VPC), Some(HOME_REGION))
}

/// One id to ask about: the spelling the request carries in its URI, and the
/// bare id the refusal must name.
struct ZoneIdRow {
    case: &'static str,
    /// The `{Id}` label as the request writes it (URI-encoded where needed).
    requested: String,
    /// The id the message must name — always the BARE one.
    bare: String,
}

#[tokio::test]
async fn every_zone_addressed_operation_words_a_missing_zone_alike() {
    let router = router();

    // A zone that exists until it is deleted, so one row asks about an id that
    // DID name a zone. It carries only its default records, so it deletes.
    let deleted = create_zone_ok(&router, "gone.example.com", "ref-refusal-gone").await;
    let dropped = call(
        &router,
        "DELETE",
        &format!("/2013-04-01/hostedzone/{deleted}"),
        "",
    )
    .await;
    assert_eq!(
        dropped.status, 200,
        "the zone must delete so its id names no zone, got {} with body {}",
        dropped.status, dropped.body
    );

    // `ResourceId` is `len 0..32`
    // (smithy_query(query="shape",shape="ResourceId")), so every spelling below
    // is one the request boundary lets through: the rows cross the id's length
    // (including the member's upper bound), whether it ever named a zone, and
    // the two spellings of the same id.
    let rows = vec![
        ZoneIdRow {
            case: "a short id that was never minted",
            requested: "ZABC123".to_string(),
            bare: "ZABC123".to_string(),
        },
        ZoneIdRow {
            case: "an unminted id of the length a real one has",
            requested: "Z0123456789ABC".to_string(),
            bare: "Z0123456789ABC".to_string(),
        },
        ZoneIdRow {
            case: "an unminted id at the member's 32-character bound",
            requested: format!("Z{}", "0".repeat(31)),
            bare: format!("Z{}", "0".repeat(31)),
        },
        ZoneIdRow {
            case: "the id of a zone that was deleted",
            requested: deleted.clone(),
            bare: deleted.clone(),
        },
        ZoneIdRow {
            case: "the /hostedzone/-prefixed spelling a response hands back",
            requested: "%2Fhostedzone%2FZABC123".to_string(),
            bare: "ZABC123".to_string(),
        },
    ];

    // The zone-addressed operations that share this not-found branch, each with
    // the URI it addresses a zone id at and a body a request of its own would
    // carry — so the only thing wrong with any of these requests is the zone.
    let operations: [ZoneOperation; 6] = [
        (
            "GetHostedZone",
            "GET",
            |id| format!("/2013-04-01/hostedzone/{id}"),
            String::new,
        ),
        (
            "UpdateHostedZoneComment",
            "POST",
            |id| format!("/2013-04-01/hostedzone/{id}"),
            || {
                "<UpdateHostedZoneCommentRequest \
                 xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
                 <Comment>post-delete</Comment></UpdateHostedZoneCommentRequest>"
                    .to_string()
            },
        ),
        (
            "AssociateVPCWithHostedZone",
            "POST",
            |id| format!("/2013-04-01/hostedzone/{id}/associatevpc"),
            || {
                format!(
                    "<AssociateVPCWithHostedZoneRequest \
                     xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">{}\
                     </AssociateVPCWithHostedZoneRequest>",
                    sound_vpc_block()
                )
            },
        ),
        (
            "DisassociateVPCFromHostedZone",
            "POST",
            |id| format!("/2013-04-01/hostedzone/{id}/disassociatevpc"),
            || {
                format!(
                    "<DisassociateVPCFromHostedZoneRequest \
                     xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">{}\
                     </DisassociateVPCFromHostedZoneRequest>",
                    sound_vpc_block()
                )
            },
        ),
        (
            "GetDNSSEC",
            "GET",
            |id| format!("/2013-04-01/hostedzone/{id}/dnssec"),
            String::new,
        ),
        (
            "ListVPCAssociationAuthorizations",
            "GET",
            |id| format!("/2013-04-01/hostedzone/{id}/authorizevpcassociation"),
            String::new,
        ),
    ];

    for row in &rows {
        for (operation, method, uri_of, body_of) in &operations {
            let case = format!("{operation} on {} ({})", row.requested, row.case);
            let reply = call(&router, method, &uri_of(&row.requested), &body_of()).await;
            assert_eq!(
                reply.status, 404,
                "case {case:?}: expected 404, got {} with body {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "NoSuchHostedZone",
                "case {case:?}: expected NoSuchHostedZone, got body {}",
                reply.body
            );
            let expected = missing_zone_message(&row.bare);
            assert_eq!(
                reply.first("Error/Message"),
                Some(expected.as_str()),
                "case {case:?}: expected message {expected:?}, got {:?}",
                reply.first("Error/Message")
            );
        }
    }
}

#[tokio::test]
async fn the_delegation_sets_zone_refusal_reads_the_same_sentence() {
    // The same sentence answers the OTHER modelled shape that reports a zone id
    // naming no zone: `CreateReusableDelegationSet`'s `HostedZoneNotFound`
    // (HTTP 400, smithy_query(query="shape",shape="HostedZoneNotFound")) — a
    // different code and status for the same fact, so it is worded by the same
    // request-derived rule and names the BARE id whichever spelling arrived.
    let router = router();
    let rows = [
        ("a bare id no zone carries", "ZNOSUCHZONE1", "ZNOSUCHZONE1"),
        (
            "the /hostedzone/-prefixed spelling of that id",
            "/hostedzone/ZNOSUCHZONE1",
            "ZNOSUCHZONE1",
        ),
        ("a short id no zone carries", "ZQ7", "ZQ7"),
    ];
    for (index, (case, requested, bare)) in rows.iter().enumerate() {
        let reply = create_set(&router, &format!("ref-set-{index}"), Some(requested)).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "HostedZoneNotFound",
            "case {case:?}: expected HostedZoneNotFound, got body {}",
            reply.body
        );
        let expected = missing_zone_message(bare);
        assert_eq!(
            reply.first("Error/Message"),
            Some(expected.as_str()),
            "case {case:?}: expected message {expected:?}, got {:?}",
            reply.first("Error/Message")
        );
    }
}

/// One way for a private zone to end up holding exactly one association: the
/// VPC it is left with, the region that VPC was created in, and the VPCs that
/// were associated and removed again on the way there.
struct LastVpcRow {
    case: &'static str,
    survivor: &'static str,
    region: &'static str,
    transient: &'static [(&'static str, &'static str)],
}

#[tokio::test]
async fn the_last_vpc_refusal_is_one_constant_sentence() {
    // The rows differ along the surviving VPC's id form (the 8- and
    // 17-character forms `vpc_id_is_well_formed` accepts), the region it was
    // created in (values of the modelled `VPCRegion` enum,
    // smithy_query(query="shape",shape="VPCRegion")) and how the zone came to
    // hold exactly one association — so a message built from any of those would
    // differ between rows.
    let rows = [
        LastVpcRow {
            case: "the VPC the zone was created with",
            survivor: CREATION_VPC,
            region: HOME_REGION,
            transient: &[],
        },
        LastVpcRow {
            case: "a long-form VPC in another region, the creation VPC removed",
            survivor: "vpc-0a1b2c3d4e5f60718",
            region: "eu-west-1",
            transient: &[],
        },
        LastVpcRow {
            case: "back to one after another VPC was associated and removed",
            survivor: "vpc-1122334455667788a",
            region: "ap-southeast-2",
            transient: &[("vpc-00ff00ff", "us-west-2")],
        },
    ];

    let router = router();
    let mut messages: Vec<String> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let zone_id = create_private_zone(
            &router,
            &format!("last-vpc-text-{index}.example.com"),
            &format!("ref-last-vpc-text-{index}"),
            CREATION_VPC,
        )
        .await;
        if row.survivor != CREATION_VPC {
            associate_ok(&router, &zone_id, row.survivor, row.region).await;
        }
        for (vpc_id, region) in row.transient {
            associate_ok(&router, &zone_id, vpc_id, region).await;
            let removed = disassociate(
                &router,
                &zone_id,
                &vpc_block(Some(vpc_id), Some(region)),
                None,
            )
            .await;
            assert_eq!(
                removed.status, 200,
                "case {case:?}: removing the transient {vpc_id}: expected 200, got {} with \
                 body {}",
                removed.status, removed.body
            );
        }
        if row.survivor != CREATION_VPC {
            let removed = disassociate(
                &router,
                &zone_id,
                &vpc_block(Some(CREATION_VPC), Some(HOME_REGION)),
                None,
            )
            .await;
            assert_eq!(
                removed.status, 200,
                "case {case:?}: removing the creation VPC: expected 200, got {} with body {}",
                removed.status, removed.body
            );
        }

        let before = associations(&get_zone(&router, &zone_id).await);
        assert_eq!(
            before,
            vec![(row.survivor.to_string(), row.region.to_string())],
            "case {case:?}: the zone must be left holding exactly its surviving VPC, got \
             {before:?}"
        );

        // The request that would empty the set.
        let reply = disassociate(
            &router,
            &zone_id,
            &vpc_block(Some(row.survivor), Some(row.region)),
            Some("removing the only association"),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "LastVPCAssociation",
            "case {case:?}: expected LastVPCAssociation, got body {}",
            reply.body
        );
        let message = reply
            .first("Error/Message")
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the refusal must carry a message; body {}",
                    reply.body
                )
            })
            .to_string();
        assert_eq!(
            message, LAST_VPC,
            "case {case:?}: expected the constant refusal {LAST_VPC:?}, got {message:?}"
        );
        // A constant names nothing of the request it refuses.
        assert!(
            !message.contains(&zone_id) && !message.contains(row.survivor),
            "case {case:?}: the constant must name neither the zone {zone_id} nor the VPC {}, \
             got {message:?}",
            row.survivor
        );
        messages.push(message);

        // The effect the refusal must leave alone, and the boundary it guards:
        // the association survives, and the identical request succeeds once a
        // second VPC is associated.
        let after = associations(&get_zone(&router, &zone_id).await);
        assert_eq!(
            after, before,
            "case {case:?}: the refusal must leave the association intact, expected {before:?}, \
             got {after:?}"
        );
        let second = format!("vpc-{index:08x}");
        associate_ok(&router, &zone_id, &second, "us-west-2").await;
        let accepted = disassociate(
            &router,
            &zone_id,
            &vpc_block(Some(row.survivor), Some(row.region)),
            None,
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: the same request must succeed with a second VPC associated, got {} \
             with body {}",
            accepted.status, accepted.body
        );
    }

    assert_eq!(
        messages
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        1,
        "the refusal is a CONSTANT, so every row must read the same message, got {messages:?}"
    );
}
