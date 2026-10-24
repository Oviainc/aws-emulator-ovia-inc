// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListHostedZonesByVPC`.
//!
//! The rules under test, each over a TABLE that spans the class it generalises
//! over rather than the one VPC that exposed it:
//!
//! 1. The operation REPORTS the associations held by the hosted zones in state:
//!    a zone is listed exactly when it holds an association naming both the
//!    requested VPC id and the requested VPC region, so a public zone, a zone
//!    associated with another VPC, and a zone whose association names the same
//!    VPC id in a different region are all left out. The summaries come back in
//!    hosted-zone-id order, each carrying the zone's id, its normalized name,
//!    and an owner that is the account the emulator presents (never an Amazon
//!    Web Services service, since every zone here was created through this API).
//! 2. A page carries at most `MaxItems` summaries, `NextToken` names the first
//!    zone the page left out, and the next request resumes there — so walking
//!    the pages yields every associated zone exactly once, in the same order,
//!    whatever the page size. `MaxItems` is echoed as the caller sent it.
//! 3. A `NextToken` this operation never minted identifies no page, so it is
//!    the modelled `InvalidPaginationToken` (400); a request whose required
//!    `VPCId` names no VPC at all is `InvalidInput` (400).
//!
//! Every test drives the service through the real router, using the same
//! harness the sibling suites use, so what is asserted is the HTTP/restXml
//! behaviour a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use crate::hosted_zone_records::{call, create_zone_body, router, Reply};

/// The account every zone in this emulator belongs to, read from the runtime
/// rather than written as a literal.
fn owning_account() -> String {
    ::engine::emulator_account_id().to_string()
}

/// One private hosted zone to create: its name and the VPC it is created with.
/// A row with `vpc: None` is a PUBLIC zone, which can hold no association.
#[derive(Debug, Clone, Copy)]
struct ZoneFixture {
    name: &'static str,
    /// `(VPCId, VPCRegion)`, both as `CreateHostedZone` is given them.
    vpc: Option<(&'static str, &'static str)>,
}

/// The zones every test in this file works from. The VPC ids and regions span
/// the axes rule 1 generalises over: a VPC with several zones, a VPC with one,
/// the same VPC id in two regions, and zones with no VPC at all. Each region is
/// a member of the modelled `VPCRegion` enum
/// (smithy_query(query="shape",shape="VPCRegion")).
const ZONES: &[ZoneFixture] = &[
    ZoneFixture {
        name: "one.payments.example.com",
        vpc: Some(("vpc-00000000000000001", "us-east-1")),
    },
    ZoneFixture {
        name: "two.payments.example.com",
        vpc: Some(("vpc-00000000000000001", "us-east-1")),
    },
    ZoneFixture {
        name: "three.payments.example.net",
        vpc: Some(("vpc-00000000000000001", "us-east-1")),
    },
    ZoneFixture {
        name: "only.reporting.example.org",
        vpc: Some(("vpc-00000000000000002", "eu-west-1")),
    },
    ZoneFixture {
        name: "west.shared.example.com",
        vpc: Some(("vpc-00000000000000003", "us-west-2")),
    },
    ZoneFixture {
        // The SAME VPC id as the row above, in another region: the association
        // names a pair, so this zone belongs to the other region's listing.
        name: "south.shared.example.com",
        vpc: Some(("vpc-00000000000000003", "ap-south-1")),
    },
    ZoneFixture {
        name: "public.example.com",
        vpc: None,
    },
];

/// Create every fixture zone and return `name -> bare zone id`.
async fn seed(router: &MultiServiceRouter) -> Vec<(&'static str, String)> {
    let mut created = Vec::new();
    for (index, fixture) in ZONES.iter().enumerate() {
        let config = match fixture.vpc {
            Some((vpc_id, region)) => format!(
                "<HostedZoneConfig><PrivateZone>true</PrivateZone></HostedZoneConfig>\
                 <VPC><VPCRegion>{region}</VPCRegion><VPCId>{vpc_id}</VPCId></VPC>"
            ),
            None => String::new(),
        };
        let reply = call(
            router,
            "POST",
            "/2013-04-01/hostedzone",
            &create_zone_body(fixture.name, &format!("by-vpc-ref-{index}"), &config),
        )
        .await;
        assert_eq!(
            reply.status, 201,
            "creating {}: expected 201 from CreateHostedZone, got {} with body {}",
            fixture.name, reply.status, reply.body
        );
        let id = reply
            .first("HostedZone/Id")
            .unwrap_or_else(|| {
                panic!(
                    "creating {}: the reply must carry HostedZone/Id; body {}",
                    fixture.name, reply.body
                )
            })
            .trim_start_matches("/hostedzone/")
            .to_string();
        created.push((fixture.name, id));
    }
    created
}

