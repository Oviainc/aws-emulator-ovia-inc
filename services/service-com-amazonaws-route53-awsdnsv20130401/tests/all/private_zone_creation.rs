// SPDX-License-Identifier: CC0-1.0

//! `CreateHostedZone`: which requests create a PRIVATE hosted zone.
//!
//! The rules under test, each asserted over a SET of inputs rather than one
//! recorded request:
//!
//! 1. Naming a `VPC` is itself what makes the new zone private. `VPC` is
//!    "(Private hosted zones only) ... the Amazon VPC that you're associating
//!    with this hosted zone", and the only further requirement the model states
//!    when it is given is "the paramaters VPCId and VPCRegion are also
//!    required" — so `HostedZoneConfig` need not be sent at all, and because
//!    `HostedZoneConfig$PrivateZone` is a boolean with a modelled default of
//!    `false`, a request that omits the flag is indistinguishable from one that
//!    sends it `false`: the flag can only ADD to what the VPC already says. The
//!    table therefore differs along HOW the zone is asked for (no config, an
//!    empty config, a comment only, the flag true, the flag false) and along
//!    the VPC named (both id forms, regions from several partitions).
//! 2. A private zone is created WITH its first association, so a private zone
//!    asked for with no complete VPC is `InvalidInput` (HTTP 400) and a VPC id
//!    outside the `vpc-<8|17 lowercase hex>` form is `InvalidVPCId` (HTTP 400),
//!    and neither persists a zone.
//! 3. The zone a VPC-only create makes is a real private zone: it takes further
//!    `AssociateVPCWithHostedZone` requests (which a public zone refuses), and
//!    every association is in what `GetHostedZone` reports.
//!
//! Every test drives the service through the real router, using the SAME
//! harness `hosted_zone_records` and `vpc_associations` use, so what is
//! asserted is the HTTP/restXml behaviour a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use crate::hosted_zone_records::{call, router};
use crate::vpc_associations::{
    associate, associations, bare_zone_id, create_zone, get_zone, vpc_block, HOME_REGION,
};

