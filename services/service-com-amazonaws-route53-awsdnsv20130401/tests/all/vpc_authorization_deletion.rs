// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `DeleteVPCAssociationAuthorization`, read back through
//! `ListVPCAssociationAuthorizations`.
//!
//! The rules under test, each asserted over a SET of inputs rather than one
//! recorded request:
//!
//! 1. Removing an authorization takes exactly the named VPC out of the zone's
//!    authorization set — the entry the read no longer reports — leaving every
//!    other authorization where it was, and answers HTTP 200 with an empty body.
//!    The table spans both VPC id forms, several regions of the modelled
//!    `VPCRegion` enum, and which entry of the set is removed (the first, one in
//!    the middle, the last and the only one).
//! 2. It withdraws a PERMISSION only: a VPC that is also ASSOCIATED with the
//!    zone stays associated after its authorization is removed.
//! 3. A request naming a VPC the zone does not authorize asks to remove an
//!    authorization that is not there, which is `VPCAssociationAuthorizationNotFound`
//!    (HTTP 404) — never authorized, already removed, or authorized on ANOTHER
//!    zone — and it leaves the set as it was.
//! 4. The request has to name a VPC and an existing zone, exactly as the
//!    authorization that created it did: an id outside the `vpc-<hex>` form is
//!    `InvalidVPCId` and a region the model does not name (absent, empty or
//!    outside the enum) is `InvalidInput` (both HTTP 400), and an id that names
//!    no zone is `NoSuchHostedZone` (HTTP 404) — none of them touching the set.
//! 5. An authorization that was removed frees its place in the zone's set, so
//!    the VPC can be authorized again and the set is the removal's inverse.
//!
//! Every test drives the service through the real router, using the SAME harness
//! `vpc_association_authorizations` uses, so what is asserted is the
//! HTTP/restXml behaviour a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use crate::hosted_zone_records::{call, create_zone_ok, router, Reply};
use crate::vpc_association_authorizations::{
    authorize, authorize_ok, generated_vpc_id, list_authorizations, page_of,
};
use crate::vpc_associations::{
    associate, associations, create_private_zone, get_zone, vpc_block, CREATION_VPC, HOME_REGION,
};

/// Send one `DeleteVPCAssociationAuthorization` request carrying `vpc` (a
/// [`vpc_block`]).
async fn deauthorize(router: &MultiServiceRouter, zone_id: &str, vpc: &str) -> Reply {
    let body = format!(
        "<DeleteVPCAssociationAuthorizationRequest \
         xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">{vpc}\
         </DeleteVPCAssociationAuthorizationRequest>"
    );
    call(
        router,
        "POST",
        &format!("/2013-04-01/hostedzone/{zone_id}/deauthorizevpcassociation"),
        &body,
    )
    .await
}

/// Remove the authorization of `vpc_id` in `region` and assert the request was
/// accepted with the empty response the operation models.
async fn deauthorize_ok(
    router: &MultiServiceRouter,
    zone_id: &str,
    vpc_id: &str,
    region: &str,
    case: &str,
) {
    let reply = deauthorize(router, zone_id, &vpc_block(Some(vpc_id), Some(region))).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from DeleteVPCAssociationAuthorization, got {} with body {}",
        reply.status, reply.body
    );
    // The operation's output shape carries no member, so an accepted request
    // reports no VPC of its own.
    let reported = page_of(&reply);
    assert!(
        reported.is_empty(),
        "case {case:?}: the reply must report no VPC, got {reported:?}; body {}",
        reply.body
    );
}

/// The authorizations the zone holds, read back through the implemented read.
async fn authorizations_of(router: &MultiServiceRouter, zone_id: &str) -> Vec<(String, String)> {
    let read = list_authorizations(router, zone_id, "").await;
    assert_eq!(
        read.status, 200,
        "expected 200 from ListVPCAssociationAuthorizations, got {} with body {}",
        read.status, read.body
    );
    page_of(&read)
}

// ---------------------------------------------------------------------------
// Rule 1: a removal takes exactly the named authorization out of the set
// ---------------------------------------------------------------------------

