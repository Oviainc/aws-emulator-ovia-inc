// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `CreateVPCAssociationAuthorization` and the read that
//! reports what it wrote, `ListVPCAssociationAuthorizations`.
//!
//! The rules under test, each asserted over a SET of inputs rather than one
//! recorded request:
//!
//! 1. A hosted zone's VPC association authorizations are a SET keyed by VPC id:
//!    authorizing a VPC appends it (with the region it was created in) to the
//!    list the read pages over, every accepted request echoes the zone's own
//!    `ResourceId` and the VPC it authorized, and authorizing a VPC the zone
//!    already authorizes leaves the list as it was. The table spans the two VPC
//!    id forms and several regions of the modelled `VPCRegion` enum.
//! 2. An authorization only PERMITS a later association: it does not associate
//!    the VPC, and associating the VPC does not consume it.
//! 3. The read pages over that list: a page holds at most the asked-for number
//!    of VPCs (bounded by the documented 50 per page), `NextToken` is present
//!    exactly while authorizations are left over, and a walk covers every
//!    authorization exactly once in the order they were authorized.
//! 4. A request has to name a VPC and an existing zone: an id outside the
//!    `vpc-<hex>` form is `InvalidVPCId` and a missing region is `InvalidInput`
//!    (both HTTP 400), an id that names no zone is `NoSuchHostedZone` (HTTP 404)
//!    for both operations, and a `NextToken` naming no authorization of the zone
//!    is `InvalidPaginationToken` (HTTP 400) — none of them touching the list.
//! 5. One zone's authorization list is bounded, and the authorization past the
//!    bound is the modelled `TooManyVPCAssociationAuthorizations` (HTTP 400).
//!
//! Every test drives the service through the real router, using the SAME
//! harness `hosted_zone_records` and `vpc_associations` use, so what is asserted
//! is the HTTP/restXml behaviour a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use crate::hosted_zone_records::{call, create_zone_ok, router, Reply};
use crate::vpc_associations::{
    associate, associations, create_private_zone, get_zone, vpc_block, CREATION_VPC, HOME_REGION,
};

/// The most VPCs one page of `ListVPCAssociationAuthorizations` carries, as the
/// operation's `MaxResults` documents it ("If you don't specify a value for
/// MaxResults, Route 53 returns up to 50 VPCs per page").
const PAGE_MAXIMUM: usize = 50;

/// Send one `CreateVPCAssociationAuthorization` request carrying `vpc` (a
/// [`vpc_block`]).
pub(crate) async fn authorize(router: &MultiServiceRouter, zone_id: &str, vpc: &str) -> Reply {
    let body = format!(
        "<CreateVPCAssociationAuthorizationRequest \
         xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">{vpc}\
         </CreateVPCAssociationAuthorizationRequest>"
    );
    call(
        router,
        "POST",
        &format!("/2013-04-01/hostedzone/{zone_id}/authorizevpcassociation"),
        &body,
    )
    .await
}

/// Authorize `vpc_id` in `region` and assert the request was accepted.
pub(crate) async fn authorize_ok(
    router: &MultiServiceRouter,
    zone_id: &str,
    vpc_id: &str,
    region: &str,
) {
    let reply = authorize(router, zone_id, &vpc_block(Some(vpc_id), Some(region))).await;
    assert_eq!(
        reply.status, 200,
        "case {vpc_id:?}: expected 200 from CreateVPCAssociationAuthorization, got {} with \
         body {}",
        reply.status, reply.body
    );
}

/// Send one `ListVPCAssociationAuthorizations` request, with `query` appended to
/// the URI (`"?maxresults=2"`).
pub(crate) async fn list_authorizations(
    router: &MultiServiceRouter,
    zone_id: &str,
    query: &str,
) -> Reply {
    call(
        router,
        "GET",
        &format!("/2013-04-01/hostedzone/{zone_id}/authorizevpcassociation{query}"),
        "",
    )
    .await
}

/// Every `(VPCId, VPCRegion)` pair one page reports, in document order.
///
/// The same reader `vpc_associations` uses for a zone's association list: both
/// lists are `VPC` elements, so one extractor serves both.
pub(crate) fn page_of(reply: &Reply) -> Vec<(String, String)> {
    associations(reply)
}

