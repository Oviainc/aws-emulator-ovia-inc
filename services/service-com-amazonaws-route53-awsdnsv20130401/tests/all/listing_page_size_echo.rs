// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of ONE rule: a `maxitems`-paged listing reports in its
//! required `MaxItems` the page size the REQUEST asked for.
//!
//! The rule under test: `MaxItems` in a Route 53 listing's response is "the
//! value that you specified for the maxitems parameter" — so the caller's value
//! when it named one, and the service's default page size when it named none.
//! The ceiling an operation publishes for `maxitems` ("Amazon Route 53 returns a
//! maximum of 1000 items") bounds the page the service SERVES; it is not the
//! size an unsized request asks for, and reporting it makes a paging client that
//! derives its loop bound or its "was this a full page" test from the echoed
//! value mis-detect truncation.
//!
//! One listing asked for one page size is ONE row of that class, so the tables
//! below span it: every `maxitems`-paged listing this service serves, crossed
//! with page sizes that differ in magnitude — none at all, the smallest, one
//! inside the population, the listing's own published bound and one past it.
//!
//! Every test drives the service through the real router, using the SAME
//! harness the sibling listing suites use, so what is asserted is the
//! HTTP/restXml behaviour a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::state::DEFAULT_PAGE_SIZE;

use crate::health_checks::create_health_check;
use crate::hosted_zone_records::{call, create_zone_ok, router_holding_vpcs, Reply};
use crate::reusable_delegation_sets::create_set;
use crate::traffic_policies::{create_policy, document_with_start_endpoint};
use crate::traffic_policy_versions::create_version;
use crate::vpc_associations::{create_private_zone, HOME_REGION};

/// The page size `ListResourceRecordSets` serves and reports when the request
/// names no `maxitems`, which this listing publishes for itself:
/// "ListResourceRecordSets returns up to 300 resource record sets at a time in
/// ASCII order"
/// (smithy_query(query="doc",shape="com.amazonaws.route53#ListResourceRecordSets")).
const RECORD_SET_PAGE_SIZE: i32 = 300;

/// The VPC a private fixture zone is associated with, which the
/// `ListHostedZonesByVPC` row addresses.
const FIXTURE_VPC: &str = "vpc-0f1e2d3c4b5a69780";

/// A router whose account holds [`FIXTURE_VPC`], so the by-VPC listing below is
/// served rather than refused as another account's VPC.
fn router() -> MultiServiceRouter {
    router_holding_vpcs(&[(FIXTURE_VPC, HOME_REGION)])
}

/// One row of the listing table.
struct ListingRow {
    case: &'static str,
    /// The request path, including any query parameters the operation requires.
    uri: String,
    /// The page size the operation serves and reports for a request that names
    /// no `maxitems`.
    default_page_size: i32,
    /// The ceiling the operation publishes for `maxitems`, which bounds the
    /// page it serves but never the value it reports.
    page_bound: i32,}

/// `uri` with `maxitems` appended, joined with `?` or `&` as the path needs.
fn with_max_items(uri: &str, max_items: Option<i32>) -> String {
    match max_items {
        None => uri.to_string(),
        Some(max_items) => {
            let join = if uri.contains('?') { '&' } else { '?' };
            format!("{uri}{join}maxitems={max_items}")
        }
    }
}

