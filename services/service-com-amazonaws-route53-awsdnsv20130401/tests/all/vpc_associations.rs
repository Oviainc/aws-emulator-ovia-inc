// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `AssociateVPCWithHostedZone`.
//!
//! The rules under test, each asserted over a SET of inputs rather than one
//! recorded request:
//!
//! 1. A private hosted zone's VPC associations are a SET keyed by VPC id:
//!    associating a VPC appends it (with the region it was created in) to the
//!    list `GetHostedZone` reports, associating one the zone already carries
//!    changes nothing, and every accepted request answers with its own
//!    `ChangeInfo` that `GetChange` can read back. The table spans the two VPC
//!    id forms, several regions of the modelled `VPCRegion` enum (including
//!    other partitions) and a growing association count.
//! 2. A VPC belongs to a PRIVATE hosted zone only, so the same request against
//!    a public zone is the modelled `PublicZoneVPCAssociation` (HTTP 400) and
//!    persists nothing.
//! 3. A request has to name a VPC: an id outside the `vpc-<hex>` form is
//!    `InvalidVPCId` and a missing region is `InvalidInput`, both HTTP 400, and
//!    neither touches the zone. (A region value outside the modelled
//!    `VPCRegion` enum never reaches the handler: the request boundary refuses
//!    every modelled constraint first, with its own 400.)
//! 4. An id that names no zone is `NoSuchHostedZone` (HTTP 404).
//! 5. A VPC cannot resolve one DNS name through two zones, so associating a VPC
//!    with a zone whose name another zone already serves for that VPC is
//!    `ConflictingDomainExists` (HTTP 400) — while a zone with a DIFFERENT name
//!    takes the same VPC.
//!
//! Every test drives the service through the real router, using the SAME
//! harness `hosted_zone_records` uses, so what is asserted is the HTTP/restXml
//! behaviour a Route 53 client sees.

use std::time::Duration;

use smithy_server_runtime::MultiServiceRouter;

use crate::hosted_zone_records::{call, create_zone_body, escape_xml, router, Reply};

/// The region the zones in this suite are first associated with, which is also
/// a value of the modelled `VPCRegion` enum
/// (smithy_query(query="shape",shape="VPCRegion")).
pub(crate) const HOME_REGION: &str = "us-east-1";

/// The VPC every private zone in this suite is created with.
pub(crate) const CREATION_VPC: &str = "vpc-9f8e7d6c";

/// A `<VPC>` block, with each member present only when it is given — so a row
/// can leave `VPCId` or `VPCRegion` out entirely.
pub(crate) fn vpc_block(vpc_id: Option<&str>, region: Option<&str>) -> String {
    let id = vpc_id.map_or_else(String::new, |id| {
        format!("<VPCId>{}</VPCId>", escape_xml(id))
    });
    let region = region.map_or_else(String::new, |region| {
        format!("<VPCRegion>{}</VPCRegion>", escape_xml(region))
    });
    format!("<VPC>{region}{id}</VPC>")
}

/// An `AssociateVPCWithHostedZone` body carrying `vpc` (a [`vpc_block`]) and,
/// when given, a `Comment`.
fn associate_body(vpc: &str, comment: Option<&str>) -> String {
    let comment = comment.map_or_else(String::new, |text| {
        format!("<Comment>{}</Comment>", escape_xml(text))
    });
    format!(
        "<AssociateVPCWithHostedZoneRequest \
         xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">{vpc}{comment}\
         </AssociateVPCWithHostedZoneRequest>"
    )
}

/// Send one `AssociateVPCWithHostedZone` request.
pub(crate) async fn associate(
    router: &MultiServiceRouter,
    zone_id: &str,
    vpc: &str,
    comment: Option<&str>,
) -> Reply {
    call(
        router,
        "POST",
        &format!("/2013-04-01/hostedzone/{zone_id}/associatevpc"),
        &associate_body(vpc, comment),
    )
    .await
}

