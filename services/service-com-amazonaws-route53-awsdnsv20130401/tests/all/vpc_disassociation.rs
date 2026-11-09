// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `DisassociateVPCFromHostedZone`, read back through
//! `GetHostedZone`, `ListHostedZonesByVPC` and `GetChange`.
//!
//! The rules under test, each asserted over a SET of inputs rather than one
//! recorded request:
//!
//! 1. A private hosted zone's VPC associations are a SET keyed by VPC id, so
//!    disassociating a VPC takes exactly the named entry out of the list
//!    `GetHostedZone` reports and `ListHostedZonesByVPC` searches by, leaves
//!    every other association where it was, and answers HTTP 200 with its own
//!    `ChangeInfo` that `GetChange` reads back. The table spans both VPC id
//!    forms, several regions of the modelled `VPCRegion` enum, and which entry
//!    of the set is removed (the first, one in the middle, the last).
//! 2. A zone keeps at least one association: the request that would empty the
//!    list is `LastVPCAssociation` (HTTP 400) and leaves the association intact,
//!    while the identical request succeeds once a second VPC is associated. The
//!    table spans how the zone came to hold exactly one VPC.
//! 3. A request naming a VPC the zone does not hold asks to remove an
//!    association that is not there, which is `VPCAssociationNotFound`
//!    (HTTP 404): never associated, already disassociated, associated with
//!    ANOTHER zone, or any VPC at all against a public zone — none of them
//!    touching the set.
//! 4. The request has to name a VPC, exactly as the association that created it
//!    did: an id outside the `vpc-<hex>` form is `InvalidVPCId` and a region the
//!    model does not name is `InvalidInput` (both HTTP 400), and neither touches
//!    the zone.
//! 5. An id that names no zone is `NoSuchHostedZone` (HTTP 404).
//! 6. A disassociated VPC frees its place in the set, so it can be associated
//!    again: association and disassociation are inverses.
//!
//! Every test drives the service through the real router, using the SAME
//! harness `hosted_zone_records` and `vpc_associations` use, so what is asserted
//! is the HTTP/restXml behaviour a Route 53 client sees.

use std::time::Duration;

use smithy_server_runtime::MultiServiceRouter;

use crate::hosted_zone_records::{call, escape_xml, router, router_holding_vpcs, Reply};
use crate::vpc_associations::{
    associate, associations, bare_zone_id, create_private_zone, create_zone, get_zone, vpc_block,
    CREATION_VPC, HOME_REGION,
};

/// Send one `DisassociateVPCFromHostedZone` request carrying `vpc` (a
/// [`vpc_block`]) and, when given, a `Comment`.
///
/// `pub(crate)` so a sibling suite drives the same request rather than growing
/// a second copy of its body.
pub(crate) async fn disassociate(
    router: &MultiServiceRouter,
    zone_id: &str,
    vpc: &str,
    comment: Option<&str>,
) -> Reply {
    let comment = comment.map_or_else(String::new, |text| {
        format!("<Comment>{}</Comment>", escape_xml(text))
    });
    let body = format!(
        "<DisassociateVPCFromHostedZoneRequest \
         xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">{vpc}{comment}\
         </DisassociateVPCFromHostedZoneRequest>"
    );
    call(
        router,
        "POST",
        &format!("/2013-04-01/hostedzone/{zone_id}/disassociatevpc"),
        &body,
    )
    .await
}

/// Associate `vpc_id` in `region` with `zone_id` and assert it was accepted.
///
/// `pub(crate)` for the same reason [`disassociate`] is.
pub(crate) async fn associate_ok(
    router: &MultiServiceRouter,
    zone_id: &str,
    vpc_id: &str,
    region: &str,
) {
    let reply = associate(
        router,
        zone_id,
        &vpc_block(Some(vpc_id), Some(region)),
        None,
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "associating {vpc_id} in {region}: expected 200, got {} with body {}",
        reply.status, reply.body
    );
}

/// The bare zone ids `ListHostedZonesByVPC` reports for one VPC — the other
/// read over the same association set.
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
        "listing the zones of {vpc_id}: expected 200, got {} with body {}",
        reply.status, reply.body
    );
    reply
        .all("HostedZoneSummary/HostedZoneId")
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// Whether `id` is a `/change/C…` id, as every `ChangeInfo` reports one.
fn is_change_id(id: &str) -> bool {
    id.strip_prefix("/change/")
        .is_some_and(|bare| bare.starts_with('C') && bare.len() == 15)
}

