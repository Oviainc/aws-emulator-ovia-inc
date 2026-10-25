// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `DisassociateVPCFromHostedZone`.
//!
//! The rules under test, each asserted over a SET of inputs rather than one
//! recorded request:
//!
//! 1. A private hosted zone's VPC associations are a SET keyed by VPC id, so
//!    disassociating removes exactly the named VPC and leaves the rest in the
//!    order they were added: `GetHostedZone` stops reporting it,
//!    `ListHostedZonesByVPC` stops listing the zone for it, and every accepted
//!    request answers with its own `ChangeInfo` that `GetChange` can read back.
//!    The table spans the two VPC id forms, several regions of the modelled
//!    `VPCRegion` enum, the position of the removed association in the list
//!    (first, middle, last) and a shrinking association count.
//! 2. The last association cannot be removed: a zone that holds exactly one
//!    association answers the modelled `LastVPCAssociation` (HTTP 400) and keeps
//!    it, however the zone came to hold just one.
//! 3. A VPC the zone holds no association for is the modelled
//!    `VPCAssociationNotFound` (HTTP 404) — a VPC never associated, one
//!    associated with ANOTHER zone, one already disassociated, and any VPC
//!    against a public zone, which holds no association at all.
//! 4. A request has to name a VPC: an id outside the `vpc-<hex>` form is
//!    `InvalidVPCId` (HTTP 400) and touches nothing.
//! 5. An id that names no zone is `NoSuchHostedZone` (HTTP 404).
//!
//! Every test drives the service through the real router, using the SAME
//! harness `hosted_zone_records` and `vpc_associations` use, so what is asserted
//! is the HTTP/restXml behaviour a Route 53 client sees.

use std::time::Duration;

use smithy_server_runtime::MultiServiceRouter;

use crate::hosted_zone_records::{call, escape_xml, router, Reply};
use crate::vpc_associations::{
    associate, associations, bare_zone_id, create_private_zone, create_zone, get_zone, vpc_block,
    CREATION_VPC, HOME_REGION,
};

/// A `DisassociateVPCFromHostedZone` body carrying `vpc` (a [`vpc_block`]) and,
/// when given, a `Comment`.
fn disassociate_body(vpc: &str, comment: Option<&str>) -> String {
    let comment = comment.map_or_else(String::new, |text| {
        format!("<Comment>{}</Comment>", escape_xml(text))
    });
    format!(
        "<DisassociateVPCFromHostedZoneRequest \
         xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">{vpc}{comment}\
         </DisassociateVPCFromHostedZoneRequest>"
    )
}

/// Send one `DisassociateVPCFromHostedZone` request.
async fn disassociate(
    router: &MultiServiceRouter,
    zone_id: &str,
    vpc: &str,
    comment: Option<&str>,
) -> Reply {
    call(
        router,
        "POST",
        &format!("/2013-04-01/hostedzone/{zone_id}/disassociatevpc"),
        &disassociate_body(vpc, comment),
    )
    .await
}