/// Read a zone back.
pub(crate) async fn get_zone(router: &MultiServiceRouter, zone_id: &str) -> Reply {
    call(router, "GET", &format!("/2013-04-01/hostedzone/{zone_id}"), "").await
}

/// The bare (`Z…`) id from a reply carrying a `HostedZone`.
pub(crate) fn bare_zone_id(reply: &Reply) -> String {
    let id = reply
        .first("HostedZone/Id")
        .unwrap_or_else(|| panic!("the reply must carry HostedZone/Id; body {}", reply.body));
    id.trim_start_matches("/hostedzone/").to_string()
}

/// Create a zone and return the reply. `config` is the `HostedZoneConfig` (and,
/// for a private zone, `VPC`) block.
pub(crate) async fn create_zone(
    router: &MultiServiceRouter,
    name: &str,
    caller_reference: &str,
    config: &str,
) -> Reply {
    call(
        router,
        "POST",
        "/2013-04-01/hostedzone",
        &create_zone_body(name, caller_reference, config),
    )
    .await
}

/// Create a PRIVATE zone associated with `vpc_id` in [`HOME_REGION`], and
/// return its bare id.
pub(crate) async fn create_private_zone(
    router: &MultiServiceRouter,
    name: &str,
    caller_reference: &str,
    vpc_id: &str,
) -> String {
    let config = format!(
        "<HostedZoneConfig><PrivateZone>true</PrivateZone></HostedZoneConfig>{}",
        vpc_block(Some(vpc_id), Some(HOME_REGION))
    );
    let reply = create_zone(router, name, caller_reference, &config).await;
    assert_eq!(
        reply.status, 201,
        "creating private zone {name}: expected 201, got {} with body {}",
        reply.status, reply.body
    );
    bare_zone_id(&reply)
}

/// The `(VPCId, VPCRegion)` pairs a `GetHostedZone` reply reports, in document
/// order — which is the order the zone holds its associations in.
pub(crate) fn associations(reply: &Reply) -> Vec<(String, String)> {
    let ids = reply.all("VPCId");
    let regions = reply.all("VPCRegion");
    assert_eq!(
        ids.len(),
        regions.len(),
        "every reported VPC must carry both its id and its region; body {}",
        reply.body
    );
    ids.into_iter()
        .map(str::to_string)
        .zip(regions.into_iter().map(str::to_string))
        .collect()
}

/// Whether `id` is a `/change/C…` id, as every `ChangeInfo` reports one.
fn is_change_id(id: &str) -> bool {
    id.strip_prefix("/change/")
        .is_some_and(|bare| bare.starts_with('C') && bare.len() == 15)
}

// ---------------------------------------------------------------------------
// Rule 1: an association is appended to the zone's set and read back
// ---------------------------------------------------------------------------

/// One row of the association table: a VPC to associate, named by its id form
/// and the region it was created in.
struct VpcRow {
    case: &'static str,
    vpc_id: &'static str,
    region: &'static str,
}