/// The `ChangeInfo/Id` of an accepted reply, asserted to have the shape every
/// change id has.
fn change_id(case: &str, reply: &Reply) -> String {
    let id = reply
        .first("ChangeInfo/Id")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the reply must carry ChangeInfo/Id; body {}",
                reply.body
            )
        })
        .to_string();
    assert!(
        is_change_id(&id),
        "case {case:?}: expected a /change/C… id, got {id:?}"
    );
    id
}

// ---------------------------------------------------------------------------
// Rule 1: the named association, and only it, leaves the set
// ---------------------------------------------------------------------------

/// One row of the disassociation table: which member of the zone's association
/// set is removed, named by its id form and the region it was created in.
struct RemovalRow {
    case: &'static str,
    vpc_id: &'static str,
    region: &'static str,
}

#[tokio::test]
async fn disassociating_a_vpc_removes_exactly_that_association() {
    // The zone is built with associations spanning both VPC id forms (the
    // 8-character original and the 17-character long form) and several regions
    // of the modelled `VPCRegion` enum
    // (smithy_query(query="shape",shape="VPCRegion")), including other
    // partitions. The rows then remove a member from a different place in the
    // set each time: the first, one in the middle, and the last.
    let members = [
        ("vpc-0a1b2c3d4e5f60718", "ca-central-1"),
        ("vpc-1a2b3c4d", "ap-southeast-2"),
        ("vpc-00112233445566778", "il-central-1"),
        ("vpc-deadbeef", "cn-north-1"),
        ("vpc-0fedcba9876543210", "us-gov-west-1"),
    ];
    let rows = [
        RemovalRow {
            case: "the first association the zone holds",
            vpc_id: CREATION_VPC,
            region: HOME_REGION,
        },
        RemovalRow {
            case: "an association in the middle of the set",
            vpc_id: "vpc-00112233445566778",
            region: "il-central-1",
        },
        RemovalRow {
            case: "the last association the set holds",
            vpc_id: "vpc-0fedcba9876543210",
            region: "us-gov-west-1",
        },
        RemovalRow {
            case: "the 8-character original id form",
            vpc_id: "vpc-1a2b3c4d",
            region: "ap-southeast-2",
        },
    ];

    let router = router_holding_vpcs(
        &[&[(CREATION_VPC, HOME_REGION)][..], &members[..]].concat(),
    );
    let zone_id = create_private_zone(
        &router,
        "disassociate.example.com",
        "disassociate-table",
        CREATION_VPC,
    )
    .await;
    let mut expected = vec![(CREATION_VPC.to_string(), HOME_REGION.to_string())];
    for (vpc_id, region) in members {
        associate_ok(&router, &zone_id, vpc_id, region).await;
        expected.push((vpc_id.to_string(), region.to_string()));
    }

    let mut change_ids = Vec::new();
    for row in &rows {
        // The VPC's own listing reports the zone while the association stands.
        let listed = zones_of_vpc(&router, row.vpc_id, row.region).await;
        assert!(
            listed.contains(&zone_id),
            "case {:?}: ListHostedZonesByVPC must report the zone while the association \
             stands, got {listed:?}",
            row.case
        );

        let comment = format!("disassociating {}", row.case);
        let reply = disassociate(
            &router,
            &zone_id,
            &vpc_block(Some(row.vpc_id), Some(row.region)),
            Some(&comment),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200 from DisassociateVPCFromHostedZone, got {} with body {}",
            row.case, reply.status, reply.body
        );
        change_ids.push(change_id(row.case, &reply));

        let status = reply.first("ChangeInfo/Status").unwrap_or("");
        assert!(
            matches!(status, "PENDING" | "INSYNC"),
            "case {:?}: expected a ChangeStatus, got {status:?}; body {}",
            row.case,
            reply.body
        );
        assert_eq!(
            reply.first("ChangeInfo/Comment"),
            Some(comment.as_str()),
            "case {:?}: the change must carry the Comment the request gave it; body {}",
            row.case,
            reply.body
        );
        assert!(
            reply.first("ChangeInfo/SubmittedAt").is_some(),
            "case {:?}: the change must report when it was submitted; body {}",
            row.case,
            reply.body
        );

        // Exactly the named entry left, in place: every other association keeps
        // its position and its region.
        expected.retain(|(id, _)| id != row.vpc_id);
        let read = get_zone(&router, &zone_id).await;
        let got = associations(&read);
        assert_eq!(
            got, expected,
            "case {:?}: GetHostedZone must report the associations the zone still holds, \
             expected {expected:?}, got {got:?}; body {}",
            row.case, read.body
        );
        let listed = zones_of_vpc(&router, row.vpc_id, row.region).await;
        assert!(
            !listed.contains(&zone_id),
            "case {:?}: ListHostedZonesByVPC must stop reporting the zone for the removed \
             VPC, got {listed:?}",
            row.case
        );
    }

    assert_eq!(
        change_ids
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        change_ids.len(),
        "each disassociation must get its own change id, got {change_ids:?}"
    );
    // The clock owes the PENDING -> INSYNC transition; nothing polls for it. A
    // blocking sleep, because this crate's tokio does not enable the `time`
    // feature; each test owns its runtime, so nothing else is waiting on it.
    std::thread::sleep(Duration::from_millis(700));
    for (row, id) in rows.iter().zip(change_ids.iter()) {
        let bare = id.trim_start_matches("/change/");
        let got = call(&router, "GET", &format!("/2013-04-01/change/{bare}"), "").await;
        assert_eq!(
            got.status, 200,
            "case {:?}: expected 200 from GetChange, got {} with body {}",
            row.case, got.status, got.body
        );
        assert_eq!(
            got.first("ChangeInfo/Id"),
            Some(id.as_str()),
            "case {:?}: GetChange must return the persisted change; body {}",
            row.case,
            got.body
        );
        assert_eq!(
            got.first("ChangeInfo/Status"),
            Some("INSYNC"),
            "case {:?}: the change must have reached INSYNC; body {}",
            row.case,
            got.body
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a zone keeps at least one association
// ---------------------------------------------------------------------------

/// One row of the last-association table: the extra VPCs associated with the
/// zone before it is narrowed down to the single `survivor`.
struct LastRow {
    case: &'static str,
    survivor: &'static str,
    extra: &'static [(&'static str, &'static str)],
}

#[tokio::test]
async fn the_last_association_of_a_zone_cannot_be_removed() {
    // The rows differ along how the zone came to hold exactly ONE association:
    // created with it, associated and then narrowed back down to one, and
    // narrowed down from several. The refusal is about the COUNT, so the same
    // request is refused whichever VPC the single remaining one is.
    let rows = [
        LastRow {
            case: "the VPC the zone was created with",
            survivor: CREATION_VPC,
            extra: &[],
        },
        LastRow {
            case: "a VPC associated after creation, narrowed back to one",
            survivor: "vpc-0123456789abcdef0",
            extra: &[("vpc-0123456789abcdef0", "eu-west-2")],
        },
        LastRow {
            case: "the survivor of a set of three",
            survivor: "vpc-1111222233334444a",
            extra: &[
                ("vpc-1111222233334444a", "eu-central-1"),
                ("vpc-2222333344445555b", "sa-east-1"),
            ],
        },
    ];

    let router = router();
    for (index, row) in rows.iter().enumerate() {
        let (case, survivor) = (row.case, row.survivor);
        let zone_id = create_private_zone(
            &router,
            &format!("last-vpc-{index}.example.com"),
            &format!("last-vpc-{index}"),
            CREATION_VPC,
        )
        .await;
        for (vpc_id, region) in row.extra {
            associate_ok(&router, &zone_id, vpc_id, region).await;
        }
        // Narrow the zone down to the single surviving association.
        let read = get_zone(&router, &zone_id).await;
        for (vpc_id, region) in associations(&read) {
            if vpc_id == *survivor {
                continue;
            }
            let reply = disassociate(
                &router,
                &zone_id,
                &vpc_block(Some(&vpc_id), Some(&region)),
                None,
            )
            .await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: narrowing the set by {vpc_id}: expected 200, got {} with body {}",
                reply.status, reply.body
            );
        }

        let read = get_zone(&router, &zone_id).await;
        let before = associations(&read);
        assert_eq!(
            before.len(),
            1,
            "case {case:?}: the zone must hold exactly one association here, got {before:?}"
        );
        let (only_id, only_region) = before[0].clone();

        // The request that would empty the list.
        let reply = disassociate(
            &router,
            &zone_id,
            &vpc_block(Some(&only_id), Some(&only_region)),
            Some("removing the only association"),
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
        let read = get_zone(&router, &zone_id).await;
        let after = associations(&read);
        assert_eq!(
            after, before,
            "case {case:?}: the refusal must leave the association intact, expected \
             {before:?}, got {after:?}; body {}",
            read.body
        );

        // The identical request succeeds once a second VPC is associated.
        let second = format!("vpc-{index:017x}");
        associate_ok(&router, &zone_id, &second, "us-west-2").await;
        let reply = disassociate(
            &router,
            &zone_id,
            &vpc_block(Some(&only_id), Some(&only_region)),
            None,
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the same request must succeed with a second VPC associated, \
             got {} with body {}",
            reply.status, reply.body
        );
        let _ = change_id(case, &reply);
        let read = get_zone(&router, &zone_id).await;
        let got = associations(&read);
        assert_eq!(
            got,
            vec![(second.clone(), "us-west-2".to_string())],
            "case {case:?}: only the second VPC must remain, got {got:?}; body {}",
            read.body
        );

        // And the zone is back to one association, so it is refused again.
        let reply = disassociate(
            &router,
            &zone_id,
            &vpc_block(Some(&second), Some("us-west-2")),
            None,
        )
        .await;
        assert_eq!(
            reply.error_code(),
            "LastVPCAssociation",
            "case {case:?}: the new last association must be refused too, got body {}",
            reply.body
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a VPC the zone does not hold is not associated
// ---------------------------------------------------------------------------

#[tokio::test]
async fn disassociating_a_vpc_the_zone_does_not_hold_is_a_not_found() {
    let router = router();
    let zone_id = create_private_zone(
        &router,
        "not-associated.example.com",
        "disassociate-missing",
        CREATION_VPC,
    )
    .await;
    // A second association, so the zone is never refused for holding only one.
    let spare = "vpc-00ff00ff00ff00ff0";
    associate_ok(&router, &zone_id, spare, "eu-west-1").await;
    // One that was associated and then removed, so its id is known to the zone's
    // history but not to its set.
    let removed = "vpc-0abc0abc0abc0abc0";
    associate_ok(&router, &zone_id, removed, "ap-northeast-1").await;
    let gone = disassociate(
        &router,
        &zone_id,
        &vpc_block(Some(removed), Some("ap-northeast-1")),
        None,
    )
    .await;
    assert_eq!(
        gone.status, 200,
        "removing the association first: expected 200, got {} with body {}",
        gone.status, gone.body
    );
    // A VPC associated with ANOTHER zone, which is no association of this one.
    let elsewhere = "vpc-0c0ffee0c0ffee0c0";
    let other_zone =
        create_private_zone(&router, "elsewhere.example.com", "other-zone", elsewhere).await;

    let before = associations(&get_zone(&router, &zone_id).await);
    let rows = [
        (
            "a VPC never associated with the zone",
            "vpc-aabbccdd",
            HOME_REGION,
        ),
        (
            "a VPC whose association was already removed",
            removed,
            "ap-northeast-1",
        ),
        ("a VPC associated with another zone", elsewhere, HOME_REGION),
        (
            "a VPC the zone never held, named in another region",
            "vpc-11223344",
            "ap-south-1",
        ),
    ];
    for (case, vpc_id, region) in rows {
        let reply = disassociate(
            &router,
            &zone_id,
            &vpc_block(Some(vpc_id), Some(region)),
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
            "VPCAssociationNotFound",
            "case {case:?}: expected VPCAssociationNotFound, got body {}",
            reply.body
        );
        let read = get_zone(&router, &zone_id).await;
        let got = associations(&read);
        assert_eq!(
            got, before,
            "case {case:?}: a refused request must leave the set at {before:?}, got \
             {got:?}; body {}",
            read.body
        );
    }

    // The other zone kept its own association throughout.
    let read = get_zone(&router, &other_zone).await;
    let got = associations(&read);
    assert!(
        got.iter().any(|(id, _)| id == elsewhere),
        "the other zone must keep its association, got {got:?}; body {}",
        read.body
    );
}

#[tokio::test]
async fn a_public_zone_holds_no_association_to_remove() {
    // A public hosted zone holds no VPC at all, so every VPC named against one
    // is an association that is not there. The rows differ along how the zone
    // came to be public.
    let rows = [
        ("no HostedZoneConfig at all", "", "public-none.example.com"),
        (
            "an empty HostedZoneConfig",
            "<HostedZoneConfig></HostedZoneConfig>",
            "public-empty.example.com",
        ),
        (
            "PrivateZone sent explicitly false",
            "<HostedZoneConfig><PrivateZone>false</PrivateZone></HostedZoneConfig>",
            "public-explicit.example.com",
        ),
    ];

    let router = router();
    for (index, (case, config, name)) in rows.iter().enumerate() {
        let created = create_zone(
            &router,
            name,
            &format!("public-disassociate-{index}"),
            config,
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201 from CreateHostedZone, got {} with body {}",
            created.status, created.body
        );
        let zone_id = bare_zone_id(&created);

        let reply = disassociate(
            &router,
            &zone_id,
            &vpc_block(Some("vpc-1a2b3c4d"), Some(HOME_REGION)),
            None,
        )
        .await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected 404 for a public zone, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "VPCAssociationNotFound",
            "case {case:?}: expected VPCAssociationNotFound, got body {}",
            reply.body
        );
        let read = get_zone(&router, &zone_id).await;
        assert_eq!(
            read.first("Config/PrivateZone"),
            Some("false"),
            "case {case:?}: the zone must still be public; body {}",
            read.body
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the request has to name a VPC
// ---------------------------------------------------------------------------

/// One row of the malformed-VPC table: the `<VPC>` block sent, and the wire
/// error code it must be refused with.
struct BadVpcRow {
    case: &'static str,
    vpc_id: Option<&'static str>,
    region: Option<&'static str>,
    code: &'static str,
}

#[tokio::test]
async fn a_request_that_names_no_valid_vpc_is_refused() {
    // A VPC id is `vpc-` plus 8 or 17 lowercase hexadecimal characters, so the
    // rows walk both bounds and one past each, the case rule, the prefix and the
    // absent member. The region rows walk the modelled `VPCRegion` enum's
    // boundary: absent, empty, a value outside it, and one differing only in
    // case.
    let rows = [
        BadVpcRow {
            case: "no VPCId member at all",
            vpc_id: None,
            region: Some(HOME_REGION),
            code: "InvalidVPCId",
        },
        BadVpcRow {
            case: "an empty VPCId",
            vpc_id: Some(""),
            region: Some(HOME_REGION),
            code: "InvalidVPCId",
        },
        BadVpcRow {
            case: "an id with no vpc- prefix",
            vpc_id: Some("9f8e7d6c"),
            region: Some(HOME_REGION),
            code: "InvalidVPCId",
        },
        BadVpcRow {
            case: "another resource kind's prefix",
            vpc_id: Some("subnet-9f8e7d6c"),
            region: Some(HOME_REGION),
            code: "InvalidVPCId",
        },
        BadVpcRow {
            case: "one character short of the 8-character form",
            vpc_id: Some("vpc-9f8e7d6"),
            region: Some(HOME_REGION),
            code: "InvalidVPCId",
        },
        BadVpcRow {
            case: "one character past the 8-character form",
            vpc_id: Some("vpc-9f8e7d6c0"),
            region: Some(HOME_REGION),
            code: "InvalidVPCId",
        },
        BadVpcRow {
            case: "one character short of the 17-character form",
            vpc_id: Some("vpc-0a1b2c3d4e5f6071"),
            region: Some(HOME_REGION),
            code: "InvalidVPCId",
        },
        BadVpcRow {
            case: "one character past the 17-character form",
            vpc_id: Some("vpc-0a1b2c3d4e5f607182"),
            region: Some(HOME_REGION),
            code: "InvalidVPCId",
        },
        BadVpcRow {
            case: "a non-hexadecimal character in the id",
            vpc_id: Some("vpc-9f8e7d6z"),
            region: Some(HOME_REGION),
            code: "InvalidVPCId",
        },
        BadVpcRow {
            case: "an uppercase id body",
            vpc_id: Some("vpc-9F8E7D6C"),
            region: Some(HOME_REGION),
            code: "InvalidVPCId",
        },
        BadVpcRow {
            case: "no VPCRegion member at all",
            vpc_id: Some(CREATION_VPC),
            region: None,
            code: "InvalidInput",
        },
        // `VPC$VPCRegion` targets the modelled `VPCRegion` enum, so a value
        // outside it is refused by the request boundary (which enforces every
        // modelled constraint) before this handler runs — and Route 53 states
        // that refusal under the one input error its model declares,
        // `InvalidInput`, HTTP 400.
        BadVpcRow {
            case: "an empty VPCRegion",
            vpc_id: Some(CREATION_VPC),
            region: Some(""),
            code: "InvalidInput",
        },
        BadVpcRow {
            case: "a region the VPCRegion enum does not name",
            vpc_id: Some(CREATION_VPC),
            region: Some("mars-west-1"),
            code: "InvalidInput",
        },
        BadVpcRow {
            case: "a modelled region in the wrong case",
            vpc_id: Some(CREATION_VPC),
            region: Some("US-EAST-1"),
            code: "InvalidInput",
        },
    ];

    let router = router();
    let zone_id = create_private_zone(
        &router,
        "bad-vpc-disassociate.example.com",
        "disassociate-bad-vpc",
        CREATION_VPC,
    )
    .await;
    associate_ok(&router, &zone_id, "vpc-0a1b2c3d4e5f60718", "eu-north-1").await;
    let untouched = associations(&get_zone(&router, &zone_id).await);

    for row in &rows {
        let reply = disassociate(&router, &zone_id, &vpc_block(row.vpc_id, row.region), None).await;
        assert_eq!(
            reply.status, 400,
            "case {:?}: expected 400, got {} with body {}",
            row.case, reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            row.code,
            "case {:?}: expected {}, got body {}",
            row.case,
            row.code,
            reply.body
        );
        let read = get_zone(&router, &zone_id).await;
        let got = associations(&read);
        assert_eq!(
            got, untouched,
            "case {:?}: a refused request must leave the zone's associations at \
             {untouched:?}, got {got:?}; body {}",
            row.case, read.body
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: the zone has to exist
// ---------------------------------------------------------------------------

#[tokio::test]
async fn disassociating_from_an_unknown_zone_is_a_not_found() {
    // The rows differ along the id shape a caller might pass: the zone-id form,
    // an id of another shape entirely, and the id of a zone that was deleted.
    let router = router();
    let doomed = create_private_zone(
        &router,
        "deleted-disassociate.example.com",
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

// ---------------------------------------------------------------------------
// Rule 6: association and disassociation are inverses
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_disassociated_vpc_can_be_associated_again() {
    // The rows differ along the region the VPC is associated in the second
    // time: the same one, and another of the modelled `VPCRegion` enum — the
    // association is keyed by id, so the region is what the read reports back.
    let rows = [
        ("re-associated in the same region", "eu-west-3"),
        ("re-associated in another region", "ap-east-1"),
    ];

    let router = router();
    let zone_id = create_private_zone(
        &router,
        "inverse.example.com",
        "disassociate-inverse",
        CREATION_VPC,
    )
    .await;
    let subject = "vpc-0f1e2d3c4b5a69788";

    for (case, second_region) in rows {
        associate_ok(&router, &zone_id, subject, "eu-west-3").await;
        let with_subject = associations(&get_zone(&router, &zone_id).await);
        assert!(
            with_subject.contains(&(subject.to_string(), "eu-west-3".to_string())),
            "case {case:?}: the subject must be associated first, got {with_subject:?}"
        );

        let reply = disassociate(
            &router,
            &zone_id,
            &vpc_block(Some(subject), Some("eu-west-3")),
            None,
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} with body {}",
            reply.status, reply.body
        );
        let without = associations(&get_zone(&router, &zone_id).await);
        assert_eq!(
            without,
            vec![(CREATION_VPC.to_string(), HOME_REGION.to_string())],
            "case {case:?}: the set must be back to the creation association, got {without:?}"
        );

        associate_ok(&router, &zone_id, subject, second_region).await;
        let again = associations(&get_zone(&router, &zone_id).await);
        assert_eq!(
            again,
            vec![
                (CREATION_VPC.to_string(), HOME_REGION.to_string()),
                (subject.to_string(), second_region.to_string()),
            ],
            "case {case:?}: a freed place must take the VPC again, got {again:?}"
        );

        // Leave the zone as the next row expects it: only the creation VPC.
        let reply = disassociate(
            &router,
            &zone_id,
            &vpc_block(Some(subject), Some(second_region)),
            None,
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200 clearing the subject again, got {} with body {}",
            reply.status, reply.body
        );
    }
}