/// One row of the removal table: the set to authorize, and which member of it
/// the request then removes.
struct RemovalRow {
    case: &'static str,
    /// The `(VPCId, VPCRegion)` pairs to authorize, in order.
    authorize: &'static [(&'static str, &'static str)],
    /// The index into `authorize` of the one to remove.
    remove: usize,
}

#[tokio::test]
async fn removing_an_authorization_takes_only_that_vpc_out_of_the_set() {
    // The rows differ along the size of the set, which member is removed (the
    // first, the middle, the last, the only one), the VPC id form (the original
    // 8-character form and the 17-character long form) and the region, drawn
    // from the modelled `VPCRegion` enum
    // (smithy_query(query="shape",shape="VPCRegion")) — including other
    // partitions, which the enum names too.
    let rows = [
        RemovalRow {
            case: "the only authorization, in the long id form",
            authorize: &[("vpc-0a1b2c3d4e5f60718", "ca-central-1")],
            remove: 0,
        },
        RemovalRow {
            case: "the only authorization, in the 8-character id form",
            authorize: &[("vpc-1a2b3c4d", "ap-southeast-2")],
            remove: 0,
        },
        RemovalRow {
            case: "the first of three",
            authorize: &[
                ("vpc-00112233445566778", "il-central-1"),
                ("vpc-deadbeef", "cn-north-1"),
                ("vpc-0fedcba9876543210", "us-gov-west-1"),
            ],
            remove: 0,
        },
        RemovalRow {
            case: "the middle of three",
            authorize: &[
                ("vpc-00112233445566778", "il-central-1"),
                ("vpc-deadbeef", "cn-north-1"),
                ("vpc-0fedcba9876543210", "us-gov-west-1"),
            ],
            remove: 1,
        },
        RemovalRow {
            case: "the last of three",
            authorize: &[
                ("vpc-00112233445566778", "eu-west-3"),
                ("vpc-deadbeef", "sa-east-1"),
                ("vpc-0fedcba9876543210", "me-central-1"),
            ],
            remove: 2,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // A zone of its own per row, so one row's removals cannot be read as
        // another's.
        let router = router();
        let zone_id = create_private_zone(
            &router,
            &format!("deauthorize-{index}.example.com"),
            &format!("deauthorize-table-{index}"),
            CREATION_VPC,
        )
        .await;

        for (vpc_id, region) in row.authorize {
            authorize_ok(&router, &zone_id, vpc_id, region).await;
        }

        let (removed_id, removed_region) = row.authorize[row.remove];
        deauthorize_ok(&router, &zone_id, removed_id, removed_region, row.case).await;

        let expected: Vec<(String, String)> = row
            .authorize
            .iter()
            .enumerate()
            .filter(|(at, _)| *at != row.remove)
            .map(|(_, (vpc_id, region))| (vpc_id.to_string(), region.to_string()))
            .collect();
        let got = authorizations_of(&router, &zone_id).await;
        assert_eq!(
            got, expected,
            "case {:?}: the read must report the set without {removed_id}, expected {expected:?}, \
             got {got:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn removals_can_empty_a_set_of_any_size_one_authorization_at_a_time() {
    // The rule holds however many authorizations the zone holds, so the set is
    // large enough to span several pages of the read, and the removals walk it
    // from the back as well as the front.
    const TOTAL: usize = 12;

    let router = router();
    let zone_id = create_private_zone(
        &router,
        "drain-authorize.example.com",
        "deauthorize-drain",
        CREATION_VPC,
    )
    .await;

    // Regions drawn from the modelled `VPCRegion` enum, cycled so the set spans
    // several of them rather than repeating one.
    let regions = ["us-east-1", "eu-west-1", "ap-south-1", "me-central-1"];
    let mut remaining: Vec<(String, String)> = Vec::new();
    for index in 0..TOTAL {
        let vpc_id = generated_vpc_id(index);
        let region = regions[index % regions.len()];
        authorize_ok(&router, &zone_id, &vpc_id, region).await;
        remaining.push((vpc_id, region.to_string()));
    }

    // Remove from the front and from the back in turn, so neither end of the
    // set is the only one exercised.
    let mut from_front = true;
    while !remaining.is_empty() {
        let at = if from_front { 0 } else { remaining.len() - 1 };
        from_front = !from_front;
        let (vpc_id, region) = remaining.remove(at);
        let case = format!("removing {vpc_id} at index {at}");
        deauthorize_ok(&router, &zone_id, &vpc_id, &region, &case).await;

        let got = authorizations_of(&router, &zone_id).await;
        assert_eq!(
            got, remaining,
            "case {case:?}: the read must report {remaining:?}, got {got:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a removal withdraws a permission, it does not disassociate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn removing_an_authorization_leaves_the_association_list_alone() {
    let router = router();
    let zone_id = create_private_zone(
        &router,
        "deauthorize-associated.example.com",
        "deauthorize-vs-associate",
        CREATION_VPC,
    )
    .await;

    // The rows differ along what was done with the authorized VPC before its
    // authorization is removed: nothing, and a full association.
    let rows = [
        ("left unassociated", "vpc-1a2b3c4d", "eu-central-1", false),
        (
            "associated first",
            "vpc-0a1b2c3d4e5f60718",
            "eu-central-1",
            true,
        ),
    ];

    for (case, vpc_id, region, then_associate) in rows {
        authorize_ok(&router, &zone_id, vpc_id, region).await;
        if then_associate {
            let reply = associate(
                &router,
                &zone_id,
                &vpc_block(Some(vpc_id), Some(region)),
                None,
            )
            .await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected 200 from AssociateVPCWithHostedZone, got {} with body {}",
                reply.status, reply.body
            );
        }

        let before = associations(&get_zone(&router, &zone_id).await);
        deauthorize_ok(&router, &zone_id, vpc_id, region, case).await;
        let read = get_zone(&router, &zone_id).await;
        let after = associations(&read);
        assert_eq!(
            after, before,
            "case {case:?}: removing an authorization must leave the association list at \
             {before:?}, got {after:?}; body {}",
            read.body
        );
        if then_associate {
            assert!(
                after.iter().any(|(id, _)| id == vpc_id),
                "case {case:?}: the VPC must stay associated, got {after:?}; body {}",
                read.body
            );
        }
    }

    // The zone still holds the VPC it was created with, and no authorization is
    // left over.
    let got = authorizations_of(&router, &zone_id).await;
    assert_eq!(
        got,
        Vec::new(),
        "every authorization was removed, got {got:?}"
    );
    let read = get_zone(&router, &zone_id).await;
    let associated = associations(&read);
    assert!(
        associated
            .iter()
            .any(|(id, region)| id == CREATION_VPC && region == HOME_REGION),
        "the creation VPC must stay associated, got {associated:?}; body {}",
        read.body
    );
}

// ---------------------------------------------------------------------------
// Rule 3: removing an authorization the zone does not hold is refused
// ---------------------------------------------------------------------------

#[tokio::test]
async fn removing_an_authorization_the_zone_does_not_hold_is_refused() {
    let router = router();
    let zone_id = create_private_zone(
        &router,
        "missing-authorize.example.com",
        "deauthorize-missing",
        CREATION_VPC,
    )
    .await;
    let other_zone = create_private_zone(
        &router,
        "missing-other.example.com",
        "deauthorize-missing-other",
        "vpc-2222333344445555a",
    )
    .await;

    let kept = ("vpc-11112222333344445", "us-west-2");
    let removed = ("vpc-5a6b7c8d", "eu-west-3");
    let elsewhere = ("vpc-6a7b8c9d", "ap-northeast-1");
    authorize_ok(&router, &zone_id, kept.0, kept.1).await;
    authorize_ok(&router, &zone_id, removed.0, removed.1).await;
    authorize_ok(&router, &other_zone, elsewhere.0, elsewhere.1).await;
    deauthorize_ok(&router, &zone_id, removed.0, removed.1, "the removed one").await;
    let untouched = vec![(kept.0.to_string(), kept.1.to_string())];

    // The rows differ along why the zone does not authorize the VPC: it never
    // did, an earlier request already removed the authorization, the
    // authorization is held by ANOTHER zone, and the VPC is merely ASSOCIATED
    // with the zone rather than authorized for it. The last row also removes
    // twice, so repeating a removal is refused the same way.
    let rows = [
        ("a VPC no authorization names", "vpc-99998888777766665", "us-east-1"),
        ("a VPC whose authorization was already removed", removed.0, removed.1),
        ("a VPC authorized on another zone", elsewhere.0, elsewhere.1),
        ("the VPC the zone is associated with", CREATION_VPC, HOME_REGION),
    ];

    for (case, vpc_id, region) in rows {
        for round in 0..2 {
            let reply = deauthorize(&router, &zone_id, &vpc_block(Some(vpc_id), Some(region))).await;
            assert_eq!(
                reply.status, 404,
                "case {case:?} round {round}: expected 404, got {} with body {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "VPCAssociationAuthorizationNotFound",
                "case {case:?} round {round}: expected VPCAssociationAuthorizationNotFound, got \
                 body {}",
                reply.body
            );

            let got = authorizations_of(&router, &zone_id).await;
            assert_eq!(
                got, untouched,
                "case {case:?} round {round}: a refused removal must leave the set at \
                 {untouched:?}, got {got:?}"
            );
        }
    }

    // The other zone's authorization is untouched by the refusals aimed at this
    // one.
    let got = authorizations_of(&router, &other_zone).await;
    assert_eq!(
        got,
        vec![(elsewhere.0.to_string(), elsewhere.1.to_string())],
        "another zone's authorization must be untouched, got {got:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: the request has to name a VPC and an existing zone
// ---------------------------------------------------------------------------

/// One row of the malformed-VPC table: the `<VPC>` block sent, and the wire
/// error code the removal must be refused with.
struct BadVpcRow {
    case: &'static str,
    vpc_id: Option<&'static str>,
    region: Option<&'static str>,
    code: &'static str,
    status: u16,
}

#[tokio::test]
async fn a_removal_that_names_no_valid_vpc_is_refused() {
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
            status: 400,
        },
        BadVpcRow {
            case: "an empty VPCId",
            vpc_id: Some(""),
            region: Some(HOME_REGION),
            code: "InvalidVPCId",
            status: 400,
        },
        BadVpcRow {
            case: "an id with no vpc- prefix",
            vpc_id: Some("1a2b3c4d"),
            region: Some(HOME_REGION),
            code: "InvalidVPCId",
            status: 400,
        },
        BadVpcRow {
            case: "another resource kind's prefix",
            vpc_id: Some("subnet-1a2b3c4d"),
            region: Some(HOME_REGION),
            code: "InvalidVPCId",
            status: 400,
        },
        BadVpcRow {
            case: "one character short of the 8-character form",
            vpc_id: Some("vpc-1a2b3c4"),
            region: Some(HOME_REGION),
            code: "InvalidVPCId",
            status: 400,
        },
        BadVpcRow {
            case: "one character past the 8-character form",
            vpc_id: Some("vpc-1a2b3c4d5"),
            region: Some(HOME_REGION),
            code: "InvalidVPCId",
            status: 400,
        },
        BadVpcRow {
            case: "one character short of the 17-character form",
            vpc_id: Some("vpc-0a1b2c3d4e5f6071"),
            region: Some(HOME_REGION),
            code: "InvalidVPCId",
            status: 400,
        },
        BadVpcRow {
            case: "one character past the 17-character form",
            vpc_id: Some("vpc-0a1b2c3d4e5f607182"),
            region: Some(HOME_REGION),
            code: "InvalidVPCId",
            status: 400,
        },
        BadVpcRow {
            case: "a non-hexadecimal character in the id",
            vpc_id: Some("vpc-1a2b3c4z"),
            region: Some(HOME_REGION),
            code: "InvalidVPCId",
            status: 400,
        },
        BadVpcRow {
            case: "an uppercase id body",
            vpc_id: Some("vpc-1A2B3C4D"),
            region: Some(HOME_REGION),
            code: "InvalidVPCId",
            status: 400,
        },
        BadVpcRow {
            case: "no VPCRegion member at all",
            vpc_id: Some("vpc-1a2b3c4d"),
            region: None,
            code: "InvalidInput",
            status: 400,
        },
        // `VPC$VPCRegion` targets the modelled `VPCRegion` enum, so a value
        // outside it is refused by the request boundary (which enforces every
        // modelled constraint) before any handler runs — and Route 53 states
        // that refusal under the one input error its model declares,
        // `InvalidInput`, HTTP 400.
        BadVpcRow {
            case: "an empty VPCRegion",
            vpc_id: Some("vpc-1a2b3c4d"),
            region: Some(""),
            code: "InvalidInput",
            status: 400,
        },
        BadVpcRow {
            case: "a region the VPCRegion enum does not name",
            vpc_id: Some("vpc-1a2b3c4d"),
            region: Some("mars-west-1"),
            code: "InvalidInput",
            status: 400,
        },
        BadVpcRow {
            case: "a modelled region in the wrong case",
            vpc_id: Some("vpc-1a2b3c4d"),
            region: Some("US-EAST-1"),
            code: "InvalidInput",
            status: 400,
        },
    ];

    let router = router();
    let zone_id = create_private_zone(
        &router,
        "bad-deauthorize.example.com",
        "deauthorize-bad-vpc",
        CREATION_VPC,
    )
    .await;
    // One authorization the refusals must leave alone.
    authorize_ok(&router, &zone_id, "vpc-11112222333344445", "us-west-2").await;
    let untouched = vec![(
        "vpc-11112222333344445".to_string(),
        "us-west-2".to_string(),
    )];

    for row in &rows {
        let reply = deauthorize(&router, &zone_id, &vpc_block(row.vpc_id, row.region)).await;
        assert_eq!(
            reply.status, row.status,
            "case {:?}: expected HTTP {}, got {} with body {}",
            row.case, row.status, reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            row.code,
            "case {:?}: expected {}, got body {}",
            row.case,
            row.code,
            reply.body
        );

        let got = authorizations_of(&router, &zone_id).await;
        assert_eq!(
            got, untouched,
            "case {:?}: a refused request must leave the set at {untouched:?}, got {got:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_removal_against_an_unknown_zone_is_refused() {
    // The rows differ along the id shape a caller might pass: the zone-id form,
    // an id of another shape entirely, and the id of a zone that was deleted.
    let router = router();
    let doomed = create_zone_ok(&router, "deleted-deauthorize.example.com", "deauthorize-deleted")
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
        let reply = deauthorize(
            &router,
            &zone_id,
            &vpc_block(Some("vpc-1a2b3c4d"), Some(HOME_REGION)),
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
// Rule 5: a removal frees the VPC's place in the set
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_removed_authorization_can_be_created_again() {
    let router = router();
    let zone_id = create_private_zone(
        &router,
        "recreate-authorize.example.com",
        "deauthorize-recreate",
        CREATION_VPC,
    )
    .await;

    // The rows differ along the id form and along whether the VPC is authorized
    // again in the region it first carried or in another one: an authorization
    // created after a removal is a NEW one, so it carries the region the new
    // request named.
    let rows = [
        (
            "the long id form, re-authorized in its own region",
            "vpc-0a1b2c3d4e5f60718",
            "eu-west-1",
            "eu-west-1",
        ),
        (
            "the 8-character id form, re-authorized in another region",
            "vpc-1a2b3c4d",
            "us-west-1",
            "ap-south-1",
        ),
    ];

    for (case, vpc_id, first_region, second_region) in rows {
        authorize_ok(&router, &zone_id, vpc_id, first_region).await;
        deauthorize_ok(&router, &zone_id, vpc_id, first_region, case).await;
        let got = authorizations_of(&router, &zone_id).await;
        assert!(
            !got.iter().any(|(id, _)| id == vpc_id),
            "case {case:?}: the removal must take {vpc_id} out of the set, got {got:?}"
        );

        let reply = authorize(&router, &zone_id, &vpc_block(Some(vpc_id), Some(second_region))).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200 re-authorizing {vpc_id}, got {} with body {}",
            reply.status, reply.body
        );
        let got = authorizations_of(&router, &zone_id).await;
        assert!(
            got.iter()
                .any(|(id, region)| id == vpc_id && region == second_region),
            "case {case:?}: the set must hold {vpc_id} in {second_region}, got {got:?}"
        );

        // Leave the set empty for the next row.
        deauthorize_ok(&router, &zone_id, vpc_id, second_region, case).await;
    }

    let got = authorizations_of(&router, &zone_id).await;
    assert_eq!(got, Vec::new(), "every authorization was removed, got {got:?}");
}