/// Seed one of every resource the listings below page over, and return the
/// table of listings addressed at them.
async fn seed(router: &MultiServiceRouter) -> Vec<ListingRow> {
    // Two public zones and a private one, so every zone listing has several
    // records to page and the by-VPC listing has one to find.
    let first = create_zone_ok(router, "paging-one.example.com", "paging-one").await;
    create_zone_ok(router, "paging-two.example.net", "paging-two").await;
    create_private_zone(
        router,
        "paging-private.example.org",
        "paging-private",
        FIXTURE_VPC,
    )
    .await;

    // Two health checks.
    for index in 0..2 {
        let reply = create_health_check(
            router,
            &format!("paging-check-{index}"),
            &format!("<Type>HTTP</Type><IPAddress>192.0.2.{}</IPAddress><Port>80</Port>", index + 1),
        )
        .await;
        assert_eq!(
            reply.status, 201,
            "seeding a health check: expected 201, got {} with body {}",
            reply.status, reply.body
        );
    }

    // A traffic policy with a second version, so the version listing pages over
    // more than one.
    let created = create_policy(
        router,
        "paging-policy",
        &document_with_start_endpoint("A", "192.0.2.10"),
        None,
    )
    .await;
    assert_eq!(
        created.status, 201,
        "seeding a traffic policy: expected 201, got {} with body {}",
        created.status, created.body
    );
    let policy_id = created
        .first("TrafficPolicy/Id")
        .unwrap_or_else(|| {
            panic!(
                "the created policy must carry TrafficPolicy/Id; body {}",
                created.body
            )
        })
        .to_string();
    let version = create_version(
        router,
        &policy_id,
        &document_with_start_endpoint("A", "192.0.2.11"),
        None,
    )
    .await;
    assert_eq!(
        version.status, 201,
        "seeding a traffic policy version: expected 201, got {} with body {}",
        version.status, version.body
    );

    // A reusable delegation set.
    let set = create_set(router, "paging-set", None).await;
    assert_eq!(
        set.status, 201,
        "seeding a reusable delegation set: expected 201, got {} with body {}",
        set.status, set.body
    );

    vec![
        ListingRow {
            case: "ListHostedZones",
            uri: "/2013-04-01/hostedzone".to_string(),
            default_page_size: DEFAULT_PAGE_SIZE,
            // "Amazon Route 53 returns a maximum of 100 items in each response."
            page_bound: 100,
        },
        ListingRow {
            case: "ListHostedZonesByName",
            uri: "/2013-04-01/hostedzonesbyname".to_string(),
            default_page_size: DEFAULT_PAGE_SIZE,
            // "Route 53 returns up to 100 items in each response."
            page_bound: 100,
        },
        ListingRow {
            case: "ListHostedZonesByVPC",
            uri: format!(
                "/2013-04-01/hostedzonesbyvpc?vpcid={FIXTURE_VPC}&vpcregion={HOME_REGION}"
            ),
            default_page_size: DEFAULT_PAGE_SIZE,
            page_bound: 100,
        },
        ListingRow {
            case: "ListHealthChecks",
            uri: "/2013-04-01/healthcheck".to_string(),
            default_page_size: DEFAULT_PAGE_SIZE,
            // "Amazon Route 53 returns a maximum of 1000 items."
            page_bound: 1_000,
        },
        ListingRow {
            case: "ListReusableDelegationSets",
            uri: "/2013-04-01/delegationset".to_string(),
            default_page_size: DEFAULT_PAGE_SIZE,
            // "If you specify a value greater than 100, Route 53 returns only
            // the first 100 reusable delegation sets."
            page_bound: 100,
        },
        ListingRow {
            case: "ListTrafficPolicies",
            uri: "/2013-04-01/trafficpolicies".to_string(),
            default_page_size: DEFAULT_PAGE_SIZE,
            // Bounded by the account's traffic policy quota, which no listing
            // can be longer than.
            page_bound: 50,
        },
        ListingRow {
            case: "ListTrafficPolicyInstances",
            uri: "/2013-04-01/trafficpolicyinstances".to_string(),
            default_page_size: DEFAULT_PAGE_SIZE,
            // "Route 53 returns a maximum of 100 items in each response."
            page_bound: 100,
        },
        ListingRow {
            case: "ListTrafficPolicyVersions",
            uri: format!("/2013-04-01/trafficpolicies/{policy_id}/versions"),
            default_page_size: DEFAULT_PAGE_SIZE,
            // Bounded by `TrafficPolicyVersion`'s own `range 1..1000`.
            page_bound: 1_000,
        },
        ListingRow {
            case: "ListResourceRecordSets",
            uri: format!("/2013-04-01/hostedzone/{first}/rrset"),
            // This listing publishes a page size of its own.
            default_page_size: RECORD_SET_PAGE_SIZE,
            page_bound: RECORD_SET_PAGE_SIZE,
        },
    ]
}

/// Send one listing request and assert it was served.
async fn listing(router: &MultiServiceRouter, case: &str, uri: &str) -> Reply {
    let reply = call(router, "GET", uri, "").await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from {uri}, got {} with body {}",
        reply.status, reply.body
    );
    reply
}

#[tokio::test]
async fn a_listing_asked_for_no_page_size_reports_the_default_page_size() {
    // The rule for the absent `maxitems`: the response reports the default page
    // size such a request asks for, never the operation's own truncation
    // ceiling. The rows are every `maxitems`-paged listing this service serves.
    let router = router();
    for row in seed(&router).await {
        let uri = with_max_items(&row.uri, None);
        let reply = listing(&router, row.case, &uri).await;
        let reported = reply.first("MaxItems");
        assert_eq!(
            reported,
            Some(row.default_page_size.to_string().as_str()),
            "case {:?}: a request naming no maxitems must report the default page size {}, \
             got {reported:?}; body {}",
            row.case,
            row.default_page_size,
            reply.body
        );
        // The page itself is not truncated: the fixture holds far fewer records
        // than the default page, so nothing follows it. `IsTruncated` is absent
        // from the token-paged listing, which says the same by naming no token
        // to resume at.
        assert_ne!(
            reply.first("IsTruncated"),
            Some("true"),
            "case {:?}: the whole fixture fits one default page; body {}",
            row.case,
            reply.body
        );
        for resume in ["NextMarker", "NextToken", "NextDNSName", "NextRecordName"] {
            assert!(
                reply.first(resume).is_none(),
                "case {:?}: a page that carries the whole fixture names no {resume}, got {:?}; \
                 body {}",
                row.case,
                reply.first(resume),
                reply.body
            );
        }
    }
}

#[tokio::test]
async fn a_listing_asked_for_a_page_size_reports_the_one_it_was_asked_for() {
    // The rule for a `maxitems` the request names: it is echoed whatever its
    // magnitude — the default is only what an unsized request is served — while
    // the page the response carries stays within the operation's own bound. The
    // sizes span the smallest, one inside the population, the published bound
    // and one past it.
    let router = router();
    for row in seed(&router).await {
        for requested in [1, 2, row.page_bound, row.page_bound + 1] {
            let case = format!("{} maxitems={requested}", row.case);
            let uri = with_max_items(&row.uri, Some(requested));
            let reply = listing(&router, &case, &uri).await;
            let reported = reply.first("MaxItems");
            assert_eq!(
                reported,
                Some(requested.to_string().as_str()),
                "case {case:?}: expected the requested maxitems echoed, got {reported:?}; \
                 body {}",
                reply.body
            );
        }
    }
}