/// The hosted zones `ListHostedZonesByVPC` reports for one VPC, by bare id.
async fn zones_of_vpc(router: &MultiServiceRouter, vpc_id: &str, region: &str) -> Vec<String> {
    let reply = call(
        router,
        "GET",
        &format!("/2013-04-01/hostedzonesbyvpc?vpcid={vpc_id}&vpcregion={region}"),
        "",
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from ListHostedZonesByVPC, got {} with body {}",
        reply.status, reply.body
    );
    reply
        .all("HostedZoneId")
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// Whether `id` is a `/change/C…` id, as every `ChangeInfo` reports one.
fn is_change_id(id: &str) -> bool {
    id.strip_prefix("/change/")
        .is_some_and(|bare| bare.starts_with('C') && bare.len() == 15)
}

/// One VPC of the suite's association table: the id form and the region it was
/// created in. Each region is a member of the modelled `VPCRegion` enum
/// (smithy_query(query="shape",shape="VPCRegion")).
struct VpcRow {
    case: &'static str,
    vpc_id: &'static str,
    region: &'static str,
}

/// The VPCs a zone is grown with before anything is removed. They span both id
/// forms (8-character original, 17-character long) and regions from three
/// partitions.
const EXTRA_VPCS: &[VpcRow] = &[
    VpcRow {
        case: "the 17-character long id form, in ca-central-1",
        vpc_id: "vpc-0a1b2c3d4e5f60718",
        region: "ca-central-1",
    },
    VpcRow {
        case: "the 8-character original id form, in ap-southeast-2",
        vpc_id: "vpc-1a2b3c4d",
        region: "ap-southeast-2",
    },
    VpcRow {
        case: "a China-partition region",
        vpc_id: "vpc-deadbeef",
        region: "cn-north-1",
    },
    VpcRow {
        case: "a GovCloud region",
        vpc_id: "vpc-0fedcba9876543210",
        region: "us-gov-west-1",
    },
    VpcRow {
        case: "a region added late to the enum",
        vpc_id: "vpc-00112233445566778",
        region: "il-central-1",
    },
];

// ---------------------------------------------------------------------------
// Rule 1: a disassociation removes exactly the named VPC
// ---------------------------------------------------------------------------

#[tokio::test]
async fn disassociating_vpcs_removes_them_from_the_zone_the_read_reports() {
    let router = router();
    let zone_id = create_private_zone(
        &router,
        "shrinking.example.com",
        "disassociate-table",
        CREATION_VPC,
    )
    .await;

    // Grow the zone, so the table below can remove an association from every
    // position of a list that holds several.
    let mut expected = vec![(CREATION_VPC.to_string(), HOME_REGION.to_string())];
    for row in EXTRA_VPCS {
        let reply = associate(
            &router,
            &zone_id,
            &vpc_block(Some(row.vpc_id), Some(row.region)),
            None,
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200 from AssociateVPCWithHostedZone, got {} with body {}",
            row.case, reply.status, reply.body
        );
        expected.push((row.vpc_id.to_string(), row.region.to_string()));
    }

    // Each row removes the association at a different position of the list the
    // zone holds, and the last row names a region OTHER than the one the
    // association was stored with: a VPC id identifies the VPC on its own, so it
    // alone is the identity of an association (`StoredZone::vpc_position`).
    let removals = [
        ("a middle association", "vpc-deadbeef", "cn-north-1"),
        (
            "the first association, the one the zone was created with",
            CREATION_VPC,
            HOME_REGION,
        ),
        (
            "the last association",
            "vpc-00112233445566778",
            "il-central-1",
        ),
        (
            "an association named with another region",
            "vpc-1a2b3c4d",
            "eu-west-1",
        ),
    ];

    let mut change_ids = Vec::new();
    for (case, vpc_id, region) in removals {
        let comment = format!("disassociating {case}");
        let reply = disassociate(
            &router,
            &zone_id,
            &vpc_block(Some(vpc_id), Some(region)),
            Some(&comment),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200 from DisassociateVPCFromHostedZone, got {} with body {}",
            reply.status, reply.body
        );

        let change_id = reply
            .first("ChangeInfo/Id")
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the reply must carry ChangeInfo/Id; body {}",
                    reply.body
                )
            })
            .to_string();
        assert!(
            is_change_id(&change_id),
            "case {case:?}: expected a /change/C… id, got {change_id:?}"
        );
        let status = reply.first("ChangeInfo/Status").unwrap_or("");
        assert!(
            matches!(status, "PENDING" | "INSYNC"),
            "case {case:?}: expected a ChangeStatus, got {status:?}; body {}",
            reply.body
        );
        assert_eq!(
            reply.first("ChangeInfo/Comment"),
            Some(comment.as_str()),
            "case {case:?}: the change must carry the Comment the request gave it; body {}",
            reply.body
        );
        assert!(
            reply.first("ChangeInfo/SubmittedAt").is_some(),
            "case {case:?}: the change must report when it was submitted; body {}",
            reply.body
        );
        change_ids.push(change_id);

        // Exactly the named VPC left, and the rest kept their order.
        expected.retain(|(id, _)| id != vpc_id);
        let read = get_zone(&router, &zone_id).await;
        let got = associations(&read);
        assert_eq!(
            got, expected,
            "case {case:?}: GetHostedZone must report the associations the zone still holds, \
             expected {expected:?}, got {got:?}; body {}",
            read.body
        );

        // The removed VPC no longer resolves this zone, while one the zone
        // still holds still does.
        let listed = zones_of_vpc(&router, vpc_id, region).await;
        assert!(
            !listed.contains(&zone_id),
            "case {case:?}: ListHostedZonesByVPC must stop listing {zone_id} for the \
             disassociated VPC, got {listed:?}"
        );
        if let Some((kept_id, kept_region)) = expected.first() {
            let listed = zones_of_vpc(&router, kept_id, kept_region).await;
            assert!(
                listed.contains(&zone_id),
                "case {case:?}: ListHostedZonesByVPC must still list {zone_id} for the kept \
                 VPC {kept_id}, got {listed:?}"
            );
        }
    }

    assert_eq!(
        change_ids
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        change_ids.len(),
        "each disassociation must get its own change id, got {change_ids:?}"
    );
    // The clock owes the PENDING -> INSYNC transition; nothing polls for it.
    // A blocking sleep, because this crate's tokio does not enable the `time`
    // feature; each test owns its runtime, so nothing else is waiting on it.
    std::thread::sleep(Duration::from_millis(700));
    for change_id in &change_ids {
        let bare = change_id.trim_start_matches("/change/");
        let got = call(&router, "GET", &format!("/2013-04-01/change/{bare}"), "").await;
        assert_eq!(
            got.status, 200,
            "change {change_id:?}: expected 200 from GetChange, got {} with body {}",
            got.status, got.body
        );
        assert_eq!(
            got.first("ChangeInfo/Id"),
            Some(change_id.as_str()),
            "change {change_id:?}: GetChange must return the persisted change; body {}",
            got.body
        );
        assert_eq!(
            got.first("ChangeInfo/Status"),
            Some("INSYNC"),
            "change {change_id:?}: the change must have reached INSYNC; body {}",
            got.body
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the last association cannot be removed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_last_association_cannot_be_disassociated() {
    let router = router();

    // Row 1: the zone still holds only the association `CreateHostedZone` made.
    let untouched = create_private_zone(
        &router,
        "single.example.com",
        "disassociate-single",
        CREATION_VPC,
    )
    .await;

    // Row 2: the zone was grown and then shrunk back to one association, so the
    // refusal depends on the count the zone holds NOW, not on how it got there.
    let shrunk = create_private_zone(
        &router,
        "shrunk.example.com",
        "disassociate-shrunk",
        CREATION_VPC,
    )
    .await;
    let extra = "vpc-0123456789abcdef0";
    let grown = associate(
        &router,
        &shrunk,
        &vpc_block(Some(extra), Some("eu-west-2")),
        None,
    )
    .await;
    assert_eq!(
        grown.status, 200,
        "expected 200 from AssociateVPCWithHostedZone, got {} with body {}",
        grown.status, grown.body
    );
    let removed = disassociate(
        &router,
        &shrunk,
        &vpc_block(Some(extra), Some("eu-west-2")),
        None,
    )
    .await;
    assert_eq!(
        removed.status, 200,
        "expected 200 disassociating the second VPC, got {} with body {}",
        removed.status, removed.body
    );

    // Row 3: a zone whose one association was made by this API rather than by
    // `CreateHostedZone`, after the creation VPC was removed.
    let replaced = create_private_zone(
        &router,
        "replaced.example.com",
        "disassociate-replaced",
        CREATION_VPC,
    )
    .await;
    let only = "vpc-00ff11ee22dd33cc4";
    let added = associate(
        &router,
        &replaced,
        &vpc_block(Some(only), Some("sa-east-1")),
        None,
    )
    .await;
    assert_eq!(
        added.status, 200,
        "expected 200 from AssociateVPCWithHostedZone, got {} with body {}",
        added.status, added.body
    );
    let dropped = disassociate(
        &router,
        &replaced,
        &vpc_block(Some(CREATION_VPC), Some(HOME_REGION)),
        None,
    )
    .await;
    assert_eq!(
        dropped.status, 200,
        "expected 200 disassociating the creation VPC, got {} with body {}",
        dropped.status, dropped.body
    );

    let rows = [
        (
            "the association CreateHostedZone made",
            untouched,
            CREATION_VPC,
            HOME_REGION,
        ),
        (
            "a zone shrunk back to one association",
            shrunk,
            CREATION_VPC,
            HOME_REGION,
        ),
        (
            "an association this API made, now the only one",
            replaced,
            only,
            "sa-east-1",
        ),
    ];

    for (case, zone_id, vpc_id, region) in rows {
        let reply = disassociate(
            &router,
            &zone_id,
            &vpc_block(Some(vpc_id), Some(region)),
            None,
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400 for the last association, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "LastVPCAssociation",
            "case {case:?}: expected LastVPCAssociation, got body {}",
            reply.body
        );

        // The refusal persisted nothing: the zone still holds the association.
        let read = get_zone(&router, &zone_id).await;
        let got = associations(&read);
        assert_eq!(
            got,
            vec![(vpc_id.to_string(), region.to_string())],
            "case {case:?}: a refused disassociation must leave the association in place, \
             got {got:?}; body {}",
            read.body
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a VPC the zone holds no association for
// ---------------------------------------------------------------------------

#[tokio::test]
async fn disassociating_a_vpc_the_zone_does_not_hold_is_a_not_found() {
    let router = router();

    // The zone under test holds two associations, so nothing below could be
    // refused as the last one instead.
    let zone_id = create_private_zone(
        &router,
        "held.example.com",
        "disassociate-not-found",
        CREATION_VPC,
    )
    .await;
    let second = "vpc-0aa11bb22cc33dd44";
    let grown = associate(
        &router,
        &zone_id,
        &vpc_block(Some(second), Some("eu-central-1")),
        None,
    )
    .await;
    assert_eq!(
        grown.status, 200,
        "expected 200 from AssociateVPCWithHostedZone, got {} with body {}",
        grown.status, grown.body
    );

    // A VPC associated with ANOTHER zone only: the association is held BY the
    // zone, so it is not this zone's to remove.
    let elsewhere = "vpc-0bb22cc33dd44ee55";
    let other_zone =
        create_private_zone(&router, "other.example.com", "disassociate-other", elsewhere).await;

    // A VPC this zone held and already gave up.
    let gone = "vpc-0cc33dd44ee55ff66";
    let added = associate(
        &router,
        &zone_id,
        &vpc_block(Some(gone), Some("ap-northeast-1")),
        None,
    )
    .await;
    assert_eq!(
        added.status, 200,
        "expected 200 from AssociateVPCWithHostedZone, got {} with body {}",
        added.status, added.body
    );
    let first_removal = disassociate(
        &router,
        &zone_id,
        &vpc_block(Some(gone), Some("ap-northeast-1")),
        None,
    )
    .await;
    assert_eq!(
        first_removal.status, 200,
        "expected 200 for the first disassociation, got {} with body {}",
        first_removal.status, first_removal.body
    );

    // A public zone, which holds no association at all — and whose associations
    // the read must still report as none afterwards.
    let created = create_zone(&router, "public.example.com", "disassociate-public", "").await;
    assert_eq!(
        created.status, 201,
        "expected 201 from CreateHostedZone, got {} with body {}",
        created.status, created.body
    );
    let public_zone = bare_zone_id(&created);

    let expected_held = vec![
        (CREATION_VPC.to_string(), HOME_REGION.to_string()),
        (second.to_string(), "eu-central-1".to_string()),
    ];
    let rows = [
        (
            "a well-formed VPC this zone never held",
            zone_id.clone(),
            "vpc-0ff00112233445566",
            HOME_REGION,
            expected_held.clone(),
        ),
        (
            "a VPC associated with another zone",
            zone_id.clone(),
            elsewhere,
            HOME_REGION,
            expected_held.clone(),
        ),
        (
            "a VPC this zone already gave up",
            zone_id.clone(),
            gone,
            "ap-northeast-1",
            expected_held.clone(),
        ),
        (
            "any VPC against a public zone",
            public_zone,
            CREATION_VPC,
            HOME_REGION,
            Vec::new(),
        ),
    ];

    for (case, zone, vpc_id, region, expected) in rows {
        let reply = disassociate(&router, &zone, &vpc_block(Some(vpc_id), Some(region)), None).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected 404, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "VPCAssociationNotFound",
            "case {case:?}: expected VPCAssociationNotFound, got body {}",
            reply.body
        );
        // The refusal names what the request asked about: the VPC (its id and
        // the region the request gave for it) and the hosted zone.
        let message = reply.first("Error/Message").unwrap_or("");
        assert_eq!(
            message,
            format!("No VPC association for {vpc_id}:{region} to hosted zone {zone}"),
            "case {case:?}: the refusal must name the VPC and the zone the request asked \
             about, got {message:?}"
        );

        let read = get_zone(&router, &zone).await;
        let got = associations(&read);
        assert_eq!(
            got, expected,
            "case {case:?}: a refused disassociation must leave the zone's associations at \
             {expected:?}, got {got:?}; body {}",
            read.body
        );
    }

    // The zone that holds `elsewhere` kept it throughout.
    let read = get_zone(&router, &other_zone).await;
    let got = associations(&read);
    assert_eq!(
        got,
        vec![(elsewhere.to_string(), HOME_REGION.to_string())],
        "the other zone must keep its own association, got {got:?}; body {}",
        read.body
    );
}

// ---------------------------------------------------------------------------
// Rule 4: the request has to name a VPC
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_request_that_names_no_valid_vpc_is_refused() {
    // A VPC id is `vpc-` plus 8 or 17 lowercase hexadecimal characters, so the
    // rows walk both bounds and one past each, the case rule, the prefix and
    // the absent member.
    let rows = [
        ("no VPCId member at all", None),
        ("an empty VPCId", Some("")),
        ("an id with no vpc- prefix", Some("1a2b3c4d")),
        ("another resource kind's prefix", Some("subnet-1a2b3c4d")),
        (
            "one character short of the 8-character form",
            Some("vpc-1a2b3c4"),
        ),
        (
            "one character past the 8-character form",
            Some("vpc-1a2b3c4d5"),
        ),
        (
            "one character short of the 17-character form",
            Some("vpc-0a1b2c3d4e5f6071"),
        ),
        (
            "one character past the 17-character form",
            Some("vpc-0a1b2c3d4e5f607182"),
        ),
        ("a non-hexadecimal character in the id", Some("vpc-1a2b3c4z")),
        ("an uppercase id body", Some("vpc-1A2B3C4D")),
    ];

    let router = router();
    let zone_id = create_private_zone(
        &router,
        "bad-vpc.example.com",
        "disassociate-bad-vpc",
        CREATION_VPC,
    )
    .await;
    let untouched = vec![(CREATION_VPC.to_string(), HOME_REGION.to_string())];

    for (case, vpc_id) in rows {
        let reply = disassociate(
            &router,
            &zone_id,
            &vpc_block(vpc_id, Some(HOME_REGION)),
            None,
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidVPCId",
            "case {case:?}: expected InvalidVPCId, got body {}",
            reply.body
        );

        let read = get_zone(&router, &zone_id).await;
        let got = associations(&read);
        assert_eq!(
            got, untouched,
            "case {case:?}: a refused request must leave the zone's associations at \
             {untouched:?}, got {got:?}; body {}",
            read.body
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: the zone has to exist
// ---------------------------------------------------------------------------

#[tokio::test]
async fn disassociating_a_vpc_from_an_unknown_zone_is_a_not_found() {
    let router = router();
    // The rows differ along the id shape a caller might pass: the zone-id form,
    // an id of another shape entirely, and the id of a zone that was deleted.
    let doomed = create_private_zone(
        &router,
        "deleted.example.com",
        "disassociate-deleted",
        CREATION_VPC,
    )
    .await;
    let removed = call(
        &router,
        "DELETE",
        &format!("/2013-04-01/hostedzone/{doomed}"),
        "",
    )
    .await;
    assert_eq!(
        removed.status, 200,
        "expected 200 from DeleteHostedZone, got {} with body {}",
        removed.status, removed.body
    );

    for (case, zone_id) in [
        (
            "a well-shaped id no zone carries",
            "ZNOSUCHZONE0001".to_string(),
        ),
        ("an id of another shape", "not-a-zone-id".to_string()),
        ("the id of a deleted zone", doomed),
    ] {
        let reply = disassociate(
            &router,
            &zone_id,
            &vpc_block(Some(CREATION_VPC), Some(HOME_REGION)),
            None,
        )
        .await;
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
    }
}