/// The bare id of a seeded zone, by name.
fn id_of(created: &[(&'static str, String)], name: &str) -> String {
    created
        .iter()
        .find(|(zone_name, _)| *zone_name == name)
        .map(|(_, id)| id.clone())
        .unwrap_or_else(|| panic!("the fixture zone {name} must have been created"))
}

/// One `ListHostedZonesByVPC` request.
async fn list_by_vpc(
    router: &MultiServiceRouter,
    vpc_id: &str,
    vpc_region: &str,
    max_items: Option<i32>,
    next_token: Option<&str>,
) -> Reply {
    let mut uri = format!("/2013-04-01/hostedzonesbyvpc?vpcid={vpc_id}&vpcregion={vpc_region}");
    if let Some(max_items) = max_items {
        uri.push_str(&format!("&maxitems={max_items}"));
    }
    if let Some(token) = next_token {
        uri.push_str(&format!("&nexttoken={token}"));
    }
    call(router, "GET", &uri, "").await
}

/// The `(HostedZoneId, Name)` of every summary in a reply, in document order.
fn summaries(reply: &Reply) -> Vec<(String, String)> {
    let ids = reply.all("HostedZoneSummary/HostedZoneId");
    let names = reply.all("HostedZoneSummary/Name");
    assert_eq!(
        ids.len(),
        names.len(),
        "every HostedZoneSummary must carry both HostedZoneId and Name; body {}",
        reply.body
    );
    ids.into_iter()
        .zip(names)
        .map(|(id, name)| (id.to_string(), name.to_string()))
        .collect()
}

// ---------------------------------------------------------------------------
// Rule 1: which zones a VPC's listing reports
// ---------------------------------------------------------------------------

/// One row of the listing table: the VPC asked about, and the fixture zone
/// names its listing must report.
struct ListingRow {
    case: &'static str,
    vpc_id: &'static str,
    vpc_region: &'static str,
    expect_zones: &'static [&'static str],
}

#[tokio::test]
async fn a_vpcs_listing_reports_exactly_the_zones_associated_with_it() {
    let rows = [
        ListingRow {
            case: "a VPC several private zones are associated with",
            vpc_id: "vpc-00000000000000001",
            vpc_region: "us-east-1",
            expect_zones: &[
                "one.payments.example.com",
                "two.payments.example.com",
                "three.payments.example.net",
            ],
        },
        ListingRow {
            case: "a VPC exactly one zone is associated with",
            vpc_id: "vpc-00000000000000002",
            vpc_region: "eu-west-1",
            expect_zones: &["only.reporting.example.org"],
        },
        ListingRow {
            case: "the same VPC id, in the region its first zone names",
            vpc_id: "vpc-00000000000000003",
            vpc_region: "us-west-2",
            expect_zones: &["west.shared.example.com"],
        },
        ListingRow {
            case: "the same VPC id, in the region its second zone names",
            vpc_id: "vpc-00000000000000003",
            vpc_region: "ap-south-1",
            expect_zones: &["south.shared.example.com"],
        },
        ListingRow {
            case: "a known VPC id asked about in a region it has no zone in",
            vpc_id: "vpc-00000000000000001",
            vpc_region: "eu-central-1",
            expect_zones: &[],
        },
        ListingRow {
            case: "a VPC no zone is associated with",
            vpc_id: "vpc-0000000000000ffff",
            vpc_region: "us-east-1",
            expect_zones: &[],
        },
    ];

    let router = router();
    let created = seed(&router).await;

    for row in &rows {
        let reply = list_by_vpc(&router, row.vpc_id, row.vpc_region, None, None).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200 from ListHostedZonesByVPC, got {} with body {}",
            row.case, reply.status, reply.body
        );

        // The summaries come back in hosted-zone-id order, which is the order
        // the pagination token resumes in.
        let mut expected: Vec<(String, String)> = row
            .expect_zones
            .iter()
            .map(|name| (id_of(&created, name), format!("{name}.")))
            .collect();
        expected.sort();
        let actual = summaries(&reply);
        assert_eq!(
            actual, expected,
            "case {:?}: expected the summaries {expected:?}, got {actual:?}; body {}",
            row.case, reply.body
        );

        // Owner: the account this emulator presents, and no OwningService.
        let accounts = reply.all("Owner/OwningAccount");
        assert_eq!(
            accounts,
            vec![owning_account(); expected.len()],
            "case {:?}: every summary must carry Owner/OwningAccount {:?}, got {accounts:?}; \
             body {}",
            row.case,
            owning_account(),
            reply.body
        );
        assert!(
            reply.all("Owner/OwningService").is_empty(),
            "case {:?}: a zone owned by an account carries no OwningService, got {:?}; body {}",
            row.case,
            reply.all("Owner/OwningService"),
            reply.body
        );

        // `MaxItems` is reported even when the caller sent none.
        assert_eq!(
            reply.first("MaxItems"),
            Some("100"),
            "case {:?}: expected the default MaxItems to be reported, got {:?}; body {}",
            row.case,
            reply.first("MaxItems"),
            reply.body
        );
        assert!(
            reply.all("NextToken").is_empty(),
            "case {:?}: a listing that fits in one page carries no NextToken, got {:?}; body {}",
            row.case,
            reply.all("NextToken"),
            reply.body
        );
    }

    // The public zone is in state — ListHostedZones reports it — yet belongs to
    // no VPC listing above, which is what makes those listings a filter on the
    // associations rather than on nothing.
    let listed = call(&router, "GET", "/2013-04-01/hostedzone", "").await;
    let public_id = id_of(&created, "public.example.com");
    assert!(
        listed.all("HostedZone/Id").contains(&format!("/hostedzone/{public_id}").as_str()),
        "the public zone must be in state; body {}",
        listed.body
    );
}