/// Walk every page of `ListVPCAssociationAuthorizations`, returning the pairs it
/// reported in order and the size of each page.
async fn walk_pages(
    router: &MultiServiceRouter,
    zone_id: &str,
    max_results: Option<i32>,
    case: &str,
) -> (Vec<(String, String)>, Vec<usize>) {
    let mut token: Option<String> = None;
    let mut seen = Vec::new();
    let mut sizes = Vec::new();
    loop {
        let mut parts = Vec::new();
        if let Some(max) = max_results {
            parts.push(format!("maxresults={max}"));
        }
        if let Some(value) = &token {
            parts.push(format!("nexttoken={value}"));
        }
        let query = if parts.is_empty() {
            String::new()
        } else {
            format!("?{}", parts.join("&"))
        };

        let reply = list_authorizations(router, zone_id, &query).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200 from ListVPCAssociationAuthorizations{query}, got {} \
             with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.first("HostedZoneId"),
            Some(zone_id),
            "case {case:?}: every page must report the zone it listed; body {}",
            reply.body
        );
        let page = page_of(&reply);
        assert!(
            page.len() <= PAGE_MAXIMUM,
            "case {case:?}: a page carries at most {PAGE_MAXIMUM} VPCs, got {}",
            page.len()
        );
        sizes.push(page.len());
        seen.extend(page);

        match reply.first("NextToken") {
            Some(next) => {
                assert!(
                    !next.is_empty(),
                    "case {case:?}: a NextToken that is present must name the next page; body {}",
                    reply.body
                );
                assert!(
                    sizes.last().copied() != Some(0),
                    "case {case:?}: a page that reported nothing must not ask the caller to \
                     page again; body {}",
                    reply.body
                );
                token = Some(next.to_string());
            }
            None => break,
        }
        assert!(
            sizes.len() < 1_000,
            "case {case:?}: paging must terminate, got {} pages",
            sizes.len()
        );
    }
    (seen, sizes)
}

/// The VPC id of the `index`-th member of a generated authorization set: the
/// 17-character long form, which is lowercase hexadecimal.
pub(crate) fn generated_vpc_id(index: usize) -> String {
    format!("vpc-{index:017x}")
}

// ---------------------------------------------------------------------------
// Rule 1: an authorization is appended to the zone's set and read back
// ---------------------------------------------------------------------------

/// One row of the authorization table: a VPC to authorize, named by its id form
/// and the region it was created in.
struct VpcRow {
    case: &'static str,
    vpc_id: &'static str,
    region: &'static str,
}

#[tokio::test]
async fn authorizing_vpcs_appends_them_to_the_list_the_read_reports() {
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
        "authorize.example.com",
        "authorize-table",
        CREATION_VPC,
    )
    .await;

    // Nothing is authorized until a request authorizes it.
    let empty = list_authorizations(&router, &zone_id, "").await;
    assert_eq!(
        empty.status, 200,
        "expected 200 from ListVPCAssociationAuthorizations on a zone with none, got {} with \
         body {}",
        empty.status, empty.body
    );
    assert_eq!(
        page_of(&empty),
        Vec::new(),
        "a zone starts with no authorization; body {}",
        empty.body
    );
    assert_eq!(
        empty.first("NextToken"),
        None,
        "a single page must not ask the caller to page again; body {}",
        empty.body
    );

    let mut expected: Vec<(String, String)> = Vec::new();
    for row in &rows {
        let reply = authorize(
            &router,
            &zone_id,
            &vpc_block(Some(row.vpc_id), Some(row.region)),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200 from CreateVPCAssociationAuthorization, got {} with body {}",
            row.case, reply.status, reply.body
        );
        // The response names the zone the authorization was created on and the
        // VPC it authorizes.
        assert_eq!(
            reply.first("HostedZoneId"),
            Some(zone_id.as_str()),
            "case {:?}: the reply must report the zone it authorized on; body {}",
            row.case,
            reply.body
        );
        assert_eq!(
            page_of(&reply),
            vec![(row.vpc_id.to_string(), row.region.to_string())],
            "case {:?}: the reply must report the VPC it authorized; body {}",
            row.case,
            reply.body
        );

        expected.push((row.vpc_id.to_string(), row.region.to_string()));
        let read = list_authorizations(&router, &zone_id, "").await;
        let got = page_of(&read);
        assert_eq!(
            got, expected,
            "case {:?}: the read must report the authorizations the zone now holds, expected \
             {expected:?}, got {got:?}; body {}",
            row.case, read.body
        );
    }
}