/// The hosted zones a listing reports, as bare (`Z…`) ids in document order.
async fn listed_zone_ids(router: &MultiServiceRouter, query: &str) -> Vec<String> {
    let reply = call(router, "GET", &format!("/2013-04-01/hostedzone{query}"), "").await;
    assert_eq!(
        reply.status, 200,
        "ListHostedZones{query} must answer 200, got {} with body {}",
        reply.status, reply.body
    );
    reply
        .all("HostedZone/Id")
        .into_iter()
        .map(|id| id.trim_start_matches("/hostedzone/").to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// Rule 1: a named VPC is what makes the zone private
// ---------------------------------------------------------------------------

/// One row of the private-create table: how the zone is asked for, and the VPC
/// it names.
struct PrivateRow {
    case: &'static str,
    name: &'static str,
    /// The `HostedZoneConfig` block sent, which may be absent entirely.
    config: &'static str,
    vpc_id: &'static str,
    /// A value of the modelled `VPCRegion` enum
    /// (smithy_query(query="shape",shape="VPCRegion")).
    region: &'static str,
    comment: Option<&'static str>,
}

#[tokio::test]
async fn naming_a_vpc_is_what_makes_a_new_hosted_zone_private() {
    let rows = [
        PrivateRow {
            case: "no HostedZoneConfig at all, with the 17-character id form",
            name: "vpc-only.example.com",
            config: "",
            vpc_id: "vpc-0a1b2c3d4e5f60718",
            region: HOME_REGION,
            comment: None,
        },
        PrivateRow {
            case: "an empty HostedZoneConfig, with the 8-character id form",
            name: "empty-config.example.net",
            config: "<HostedZoneConfig></HostedZoneConfig>",
            vpc_id: "vpc-1a2b3c4d",
            region: "eu-west-1",
            comment: None,
        },
        PrivateRow {
            case: "a HostedZoneConfig carrying only a comment",
            name: "commented.example.org",
            config: "<HostedZoneConfig><Comment>internal only</Comment></HostedZoneConfig>",
            vpc_id: "vpc-9f8e7d6c",
            region: "ap-southeast-2",
            comment: Some("internal only"),
        },
        PrivateRow {
            case: "PrivateZone sent explicitly true",
            name: "explicit.example.co.uk",
            config: "<HostedZoneConfig><PrivateZone>true</PrivateZone></HostedZoneConfig>",
            vpc_id: "vpc-00112233445566778",
            region: "il-central-1",
            comment: None,
        },
        PrivateRow {
            case: "PrivateZone true alongside a comment, in another partition",
            name: "explicit-commented.example.com",
            config: "<HostedZoneConfig><Comment>private and commented</Comment>\
                     <PrivateZone>true</PrivateZone></HostedZoneConfig>",
            vpc_id: "vpc-deadbeef",
            region: "cn-north-1",
            comment: Some("private and commented"),
        },
        PrivateRow {
            // The flag's modelled default IS false, so this request reaches the
            // service exactly as one that omits the flag does: the VPC decides.
            case: "PrivateZone sent explicitly false alongside a VPC",
            name: "flag-false.example.com",
            config: "<HostedZoneConfig><PrivateZone>false</PrivateZone></HostedZoneConfig>",
            vpc_id: "vpc-0fedcba9876543210",
            region: "us-gov-west-1",
            comment: None,
        },
    ];

    let router = router();
    let mut private_ids = Vec::new();

    for (index, row) in rows.iter().enumerate() {
        let body = format!(
            "{}{}",
            row.config,
            vpc_block(Some(row.vpc_id), Some(row.region))
        );
        let created = create_zone(&router, row.name, &format!("private-{index}"), &body).await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from CreateHostedZone, got {} with body {}",
            row.case, created.status, created.body
        );
        assert_eq!(
            created.first("Config/PrivateZone"),
            Some("true"),
            "case {:?}: the created zone must be private, got {:?}; body {}",
            row.case,
            created.first("Config/PrivateZone"),
            created.body
        );
        assert_eq!(
            created.first("Config/Comment"),
            row.comment,
            "case {:?}: expected comment {:?}, got {:?}; body {}",
            row.case,
            row.comment,
            created.first("Config/Comment"),
            created.body
        );
        // The create echoes the VPC it associated.
        assert_eq!(
            associations(&created),
            vec![(row.vpc_id.to_string(), row.region.to_string())],
            "case {:?}: CreateHostedZone must echo the VPC it associated, got {:?}; body {}",
            row.case,
            associations(&created),
            created.body
        );

        // Round trip: the zone reads back private, with that association.
        let zone_id = bare_zone_id(&created);
        let read = get_zone(&router, &zone_id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected 200 from GetHostedZone, got {} with body {}",
            row.case, read.status, read.body
        );
        assert_eq!(
            read.first("Config/PrivateZone"),
            Some("true"),
            "case {:?}: GetHostedZone must report the zone private, got {:?}; body {}",
            row.case,
            read.first("Config/PrivateZone"),
            read.body
        );
        assert_eq!(
            associations(&read),
            vec![(row.vpc_id.to_string(), row.region.to_string())],
            "case {:?}: GetHostedZone must report the creation association, got {:?}; body {}",
            row.case,
            associations(&read),
            read.body
        );
        private_ids.push(zone_id);
    }

    // The control row: no VPC at all is a PUBLIC zone, so it is the one zone the
    // private listing leaves out.
    let public = create_zone(&router, "public.example.com", "public-control", "").await;
    assert_eq!(
        public.status, 201,
        "case a zone created with no VPC: expected 201, got {} with body {}",
        public.status, public.body
    );
    assert_eq!(
        public.first("Config/PrivateZone"),
        Some("false"),
        "case a zone created with no VPC: expected a public zone, got body {}",
        public.body
    );
    let public_id = bare_zone_id(&public);

    let listed = listed_zone_ids(&router, "?hostedzonetype=PrivateHostedZone").await;
    for zone_id in &private_ids {
        assert!(
            listed.contains(zone_id),
            "zone {zone_id} must be listed as a private hosted zone, got {listed:?}"
        );
    }
    assert!(
        !listed.contains(&public_id),
        "the public zone {public_id} must not be listed as private, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: a private zone needs ONE complete, well-formed VPC
// ---------------------------------------------------------------------------

/// One row of the refusal table: the `HostedZoneConfig` and `VPC` blocks sent,
/// and the wire error code the request must be refused with.
struct RefusedRow {
    case: &'static str,
    config: &'static str,
    vpc: String,
    code: &'static str,
}

#[tokio::test]
async fn a_private_zone_without_one_usable_vpc_is_refused() {
    // A VPC id is `vpc-` plus 8 or 17 lowercase hexadecimal characters (the two
    // accepted forms are rows of the table above), so these rows walk one past
    // each bound, the case rule, a non-hex character and the missing prefix.
    let rows = [
        RefusedRow {
            case: "PrivateZone true with no VPC at all",
            config: "<HostedZoneConfig><PrivateZone>true</PrivateZone></HostedZoneConfig>",
            vpc: String::new(),
            code: "InvalidInput",
        },
        RefusedRow {
            case: "PrivateZone true with a VPC naming neither member",
            config: "<HostedZoneConfig><PrivateZone>true</PrivateZone></HostedZoneConfig>",
            vpc: vpc_block(None, None),
            code: "InvalidInput",
        },
        RefusedRow {
            case: "a VPC naming only VPCId",
            config: "",
            vpc: vpc_block(Some("vpc-1a2b3c4d"), None),
            code: "InvalidInput",
        },
        RefusedRow {
            case: "a VPC naming only VPCRegion",
            config: "",
            vpc: vpc_block(None, Some(HOME_REGION)),
            code: "InvalidInput",
        },
        RefusedRow {
            case: "a VPC whose VPCId is empty",
            config: "",
            vpc: vpc_block(Some(""), Some(HOME_REGION)),
            code: "InvalidInput",
        },
        RefusedRow {
            case: "a VPCId one character short of the 8-character form",
            config: "",
            vpc: vpc_block(Some("vpc-1a2b3c4"), Some(HOME_REGION)),
            code: "InvalidVPCId",
        },
        RefusedRow {
            case: "a VPCId one character past the 8-character form",
            config: "",
            vpc: vpc_block(Some("vpc-1a2b3c4d5"), Some(HOME_REGION)),
            code: "InvalidVPCId",
        },
        RefusedRow {
            case: "a VPCId one character short of the 17-character form",
            config: "",
            vpc: vpc_block(Some("vpc-0a1b2c3d4e5f6071"), Some(HOME_REGION)),
            code: "InvalidVPCId",
        },
        RefusedRow {
            case: "a VPCId one character past the 17-character form",
            config: "",
            vpc: vpc_block(Some("vpc-0a1b2c3d4e5f607189"), Some(HOME_REGION)),
            code: "InvalidVPCId",
        },
        RefusedRow {
            case: "a VPCId in uppercase hexadecimal",
            config: "<HostedZoneConfig><PrivateZone>true</PrivateZone></HostedZoneConfig>",
            vpc: vpc_block(Some("vpc-1A2B3C4D"), Some(HOME_REGION)),
            code: "InvalidVPCId",
        },
        RefusedRow {
            case: "a VPCId carrying a non-hexadecimal character",
            config: "",
            vpc: vpc_block(Some("vpc-1a2b3c4g"), Some(HOME_REGION)),
            code: "InvalidVPCId",
        },
        RefusedRow {
            case: "an id with no vpc- prefix",
            config: "",
            vpc: vpc_block(Some("1a2b3c4d"), Some(HOME_REGION)),
            code: "InvalidVPCId",
        },
    ];

    let router = router();
    for (index, row) in rows.iter().enumerate() {
        let reply = create_zone(
            &router,
            "refused.example.com",
            &format!("refused-{index}"),
            &format!("{}{}", row.config, row.vpc),
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
    }

    // A refusal creates nothing, so the account still holds no hosted zone.
    let listed = listed_zone_ids(&router, "").await;
    assert!(
        listed.is_empty(),
        "a refused CreateHostedZone must persist no zone, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2b: the region a VPC was created in has to be one a VPC can exist in
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_create_naming_a_region_no_vpc_can_exist_in_is_refused() {
    // `VPC$VPCRegion` targets the modelled `VPCRegion` enum
    // (smithy_query(query="shape",shape="VPCRegion"), `len 1..64`), so a value
    // outside it names no region a VPC can have been created in and no private
    // zone is created for it. The rows walk that boundary: a well-formed region
    // name the enum does not carry, one differing from a member only in case,
    // and one past the member length. The REQUEST boundary refuses every
    // modelled constraint before the handler runs, and this service states such
    // a refusal under the one input error its model declares, `InvalidInput`
    // (HTTP 400) — the same code the handler's own backstop names — so what a
    // client branches on is the same whichever layer refused, beside the zone
    // that was not created.
    let rows = [
        ("a region name the enum does not carry", "xx-nowhere-1".to_string()),
        ("a modelled region in another case", "US-EAST-1".to_string()),
        ("a region name past the modelled maximum", "e".repeat(65)),
    ];

    let router = router();
    for (index, (case, region)) in rows.iter().enumerate() {
        let reply = create_zone(
            &router,
            "bad-region.example.com",
            &format!("bad-region-{index}"),
            &vpc_block(Some("vpc-1a2b3c4d"), Some(region)),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidInput",
            "case {case:?}: expected InvalidInput, got body {}",
            reply.body
        );
    }

    // A refusal creates nothing.
    let listed = listed_zone_ids(&router, "").await;
    assert!(
        listed.is_empty(),
        "a refused CreateHostedZone must persist no zone, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: the zone a VPC-only create makes is a private zone downstream
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_vpc_only_private_zone_takes_further_associations() {
    // The rows are the further VPCs associated with the zone, differing along
    // the id form and the region, so the assertion is about the growing set
    // rather than about one association.
    let further = [
        ("vpc-1a2b3c4d", "eu-central-1"),
        ("vpc-0123456789abcdef0", "ap-northeast-1"),
    ];
    let creation_vpc = "vpc-0a1b2c3d4e5f60718";

    let router = router();
    let created = create_zone(
        &router,
        "split-horizon.example.com",
        "vpc-only-associations",
        &vpc_block(Some(creation_vpc), Some(HOME_REGION)),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "a VPC-only create must answer 201, got {} with body {}",
        created.status, created.body
    );
    let zone_id = bare_zone_id(&created);

    let mut expected = vec![(creation_vpc.to_string(), HOME_REGION.to_string())];
    for (vpc_id, region) in further {
        let reply = associate(&router, &zone_id, &vpc_block(Some(vpc_id), Some(region)), None)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {vpc_id} in {region}: a private zone must take the association, got {} \
             with body {}",
            reply.status, reply.body
        );
        expected.push((vpc_id.to_string(), region.to_string()));

        let read = get_zone(&router, &zone_id).await;
        let got = associations(&read);
        assert_eq!(
            got, expected,
            "case {vpc_id} in {region}: expected associations {expected:?}, got {got:?}; \
             body {}",
            read.body
        );
    }
}