#[tokio::test]
async fn the_listing_reflects_the_association_a_zone_was_created_with() {
    // The round trip: the association CreateHostedZone wrote is the one
    // GetHostedZone reports AND the one this listing selects on.
    let router = router();
    let created = seed(&router).await;
    let zone_id = id_of(&created, "west.shared.example.com");

    let read = call(
        &router,
        "GET",
        &format!("/2013-04-01/hostedzone/{zone_id}"),
        "",
    )
    .await;
    assert_eq!(
        read.status, 200,
        "expected 200 from GetHostedZone, got {} with body {}",
        read.status, read.body
    );
    assert_eq!(
        (read.first("VPC/VPCId"), read.first("VPC/VPCRegion")),
        (Some("vpc-00000000000000003"), Some("us-west-2")),
        "GetHostedZone must report the association the zone was created with; body {}",
        read.body
    );

    let reply = list_by_vpc(&router, "vpc-00000000000000003", "us-west-2", None, None).await;
    assert_eq!(
        summaries(&reply),
        vec![(zone_id.clone(), "west.shared.example.com.".to_string())],
        "the listing must report the zone whose association GetHostedZone just \
         reported; body {}",
        reply.body
    );
}

// ---------------------------------------------------------------------------
// Rule 2: pagination
// ---------------------------------------------------------------------------