#[tokio::test]
async fn re_authorizing_a_vpc_leaves_the_authorization_set_as_it_was() {
    // The rule: the set is keyed by VPC id, so the same VPC is authorized once
    // however many times it is requested — and the repeat is accepted, rather
    // than refused or duplicated.
    let router = router();
    let zone_id = create_private_zone(
        &router,
        "repeat-authorize.example.com",
        "authorize-repeat",
        CREATION_VPC,
    )
    .await;

    let first = ("vpc-1a2b3c4d", "eu-west-2");
    let second = ("vpc-0123456789abcdef0", "sa-east-1");
    authorize_ok(&router, &zone_id, first.0, first.1).await;
    authorize_ok(&router, &zone_id, second.0, second.1).await;
    let expected = vec![
        (first.0.to_string(), first.1.to_string()),
        (second.0.to_string(), second.1.to_string()),
    ];

    // The rows differ along which already-authorized VPC is authorized again,
    // and along whether the repeat names the region it was authorized with.
    let repeats = [
        ("the first VPC, with its own region", first.0, first.1),
        ("the second VPC, with its own region", second.0, second.1),
        ("the first VPC, named with another region", first.0, "us-west-1"),
    ];
    for (case, vpc_id, region) in repeats {
        for round in 0..2 {
            let reply = authorize(&router, &zone_id, &vpc_block(Some(vpc_id), Some(region))).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?} round {round}: expected 200 for a repeat authorization, got {} \
                 with body {}",
                reply.status, reply.body
            );

            let read = list_authorizations(&router, &zone_id, "").await;
            let got = page_of(&read);
            assert_eq!(
                got, expected,
                "case {case:?} round {round}: a repeat must leave the set at {expected:?}, got \
                 {got:?}; body {}",
                read.body
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: an authorization is not an association
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_authorization_neither_associates_the_vpc_nor_is_consumed_by_one() {
    let router = router();
    let zone_id = create_private_zone(
        &router,
        "authorize-then-associate.example.com",
        "authorize-vs-associate",
        CREATION_VPC,
    )
    .await;

    // The rows differ along what is done with the authorized VPC afterwards:
    // nothing at all, and a full association.
    let rows = [
        ("left unassociated", "vpc-1a2b3c4d", "eu-central-1", false),
        (
            "associated afterwards",
            "vpc-0a1b2c3d4e5f60718",
            "eu-central-1",
            true,
        ),
    ];

    for (case, vpc_id, region, then_associate) in rows {
        authorize_ok(&router, &zone_id, vpc_id, region).await;

        // Authorizing changed the authorization list, never the association
        // list: the zone still holds only the VPC it was created with.
        let read = get_zone(&router, &zone_id).await;
        let associated = associations(&read);
        assert_eq!(
            associated,
            vec![(CREATION_VPC.to_string(), HOME_REGION.to_string())],
            "case {case:?}: authorizing must not associate the VPC, got {associated:?}; body {}",
            read.body
        );

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
            let read = get_zone(&router, &zone_id).await;
            let associated = associations(&read);
            assert!(
                associated.iter().any(|(id, _)| id == vpc_id),
                "case {case:?}: the VPC must now be associated, got {associated:?}; body {}",
                read.body
            );
        }

        // Either way the authorization is still listed: Route 53 keeps it until
        // a DeleteVPCAssociationAuthorization request removes it.
        let read = list_authorizations(&router, &zone_id, "").await;
        let got = page_of(&read);
        assert!(
            got.iter()
                .any(|(id, reported)| id == vpc_id && reported == region),
            "case {case:?}: the authorization must survive, got {got:?}; body {}",
            read.body
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the read pages over the list
// ---------------------------------------------------------------------------

/// One row of the paging table: the `MaxResults` sent, and the page length it
/// must produce while authorizations are left over (`None` where the emulator's
/// own bound decides it, and only the invariants are asserted).
struct PageRow {
    case: &'static str,
    max_results: Option<i32>,
    full_page: Option<usize>,
}

#[tokio::test]
async fn authorization_paging_covers_every_authorization_exactly_once() {
    // More authorizations than one page can hold, so the documented 50-per-page
    // bound is observable.
    const TOTAL: usize = 60;

    let router = router();
    let zone_id = create_private_zone(
        &router,
        "paging-authorize.example.com",
        "authorize-paging",
        CREATION_VPC,
    )
    .await;

    // Regions drawn from the modelled `VPCRegion` enum, cycled so the set spans
    // several of them rather than repeating one.
    let regions = ["us-east-1", "eu-west-1", "ap-south-1", "me-central-1"];
    let mut expected = Vec::new();
    for index in 0..TOTAL {
        let vpc_id = generated_vpc_id(index);
        let region = regions[index % regions.len()];
        authorize_ok(&router, &zone_id, &vpc_id, region).await;
        expected.push((vpc_id, region.to_string()));
    }

    // The rows differ along the asked-for page size: one, a size that does not
    // divide the total, the documented maximum, one past it, no value at all
    // (the documented default), and sizes below the bottom of the range.
    let rows = [
        PageRow {
            case: "one VPC per page",
            max_results: Some(1),
            full_page: Some(1),
        },
        PageRow {
            case: "a page size that does not divide the total",
            max_results: Some(7),
            full_page: Some(7),
        },
        PageRow {
            case: "the documented page maximum",
            max_results: Some(50),
            full_page: Some(PAGE_MAXIMUM),
        },
        PageRow {
            case: "one past the documented page maximum",
            max_results: Some(51),
            full_page: Some(PAGE_MAXIMUM),
        },
        PageRow {
            case: "no MaxResults at all, which is the documented default",
            max_results: None,
            full_page: Some(PAGE_MAXIMUM),
        },
        PageRow {
            case: "a page size of zero",
            max_results: Some(0),
            full_page: None,
        },
        PageRow {
            case: "a negative page size",
            max_results: Some(-7),
            full_page: None,
        },
    ];

    for row in &rows {
        let (seen, sizes) = walk_pages(&router, &zone_id, row.max_results, row.case).await;
        assert_eq!(
            seen, expected,
            "case {:?}: the walk must cover every authorization exactly once, in the order they \
             were authorized; got {seen:?}",
            row.case
        );
        assert!(
            sizes.iter().all(|size| *size > 0),
            "case {:?}: no page may be empty while authorizations are left over, got {sizes:?}",
            row.case
        );
        if let Some(full) = row.full_page {
            let (last, full_pages) = sizes
                .split_last()
                .expect("every walk reads at least one page");
            assert!(
                full_pages.iter().all(|size| *size == full),
                "case {:?}: every page but the last must hold {full} VPCs, got {sizes:?}",
                row.case
            );
            assert!(
                *last > 0 && *last <= full,
                "case {:?}: the last page must hold between 1 and {full} VPCs, got {sizes:?}",
                row.case
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the request has to name a VPC, an existing zone and a valid token
// ---------------------------------------------------------------------------

/// One row of the malformed-VPC table: the `<VPC>` block sent, and the wire
/// error code the authorization must be refused with.
struct BadVpcRow {
    case: &'static str,
    vpc_id: Option<&'static str>,
    region: Option<&'static str>,
    code: &'static str,
}

#[tokio::test]
async fn an_authorization_that_names_no_valid_vpc_is_refused() {
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
        // modelled constraint) before any handler runs — and Route 53 states
        // that refusal under the one input error its model declares,
        // `InvalidInput`, HTTP 400.
        BadVpcRow {
            case: "an empty VPCRegion",
            vpc_id: Some("vpc-1a2b3c4d"),
            region: Some(""),
            code: "InvalidInput",
        },
        BadVpcRow {
            case: "a region the VPCRegion enum does not name",
            vpc_id: Some("vpc-1a2b3c4d"),
            region: Some("mars-west-1"),
            code: "InvalidInput",
        },
        BadVpcRow {
            case: "a modelled region in the wrong case",
            vpc_id: Some("vpc-1a2b3c4d"),
            region: Some("US-EAST-1"),
            code: "InvalidInput",
        },
    ];

    let router = router();
    let zone_id = create_private_zone(
        &router,
        "bad-authorize.example.com",
        "authorize-bad-vpc",
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
        let reply = authorize(&router, &zone_id, &vpc_block(row.vpc_id, row.region)).await;
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

        let read = list_authorizations(&router, &zone_id, "").await;
        let got = page_of(&read);
        assert_eq!(
            got, untouched,
            "case {:?}: a refused request must leave the authorizations at {untouched:?}, got \
             {got:?}; body {}",
            row.case, read.body
        );
    }
}

#[tokio::test]
async fn both_authorization_operations_report_an_unknown_zone() {
    // The rows differ along the id shape a caller might pass: the zone-id form,
    // an id of another shape entirely, and the id of a zone that was deleted.
    let router = router();
    let doomed = create_zone_ok(&router, "deleted-authorize.example.com", "authorize-deleted")
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
        for (operation, reply) in [
            (
                "CreateVPCAssociationAuthorization",
                authorize(
                    &router,
                    &zone_id,
                    &vpc_block(Some("vpc-1a2b3c4d"), Some(HOME_REGION)),
                )
                .await,
            ),
            (
                "ListVPCAssociationAuthorizations",
                list_authorizations(&router, &zone_id, "").await,
            ),
        ] {
            assert_eq!(
                reply.status, 404,
                "case {case:?} on {operation}: expected 404, got {} with body {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "NoSuchHostedZone",
                "case {case:?} on {operation}: expected NoSuchHostedZone, got body {}",
                reply.body
            );
        }
    }
}

#[tokio::test]
async fn a_next_token_naming_no_authorization_of_the_zone_is_refused() {
    let router = router();
    let zone_id = create_private_zone(
        &router,
        "token-authorize.example.com",
        "authorize-token",
        CREATION_VPC,
    )
    .await;
    let other_zone = create_private_zone(
        &router,
        "token-other.example.com",
        "authorize-token-other",
        "vpc-2222333344445555a",
    )
    .await;
    authorize_ok(&router, &zone_id, "vpc-1a2b3c4d", HOME_REGION).await;
    authorize_ok(&router, &other_zone, "vpc-5a6b7c8d", "eu-west-3").await;

    // The rows differ along where the token came from: a well-formed VPC id
    // nobody authorized, one authorized on ANOTHER zone, a token of a shape no
    // page ever hands out, and one that is only an id of a VPC ASSOCIATED with
    // the zone rather than authorized for it.
    let rows = [
        ("a VPC id no authorization names", "vpc-99998888777766665"),
        ("a VPC authorized on another zone", "vpc-5a6b7c8d"),
        ("a token of another shape entirely", "not-a-token"),
        ("the VPC the zone is associated with", CREATION_VPC),
    ];

    for (case, token) in rows {
        let reply = list_authorizations(&router, &zone_id, &format!("?nexttoken={token}")).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidPaginationToken",
            "case {case:?}: expected InvalidPaginationToken, got body {}",
            reply.body
        );
    }

    // The zone's own list is still readable, and still reports what it held.
    let read = list_authorizations(&router, &zone_id, "").await;
    let got = page_of(&read);
    assert_eq!(
        got,
        vec![("vpc-1a2b3c4d".to_string(), HOME_REGION.to_string())],
        "a refused token must leave the list readable, got {got:?}; body {}",
        read.body
    );
}

// ---------------------------------------------------------------------------
// Rule 5: one zone's authorization list is bounded
// ---------------------------------------------------------------------------

#[tokio::test]
async fn authorizations_are_bounded_at_the_per_zone_quota() {
    // Route 53's documented per-zone quota on authorizations for associating
    // VPCs with a private hosted zone.
    const QUOTA: usize = 1_000;

    let router = router();
    let zone_id = create_private_zone(
        &router,
        "quota-authorize.example.com",
        "authorize-quota",
        CREATION_VPC,
    )
    .await;

    for index in 0..QUOTA {
        authorize_ok(&router, &zone_id, &generated_vpc_id(index), "us-east-2").await;
    }

    // The rows differ along what the request past the quota asks for: a VPC the
    // zone does not authorize yet (refused), and one it already does (accepted,
    // because it adds nothing).
    let rows = [
        ("a VPC not authorized yet", generated_vpc_id(QUOTA), 400),
        ("a VPC already authorized", generated_vpc_id(0), 200),
    ];
    for (case, vpc_id, expected_status) in rows {
        let reply = authorize(
            &router,
            &zone_id,
            &vpc_block(Some(&vpc_id), Some("us-east-2")),
        )
        .await;
        assert_eq!(
            reply.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} with body {}",
            reply.status, reply.body
        );
        if expected_status == 400 {
            assert_eq!(
                reply.error_code(),
                "TooManyVPCAssociationAuthorizations",
                "case {case:?}: expected TooManyVPCAssociationAuthorizations, got body {}",
                reply.body
            );
        }
    }

    // The list still holds exactly the quota, and nothing beyond it.
    let (seen, _) = walk_pages(&router, &zone_id, None, "at the quota").await;
    assert_eq!(
        seen.len(),
        QUOTA,
        "the zone must hold exactly {QUOTA} authorizations, got {}",
        seen.len()
    );
    assert!(
        seen.iter()
            .all(|(id, _)| *id != generated_vpc_id(QUOTA)),
        "the refused authorization must not have been persisted"
    );
}