#[tokio::test]
async fn associating_vpcs_appends_them_to_the_zone_the_read_reports() {
    // Each row differs along the id form (the original 8-character form and the
    // 17-character long form) and along the region, drawn from the modelled
    // `VPCRegion` enum (smithy_query(query="shape",shape="VPCRegion")) —
    // including regions in other partitions, which the enum names too.
    let rows = [
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
            case: "a region added late to the enum",
            vpc_id: "vpc-00112233445566778",
            region: "il-central-1",
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
    ];

    let router = router();
    let zone_id = create_private_zone(
        &router,
        "multi-vpc.example.com",
        "associate-table",
        CREATION_VPC,
    )
    .await;

    // The zone starts with the association `CreateHostedZone` made.
    let mut expected = vec![(CREATION_VPC.to_string(), HOME_REGION.to_string())];
    let read = get_zone(&router, &zone_id).await;
    assert_eq!(
        associations(&read),
        expected,
        "a private zone starts with the association it was created with; body {}",
        read.body
    );

    let mut change_ids = Vec::new();
    for row in &rows {
        let comment = format!("associating {}", row.case);
        let reply = associate(
            &router,
            &zone_id,
            &vpc_block(Some(row.vpc_id), Some(row.region)),
            Some(&comment),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200 from AssociateVPCWithHostedZone, got {} with body {}",
            row.case, reply.status, reply.body
        );

        let change_id = reply
            .first("ChangeInfo/Id")
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the reply must carry ChangeInfo/Id; body {}",
                    row.case, reply.body
                )
            })
            .to_string();
        assert!(
            is_change_id(&change_id),
            "case {:?}: expected a /change/C… id, got {change_id:?}",
            row.case
        );
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
        change_ids.push(change_id);

        expected.push((row.vpc_id.to_string(), row.region.to_string()));
        let read = get_zone(&router, &zone_id).await;
        let got = associations(&read);
        assert_eq!(
            got, expected,
            "case {:?}: GetHostedZone must report the associations the zone now holds, \
             expected {expected:?}, got {got:?}; body {}",
            row.case, read.body
        );
    }

    // Each accepted request got its own change, and each change is readable.
    assert_eq!(
        change_ids
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        change_ids.len(),
        "each association must get its own change id, got {change_ids:?}"
    );
    // The clock owes the PENDING -> INSYNC transition; nothing polls for it.
    // A blocking sleep, because this crate's tokio does not enable the `time`
    // feature; each test owns its runtime, so nothing else is waiting on it.
    std::thread::sleep(Duration::from_millis(700));
    for (row, change_id) in rows.iter().zip(change_ids.iter()) {
        let bare = change_id.trim_start_matches("/change/");
        let got = call(&router, "GET", &format!("/2013-04-01/change/{bare}"), "").await;
        assert_eq!(
            got.status, 200,
            "case {:?}: expected 200 from GetChange, got {} with body {}",
            row.case, got.status, got.body
        );
        assert_eq!(
            got.first("ChangeInfo/Id"),
            Some(change_id.as_str()),
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

#[tokio::test]
async fn re_associating_a_vpc_leaves_the_association_set_as_it_was() {
    // The rule: the set is keyed by VPC id, so the same VPC is held once
    // however many times it is associated — and the repeat is accepted, with
    // its own change, rather than refused.
    let router = router();
    let zone_id =
        create_private_zone(&router, "repeat.example.com", "associate-repeat", CREATION_VPC).await;

    // The rows differ along which already-held VPC is associated again, and
    // along whether the request repeats the region it was stored with.
    let extra = "vpc-0123456789abcdef0";
    let first = associate(
        &router,
        &zone_id,
        &vpc_block(Some(extra), Some("eu-west-2")),
        None,
    )
    .await;
    assert_eq!(
        first.status, 200,
        "expected 200 for the first association, got {} with body {}",
        first.status, first.body
    );
    let expected = vec![
        (CREATION_VPC.to_string(), HOME_REGION.to_string()),
        (extra.to_string(), "eu-west-2".to_string()),
    ];

    let repeats = [
        ("the VPC the zone was created with", CREATION_VPC, HOME_REGION),
        ("a VPC associated by this operation", extra, "eu-west-2"),
    ];
    let mut change_ids = Vec::new();
    for (case, vpc_id, region) in repeats {
        for round in 0..2 {
            let reply = associate(
                &router,
                &zone_id,
                &vpc_block(Some(vpc_id), Some(region)),
                None,
            )
            .await;
            assert_eq!(
                reply.status, 200,
                "case {case:?} round {round}: expected 200 for a repeat association, \
                 got {} with body {}",
                reply.status, reply.body
            );
            change_ids.push(
                reply
                    .first("ChangeInfo/Id")
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: no ChangeInfo/Id in {}", reply.body)
                    })
                    .to_string(),
            );

            let read = get_zone(&router, &zone_id).await;
            let got = associations(&read);
            assert_eq!(
                got, expected,
                "case {case:?} round {round}: a repeat must leave the set at {expected:?}, \
                 got {got:?}; body {}",
                read.body
            );
        }
    }
    assert_eq!(
        change_ids
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        change_ids.len(),
        "every accepted request answers with its own change, got {change_ids:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: a public zone takes no VPC
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_public_zone_refuses_a_vpc_association() {
    // The rows differ along HOW the zone came to be public: with no
    // `HostedZoneConfig` at all, with an empty one, and with `PrivateZone`
    // sent explicitly false.
    let rows = [
        ("no HostedZoneConfig at all", "", "no-config.example.com"),
        (
            "an empty HostedZoneConfig",
            "<HostedZoneConfig></HostedZoneConfig>",
            "empty-config.example.com",
        ),
        (
            "PrivateZone sent explicitly false",
            "<HostedZoneConfig><PrivateZone>false</PrivateZone></HostedZoneConfig>",
            "explicit-public.example.com",
        ),
    ];

    let router = router();
    for (index, (case, config, name)) in rows.iter().enumerate() {
        let created = create_zone(&router, name, &format!("public-zone-{index}"), config).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201 from CreateHostedZone, got {} with body {}",
            created.status, created.body
        );
        let zone_id = bare_zone_id(&created);

        let reply = associate(
            &router,
            &zone_id,
            &vpc_block(Some("vpc-1a2b3c4d"), Some(HOME_REGION)),
            None,
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400 for a public zone, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "PublicZoneVPCAssociation",
            "case {case:?}: expected PublicZoneVPCAssociation, got body {}",
            reply.body
        );

        // The refusal persisted nothing: the zone still reports no VPC, and it
        // is still a public zone.
        let read = get_zone(&router, &zone_id).await;
        assert_eq!(
            associations(&read),
            Vec::new(),
            "case {case:?}: a refused association must leave the zone with none; body {}",
            read.body
        );
        assert_eq!(
            read.first("Config/PrivateZone"),
            Some("false"),
            "case {case:?}: the zone must still be public; body {}",
            read.body
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the request has to name a VPC
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
    // rows walk both bounds and one past each, the case rule, the prefix and
    // the absent member. The region rows walk the modelled `VPCRegion` enum's
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
            vpc_id: Some("1a2b3c4d"),
            region: Some(HOME_REGION),
            code: "InvalidVPCId",
        },
        BadVpcRow {
            case: "another resource kind's prefix",
            vpc_id: Some("subnet-1a2b3c4d"),
            region: Some(HOME_REGION),
            code: "InvalidVPCId",
        },
        BadVpcRow {
            case: "one character short of the 8-character form",
            vpc_id: Some("vpc-1a2b3c4"),
            region: Some(HOME_REGION),
            code: "InvalidVPCId",
        },
        BadVpcRow {
            case: "one character past the 8-character form",
            vpc_id: Some("vpc-1a2b3c4d5"),
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
            vpc_id: Some("vpc-1a2b3c4z"),
            region: Some(HOME_REGION),
            code: "InvalidVPCId",
        },
        BadVpcRow {
            case: "an uppercase id body",
            vpc_id: Some("vpc-1A2B3C4D"),
            region: Some(HOME_REGION),
            code: "InvalidVPCId",
        },
        BadVpcRow {
            case: "no VPCRegion member at all",
            vpc_id: Some("vpc-1a2b3c4d"),
            region: None,
            code: "InvalidInput",
        },
        // `VPC$VPCRegion` targets the modelled `VPCRegion` enum, so a value
        // outside it is refused by the request boundary (which enforces every
        // modelled constraint) before any handler runs: the refusal is the
        // boundary's own `ValidationException`, still HTTP 400.
        BadVpcRow {
            case: "an empty VPCRegion",
            vpc_id: Some("vpc-1a2b3c4d"),
            region: Some(""),
            code: "ValidationException",
        },
        BadVpcRow {
            case: "a region the VPCRegion enum does not name",
            vpc_id: Some("vpc-1a2b3c4d"),
            region: Some("mars-west-1"),
            code: "ValidationException",
        },
        BadVpcRow {
            case: "a modelled region in the wrong case",
            vpc_id: Some("vpc-1a2b3c4d"),
            region: Some("US-EAST-1"),
            code: "ValidationException",
        },
    ];

    let router = router();
    let zone_id = create_private_zone(
        &router,
        "bad-vpc.example.com",
        "associate-bad-vpc",
        CREATION_VPC,
    )
    .await;
    let untouched = vec![(CREATION_VPC.to_string(), HOME_REGION.to_string())];

    for row in &rows {
        let reply = associate(
            &router,
            &zone_id,
            &vpc_block(row.vpc_id, row.region),
            None,
        )
        .await;
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
// Rule 4: the zone has to exist
// ---------------------------------------------------------------------------

#[tokio::test]
async fn associating_a_vpc_with_an_unknown_zone_is_a_not_found() {
    // The rows differ along the id shape a caller might pass: the zone-id form,
    // an id of another shape entirely, and the id of a zone that was deleted.
    let router = router();
    let doomed = create_private_zone(
        &router,
        "deleted.example.com",
        "associate-deleted",
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
        ("a well-shaped id no zone carries", "ZNOSUCHZONE0001".to_string()),
        ("an id of another shape", "not-a-zone-id".to_string()),
        ("the id of a deleted zone", doomed),
    ] {
        let reply = associate(
            &router,
            &zone_id,
            &vpc_block(Some("vpc-1a2b3c4d"), Some(HOME_REGION)),
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
// Rule 5: one VPC cannot resolve one name through two zones
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_vpc_cannot_be_associated_with_two_zones_of_the_same_name() {
    let router = router();
    let shared_vpc = "vpc-0abcdef012345678a";
    // The zone that already serves `shared.example.com` for the VPC.
    let first = create_private_zone(&router, "shared.example.com", "conflict-first", shared_vpc)
        .await;
    // The rows differ along how the OTHER zone's name relates to the first's:
    // identical, identical but for the trailing dot, identical but for case —
    // all the SAME DNS name — and then a different name, which is accepted.
    let rows = [
        ("the same name", "shared.example.com", 400),
        ("the same name with a trailing dot", "shared.example.com.", 400),
        ("the same name in another case", "SHARED.example.com", 400),
        ("a different name", "other.example.com", 200),
    ];

    for (index, (case, name, expected_status)) in rows.iter().enumerate() {
        let other = create_private_zone(
            &router,
            name,
            &format!("conflict-other-{index}"),
            "vpc-1111222233334444a",
        )
        .await;
        let reply = associate(
            &router,
            &other,
            &vpc_block(Some(shared_vpc), Some(HOME_REGION)),
            None,
        )
        .await;
        assert_eq!(
            reply.status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} with body {}",
            reply.status, reply.body
        );
        if *expected_status == 400 {
            assert_eq!(
                reply.error_code(),
                "ConflictingDomainExists",
                "case {case:?}: expected ConflictingDomainExists, got body {}",
                reply.body
            );
            let read = get_zone(&router, &other).await;
            let got = associations(&read);
            assert!(
                got.iter().all(|(id, _)| id != shared_vpc),
                "case {case:?}: the refused VPC must not be associated, got {got:?}; body {}",
                read.body
            );
        } else {
            let read = get_zone(&router, &other).await;
            let got = associations(&read);
            assert!(
                got.iter().any(|(id, _)| id == shared_vpc),
                "case {case:?}: a zone with another name must take the VPC, got {got:?}; \
                 body {}",
                read.body
            );
        }
    }

    // The zone that held the VPC first still holds it, whatever the others did.
    let read = get_zone(&router, &first).await;
    let got = associations(&read);
    assert!(
        got.iter().any(|(id, _)| id == shared_vpc),
        "the first zone must keep its association, got {got:?}; body {}",
        read.body
    );
}