#[tokio::test]
async fn walking_the_pages_yields_every_associated_zone_once_at_any_page_size() {
    // The page sizes span 1, a size that divides the count unevenly, the exact
    // count, one past it, and the published maximum plus one — `MaxItems` is an
    // Integer with no modelled range, and Route 53 lists "in groups of up to
    // 100", so 101 is one past that bound and still answers a full listing.
    let page_sizes = [None, Some(1), Some(2), Some(3), Some(4), Some(101)];

    let router = router();
    let created = seed(&router).await;
    let mut expected: Vec<(String, String)> = [
        "one.payments.example.com",
        "two.payments.example.com",
        "three.payments.example.net",
    ]
    .iter()
    .map(|name| (id_of(&created, name), format!("{name}.")))
    .collect();
    expected.sort();

    for max_items in page_sizes {
        let mut collected: Vec<(String, String)> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let reply = list_by_vpc(
                &router,
                "vpc-00000000000000001",
                "us-east-1",
                max_items,
                token.as_deref(),
            )
            .await;
            assert_eq!(
                reply.status, 200,
                "case {max_items:?}: expected 200 from ListHostedZonesByVPC, got {} with body {}",
                reply.status, reply.body
            );
            // The caller's own MaxItems is echoed back.
            let reported = reply.first("MaxItems").map(str::to_string);
            assert_eq!(
                reported.as_deref(),
                Some(max_items.unwrap_or(100).to_string().as_str()),
                "case {max_items:?}: expected MaxItems {:?} echoed, got {reported:?}; body {}",
                max_items.unwrap_or(100),
                reply.body
            );

            let page = summaries(&reply);
            let cap = max_items.unwrap_or(100).clamp(1, 100) as usize;
            assert!(
                page.len() <= cap,
                "case {max_items:?}: a page carries at most {cap} summaries, got {}; body {}",
                page.len(),
                reply.body
            );
            collected.extend(page.clone());
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {max_items:?}: the walk must terminate, got {pages} pages; body {}",
                reply.body
            );

            match reply.first("NextToken") {
                Some(next) => {
                    // The token names the first zone the page left out, so the
                    // next page starts exactly there.
                    assert_eq!(
                        next,
                        expected[collected.len()].0,
                        "case {max_items:?}: NextToken must name the next zone to return; body {}",
                        reply.body
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }

        assert_eq!(
            collected, expected,
            "case {max_items:?}: walking the pages must yield every associated zone once, \
             in id order; got {collected:?}"
        );
    }
}

#[tokio::test]
async fn a_token_past_every_associated_zone_lists_nothing() {
    // A well-formed token is a hosted-zone id to resume at; one that sorts
    // after every associated zone leaves an empty, untruncated page rather than
    // a refusal.
    let router = router();
    let _created = seed(&router).await;
    let reply = list_by_vpc(
        &router,
        "vpc-00000000000000001",
        "us-east-1",
        None,
        Some("ZZZZZZZZZZZZZZZ"),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 for a token past every zone, got {} with body {}",
        reply.status, reply.body
    );
    assert!(
        summaries(&reply).is_empty(),
        "expected no summaries past the last zone, got {:?}; body {}",
        summaries(&reply),
        reply.body
    );
    assert!(
        reply.all("NextToken").is_empty(),
        "an exhausted listing carries no NextToken; body {}",
        reply.body
    );
}

// ---------------------------------------------------------------------------
// Rule 3: refusals
// ---------------------------------------------------------------------------

/// One refused request: what makes it bad, and the modelled refusal it earns.
struct RefusalRow {
    case: &'static str,
    vpc_id: &'static str,
    vpc_region: &'static str,
    next_token: Option<&'static str>,
    code: &'static str,
}

#[tokio::test]
async fn a_bad_vpc_id_or_pagination_token_is_refused() {
    let rows = [
        RefusalRow {
            // `VPCId` is `len 0..1024`, so an empty one satisfies every
            // modelled constraint and the handler is what refuses it.
            case: "an empty VPCId names no VPC",
            vpc_id: "",
            vpc_region: "us-east-1",
            next_token: None,
            code: "InvalidInput",
        },
        RefusalRow {
            case: "a token this operation never minted",
            vpc_id: "vpc-00000000000000001",
            vpc_region: "us-east-1",
            next_token: Some("not-a-token"),
            code: "InvalidPaginationToken",
        },
        RefusalRow {
            case: "a token carrying the id prefix but no id body",
            vpc_id: "vpc-00000000000000001",
            vpc_region: "us-east-1",
            next_token: Some("Z"),
            code: "InvalidPaginationToken",
        },
        RefusalRow {
            case: "a token longer than any hosted-zone id",
            vpc_id: "vpc-00000000000000001",
            vpc_region: "us-east-1",
            next_token: Some("ZAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
            code: "InvalidPaginationToken",
        },
    ];

    let router = router();
    let _created = seed(&router).await;

    for row in &rows {
        let reply = list_by_vpc(
            &router,
            row.vpc_id,
            row.vpc_region,
            None,
            row.next_token,
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {:?}: expected HTTP 400, got {} with body {}",
            row.case, reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            row.code,
            "case {:?}: expected the error code {}, got body {}",
            row.case,
            row.code,
            reply.body
        );
    }
}
