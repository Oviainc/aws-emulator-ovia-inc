// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListTrafficPolicyInstances`.
//!
//! The rule under test: the operation REPORTS the traffic policy instances the
//! account holds at the moment of the call, read from the service's state in the
//! order given by the hosted zone each instance's record sets live in, then the
//! DNS name they answer for, then their record type — the triple the listing's
//! markers carry — as a page that starts at the instance
//! `(HostedZoneIdMarker, TrafficPolicyInstanceNameMarker,
//! TrafficPolicyInstanceTypeMarker)` name, carries at most `MaxItems`
//! instances, echoes that `maxitems` back and names in those three markers the
//! first instance of the next page. So the pages concatenate to the whole
//! population exactly once, every listed instance reports what
//! `GetTrafficPolicyInstance` reports for it, and nothing a caller reads changes
//! what a later caller sees.
//!
//! The tables therefore vary the axes the rule generalises over rather than
//! renaming an instance: the POPULATION (none, then one more instance at a
//! time), the HOSTED ZONE the instance lives in (two of them, so the listing is
//! account-wide), the DNS NAME and its depth, the RECORD TYPE (including two
//! instances that differ only in it, which the type marker alone tells apart),
//! the TTL, the PAGE SIZE (every size from one to one past the population, plus
//! the published page bound and one past it) and the MARKERS (every position in
//! the listing, each component of the triple on its own, and markers the account
//! holds no instance under).
//!
//! Every test drives the service through the real router, using the SAME
//! harness the other traffic-policy suites use, so what is asserted is the
//! HTTP/restXml behaviour a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::ops::types::R_R_TYPE_VALUES;
use service_com_amazonaws_route53_awsdnsv20130401::state::{
    DEFAULT_PAGE_SIZE, TRAFFIC_POLICY_INSTANCE_APPLIED, TRAFFIC_POLICY_INSTANCE_CREATING,
};

use crate::hosted_zone_records::{call, create_zone_ok, router, Reply};
use crate::traffic_policies::{create_policy, document_with_start_endpoint, policy_of};
use crate::traffic_policy_instances::{create_instance, get_instance, instance_count};

/// The page this listing publishes for itself: "Route 53 returns a maximum of
/// 100 items in each response. If you have a lot of traffic policy instances,
/// you can use the MaxItems parameter to list them in groups of up to 100"
/// (smithy_query(query="doc",shape="com.amazonaws.route53#ListTrafficPolicyInstances")).
const PAGE_BOUND: i32 = 100;

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

/// Send one `ListTrafficPolicyInstances` request, with `query` as its query
/// string.
async fn list_instances(router: &MultiServiceRouter, query: &str) -> Reply {
    call(
        router,
        "GET",
        &format!("/2013-04-01/trafficpolicyinstances{query}"),
        "",
    )
    .await
}

/// List with `query`, asserting the call succeeded.
async fn listing(router: &MultiServiceRouter, case: &str, query: &str) -> Reply {
    let reply = list_instances(router, query).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from ListTrafficPolicyInstances, got {} with body {}",
        reply.status, reply.body
    );
    reply
}

// ---------------------------------------------------------------------------
// What a reply reports
// ---------------------------------------------------------------------------

/// One `TrafficPolicyInstance` as the wire reports it, apart from its `State`:
/// the state follows the clock, so it is asserted on its own rather than
/// compared for equality.
#[derive(Debug, Clone, PartialEq, Eq)]
struct InstanceView {
    id: String,
    hosted_zone_id: String,
    name: String,
    ttl: String,
    traffic_policy_id: String,
    traffic_policy_version: String,
    traffic_policy_type: String,
}

/// Every instance a reply reports, in the order it reports them.
///
/// Each member read here is required of `TrafficPolicyInstance`, so a reply
/// that reports one instance whole reports as many of each member as it has
/// instances; anything else is a malformed listing rather than a different
/// order.
fn instances_of(reply: &Reply, case: &str) -> Vec<InstanceView> {
    let pick = |member: &str| reply.all(&format!("TrafficPolicyInstance/{member}"));
    let ids = pick("Id");
    let zones = pick("HostedZoneId");
    let names = pick("Name");
    let ttls = pick("TTL");
    let policies = pick("TrafficPolicyId");
    let versions = pick("TrafficPolicyVersion");
    let types = pick("TrafficPolicyType");
    let widths = (
        zones.len(),
        names.len(),
        ttls.len(),
        policies.len(),
        versions.len(),
        types.len(),
    );
    let wanted = (
        ids.len(),
        ids.len(),
        ids.len(),
        ids.len(),
        ids.len(),
        ids.len(),
    );
    assert_eq!(
        widths, wanted,
        "case {case:?}: every listed instance must be reported whole, got {widths:?} beside \
         {} id(s); body {}",
        ids.len(),
        reply.body
    );
    (0..ids.len())
        .map(|at| InstanceView {
            id: ids[at].to_string(),
            hosted_zone_id: zones[at].to_string(),
            name: names[at].to_string(),
            ttl: ttls[at].to_string(),
            traffic_policy_id: policies[at].to_string(),
            traffic_policy_version: versions[at].to_string(),
            traffic_policy_type: types[at].to_string(),
        })
        .collect()
}

/// The markers of a page: `None` once the listing has ended, where there is no
/// instance left to resume at.
fn next_markers(reply: &Reply) -> Option<(String, String, String)> {
    let zone = reply.first("HostedZoneIdMarker")?;
    let name = reply.first("TrafficPolicyInstanceNameMarker")?;
    let rr_type = reply.first("TrafficPolicyInstanceTypeMarker")?;
    Some((zone.to_string(), name.to_string(), rr_type.to_string()))
}

/// The markers that resume a listing at `instance`, as a query string.
fn marker_query(instance: &InstanceView, max_items: Option<usize>) -> String {
    let size = max_items.map_or_else(String::new, |size| format!("&maxitems={size}"));
    format!(
        "?hostedzoneid={}&trafficpolicyinstancename={}&trafficpolicyinstancetype={}{size}",
        instance.hosted_zone_id, instance.name, instance.traffic_policy_type
    )
}

/// The place an instance takes in the listing: the hosted zone it is in, the
/// DNS name with its labels REVERSED (the order Route 53 reports DNS names in),
/// then the record type.
fn listing_key(instance: &InstanceView) -> (String, String, String) {
    let labels = instance.name.trim_end_matches('.').to_ascii_lowercase();
    let reversed: Vec<&str> = labels.split('.').rev().collect();
    (
        instance.hosted_zone_id.clone(),
        format!("{}.", reversed.join(".")),
        instance.traffic_policy_type.clone(),
    )
}

/// `created` in the order the listing has to report them in.
fn in_listing_order(created: &[InstanceView]) -> Vec<InstanceView> {
    let mut ordered = created.to_vec();
    ordered.sort_by_key(listing_key);
    ordered
}

// ---------------------------------------------------------------------------
// The population
// ---------------------------------------------------------------------------

/// One row of the population: which of the test's zones the instance lives in,
/// the DNS name its record sets answer for, their record type and TTL.
struct InstanceRow {
    case: &'static str,
    /// Index into the zones the test created.
    zone: usize,
    name: &'static str,
    /// The canonical form of that name, which is what a response reports.
    expect_name: &'static str,
    rr_type: &'static str,
    ttl: i64,
}

/// The zone names the population lives across.
const ZONE_NAMES: [&str; 2] = ["example.com", "example.net"];

/// The population every test in this suite lists: instances across two hosted
/// zones, at names of differing depth and spelling, of differing record types —
/// including two that differ ONLY in that type — and with differing TTLs.
fn population() -> Vec<InstanceRow> {
    vec![
        InstanceRow {
            case: "an A instance at a subdomain of the first zone",
            zone: 0,
            name: "www.example.com",
            expect_name: "www.example.com.",
            rr_type: "A",
            ttl: 60,
        },
        InstanceRow {
            case: "an AAAA instance at that same name, told apart by its type",
            zone: 0,
            name: "WWW.Example.com",
            expect_name: "www.example.com.",
            rr_type: "AAAA",
            ttl: 0,
        },
        InstanceRow {
            case: "a CNAME instance earlier in the zone's DNS order",
            zone: 0,
            name: "alpha.example.com.",
            expect_name: "alpha.example.com.",
            rr_type: "CNAME",
            ttl: 86_400,
        },
        InstanceRow {
            case: "a TXT instance two labels deep",
            zone: 0,
            name: "deep.sub.example.com",
            expect_name: "deep.sub.example.com.",
            rr_type: "TXT",
            ttl: 300,
        },
        InstanceRow {
            case: "an A instance in the second zone",
            zone: 1,
            name: "www.example.net",
            expect_name: "www.example.net.",
            rr_type: "A",
            ttl: 120,
        },
        InstanceRow {
            case: "an MX instance in the second zone",
            zone: 1,
            name: "mail.example.net",
            expect_name: "mail.example.net.",
            rr_type: "MX",
            ttl: 900,
        },
    ]
}

/// A value one record set of `rr_type` can carry, so each row's policy document
/// is one the service accepts.
fn value_for(rr_type: &str) -> &'static str {
    match rr_type {
        "A" => "192.0.2.1",
        "AAAA" => "2001:db8::1",
        "CNAME" => "target.example.org",
        "MX" => "10 mail.example.org",
        "TXT" => "v=spf1 ~all",
        _ => "192.0.2.1",
    }
}

/// Create the two zones every test lists across, and return their bare ids.
async fn create_zones(router: &MultiServiceRouter, suffix: &str) -> Vec<String> {
    let mut zones = Vec::new();
    for (index, name) in ZONE_NAMES.iter().enumerate() {
        zones.push(create_zone_ok(router, name, &format!("{suffix}-{index}")).await);
    }
    zones
}

/// Create the instance `row` describes and return what every listing of the
/// account must then report for it.
async fn create_row(
    router: &MultiServiceRouter,
    zones: &[String],
    row: &InstanceRow,
    nonce: usize,
) -> InstanceView {
    let created = create_policy(
        router,
        &format!("instance-listing-policy-{nonce}"),
        &document_with_start_endpoint(row.rr_type, value_for(row.rr_type)),
        Some(row.case),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case {:?}: expected 201 from CreateTrafficPolicy, got {} with body {}",
        row.case, created.status, created.body
    );
    let policy = policy_of(&created, row.case).id;

    let zone = &zones[row.zone];
    let instance = create_instance(router, zone, row.name, row.ttl, &policy, 1).await;
    assert_eq!(
        instance.status, 201,
        "case {:?}: expected 201 from CreateTrafficPolicyInstance, got {} with body {}",
        row.case, instance.status, instance.body
    );
    let answered = instances_of(&instance, row.case);
    assert_eq!(
        answered.len(),
        1,
        "case {:?}: a create answers with exactly one instance; body {}",
        row.case,
        instance.body
    );
    let held = answered[0].clone();
    assert_eq!(
        (held.hosted_zone_id.as_str(), held.name.as_str()),
        (zone.as_str(), row.expect_name),
        "case {:?}: the create must persist the instance in the zone and at the canonical \
         name it was asked for, got {held:?}; body {}",
        row.case,
        instance.body
    );
    held
}

/// Create every row of [`population`], in order, and return what the listing
/// must report for them, in the order the listing reports them in.
async fn populate(router: &MultiServiceRouter, zones: &[String]) -> Vec<InstanceView> {
    let mut created = Vec::new();
    for (nonce, row) in population().iter().enumerate() {
        created.push(create_row(router, zones, row, nonce).await);
    }
    in_listing_order(&created)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_listing_reports_every_instance_the_account_holds() {
    // The population is the varying axis: none, then one more instance after
    // every row — so the listing is asserted against a set that grows by an
    // instance in another zone, at another name or of another type each time.
    // Every row's record type is one the model's own `RRType` enum names.
    for row in &population() {
        assert!(
            R_R_TYPE_VALUES.contains(&row.rr_type),
            "case {:?}: {} is not a modelled RRType",
            row.case,
            row.rr_type
        );
    }

    let router = router();
    let zones = create_zones(&router, "instance-listing").await;
    let mut created: Vec<InstanceView> = Vec::new();

    for (nonce, row) in population().iter().enumerate() {
        let case = format!(
            "{} instance(s), before creating {}",
            created.len(),
            row.case
        );
        let reply = listing(&router, &case, "").await;
        let listed = instances_of(&reply, &case);
        assert_eq!(
            listed,
            in_listing_order(&created),
            "case {case:?}: the listing must report every instance the account holds, by \
             hosted zone, then DNS name, then record type; body {}",
            reply.body
        );
        assert_ne!(
            reply.first("IsTruncated"),
            Some("true"),
            "case {case:?}: a listing that fits one page must not be truncated; body {}",
            reply.body
        );
        assert_eq!(
            next_markers(&reply),
            None,
            "case {case:?}: an untruncated listing names no instance to resume at; body {}",
            reply.body
        );
        assert_eq!(
            reply.first("MaxItems"),
            Some(DEFAULT_PAGE_SIZE.to_string()).as_deref(),
            "case {case:?}: a request naming no maxitems asks for the default page; body {}",
            reply.body
        );

        // Every listed instance is cross-checked against what
        // GetTrafficPolicyInstance reports for the same id, so the listing is
        // proved against another reader of the same state.
        for (at, instance) in listed.iter().enumerate() {
            let read = get_instance(&router, &instance.id).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: the listed instance {} must be readable, got {} with body {}",
                instance.id, read.status, read.body
            );
            let reported = instances_of(&read, &case);
            assert_eq!(
                reported.first(),
                Some(instance),
                "case {case:?}: instance {at} must be listed as GetTrafficPolicyInstance \
                 reports it; body {}",
                read.body
            );
            let state = read.first("TrafficPolicyInstance/State").unwrap_or("");
            assert!(
                [
                    TRAFFIC_POLICY_INSTANCE_CREATING,
                    TRAFFIC_POLICY_INSTANCE_APPLIED
                ]
                .contains(&state),
                "case {case:?}: instance {at} reported the state {state:?}; body {}",
                read.body
            );
        }

        created.push(create_row(&router, &zones, row, nonce).await);
    }
}

#[tokio::test]
async fn traffic_policy_instance_paging_covers_the_listing_exactly_once() {
    // The rule as an invariant: a page holds at most MaxItems instances and the
    // three markers name where the next page starts, so the pages concatenate
    // to the full listing for EVERY page size. The page size is the varying
    // axis.
    let router = router();
    let zones = create_zones(&router, "instance-listing-paging").await;
    let all = populate(&router, &zones).await;

    let unpaged = listing(&router, "the whole listing", "").await;
    assert_eq!(
        instances_of(&unpaged, "the whole listing"),
        all,
        "the whole listing must report every created instance in listing order; body {}",
        unpaged.body
    );

    for page_size in 1..=all.len() + 1 {
        let mut walked: Vec<InstanceView> = Vec::new();
        let mut resume: Option<InstanceView> = None;
        let mut pages = 0;
        loop {
            let case = format!("maxitems={page_size}, resuming at {resume:?}");
            let query = match &resume {
                Some(at) => marker_query(at, Some(page_size)),
                None => format!("?maxitems={page_size}"),
            };
            let page = listing(&router, &case, &query).await;
            assert_eq!(
                page.first("MaxItems"),
                Some(page_size.to_string()).as_deref(),
                "case {case:?}: the response must echo the request's maxitems; body {}",
                page.body
            );
            let listed = instances_of(&page, &case);
            assert!(
                listed.len() <= page_size,
                "case {case:?}: a page held {} instance(s); body {}",
                listed.len(),
                page.body
            );
            walked.extend(listed);
            pages += 1;
            assert!(
                pages <= all.len() + 2,
                "case {case:?}: paging did not terminate"
            );
            if page.first("IsTruncated") != Some("true") {
                assert_eq!(
                    next_markers(&page),
                    None,
                    "case {case:?}: a final page names no instance to resume at; body {}",
                    page.body
                );
                break;
            }
            let markers = next_markers(&page).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a truncated page must name the instance the next page \
                     starts at; body {}",
                    page.body
                )
            });
            let next = all.get(walked.len()).unwrap_or_else(|| {
                panic!("case {case:?}: a truncated page must leave an instance to resume at")
            });
            assert_eq!(
                markers,
                (
                    next.hosted_zone_id.clone(),
                    next.name.clone(),
                    next.traffic_policy_type.clone()
                ),
                "case {case:?}: the markers must name the first instance of the next page; \
                 body {}",
                page.body
            );
            resume = Some(next.clone());
        }
        assert_eq!(
            walked, all,
            "case maxitems={page_size}: paging must cover the listing exactly once"
        );
    }
}

#[tokio::test]
async fn the_markers_resume_the_listing_at_the_instance_they_name() {
    // The markers name "the first traffic policy instance in the next group", so
    // a listing resumed at any position reports exactly the tail from that
    // position. Every position is a row; so is each component of the triple on
    // its own, which positions the first instance carrying it.
    let router = router();
    let zones = create_zones(&router, "instance-listing-markers").await;
    let all = populate(&router, &zones).await;
    assert!(
        all.len() > 1,
        "the suite needs several instances to resume at, got {}",
        all.len()
    );

    for (at, instance) in all.iter().enumerate() {
        let case = format!("the full marker triple of instance {at}");
        let page = listing(&router, &case, &marker_query(instance, None)).await;
        assert_eq!(
            instances_of(&page, &case),
            all[at..].to_vec(),
            "case {case:?}: the listing must resume at the instance the markers name; body {}",
            page.body
        );
    }

    // One component of the triple on its own names the FIRST instance carrying
    // it, which is where the listing then resumes.
    let partial = [
        ("the hosted zone alone", "hostedzoneid", 0),
        ("the DNS name alone", "trafficpolicyinstancename", 1),
        ("the record type alone", "trafficpolicyinstancetype", 2),
    ];
    for (case, parameter, component) in partial {
        for instance in &all {
            let value = match component {
                0 => instance.hosted_zone_id.clone(),
                1 => instance.name.clone(),
                _ => instance.traffic_policy_type.clone(),
            };
            let case = format!("{case}: {parameter}={value}");
            let page = listing(&router, &case, &format!("?{parameter}={value}")).await;
            let expected_at = match component {
                0 => all
                    .iter()
                    .position(|held| held.hosted_zone_id == value)
                    .expect("the value came from an instance of the listing"),
                1 => all
                    .iter()
                    .position(|held| held.name == value)
                    .expect("the value came from an instance of the listing"),
                _ => all
                    .iter()
                    .position(|held| held.traffic_policy_type == value)
                    .expect("the value came from an instance of the listing"),
            };
            assert_eq!(
                instances_of(&page, &case),
                all[expected_at..].to_vec(),
                "case {case:?}: the listing must resume at the first instance the marker \
                 names; body {}",
                page.body
            );
        }
    }
}

#[tokio::test]
async fn markers_naming_no_instance_of_the_account_are_refused() {
    // Markers position the listing AT an instance, so markers the account holds
    // no instance under name no position in the listing: the request is refused
    // rather than answered from somewhere else. The rows are a marker triple
    // whose zone, whose name and whose type is one no instance carries, plus
    // three parts that each name an instance but never the same one.
    let router = router();
    let zones = create_zones(&router, "instance-listing-unknown").await;
    let all = populate(&router, &zones).await;
    // An instance picked by WHAT IT IS rather than by its place in the listing,
    // which depends on the ids the zones were minted with: the A instance at
    // `www.example.com.`, the name a second instance carries at another type and
    // a third in another zone.
    let held = all
        .iter()
        .find(|instance| instance.name == "www.example.com." && instance.traffic_policy_type == "A")
        .expect("the population holds an A instance at www.example.com.")
        .clone();

    let rows = [
        (
            "a hosted zone the account holds no instance in",
            format!(
                "?hostedzoneid=Z0000000000000&trafficpolicyinstancename={}\
                 &trafficpolicyinstancetype={}",
                held.name, held.traffic_policy_type
            ),
        ),
        (
            "a DNS name no instance answers for",
            format!(
                "?hostedzoneid={}&trafficpolicyinstancename=nobody.example.com.\
                 &trafficpolicyinstancetype={}",
                held.hosted_zone_id, held.traffic_policy_type
            ),
        ),
        (
            "a record type no instance of that name carries",
            format!(
                "?hostedzoneid={}&trafficpolicyinstancename={}&trafficpolicyinstancetype=SRV",
                held.hosted_zone_id, held.name
            ),
        ),
        (
            "a name and a type that belong to different instances",
            format!(
                "?hostedzoneid={}&trafficpolicyinstancename={}&trafficpolicyinstancetype=MX",
                held.hosted_zone_id, held.name
            ),
        ),
        (
            "a name alone that no instance answers for",
            "?trafficpolicyinstancename=nobody.example.org.".to_string(),
        ),
    ];

    for (case, query) in &rows {
        let reply = list_instances(&router, query).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "NoSuchTrafficPolicyInstance",
            "case {case:?}: expected the modelled NoSuchTrafficPolicyInstance, got {:?}; \
             body {}",
            reply.error_code(),
            reply.body
        );
    }

    // The refusals left the listing exactly as it was.
    let after = listing(&router, "after the refusals", "").await;
    assert_eq!(
        instances_of(&after, "after the refusals"),
        all,
        "a refused listing must leave the account's instances alone; body {}",
        after.body
    );
}

#[tokio::test]
async fn the_listing_echoes_the_page_size_it_was_asked_for() {
    // `MaxItems` in the response is "the value that you specified for the
    // MaxItems parameter", so it is echoed whatever the caller sent — while the
    // page itself never exceeds the 100 items this listing publishes. The rows
    // are sizes below the population, the published bound and one past it.
    let router = router();
    let zones = create_zones(&router, "instance-listing-page-size").await;
    let all = populate(&router, &zones).await;

    for requested in [1, 2, PAGE_BOUND, PAGE_BOUND + 1] {
        let case = format!("maxitems={requested}");
        let page = listing(&router, &case, &format!("?maxitems={requested}")).await;
        assert_eq!(
            page.first("MaxItems"),
            Some(requested.to_string()).as_deref(),
            "case {case:?}: expected the requested maxitems echoed, got {:?}; body {}",
            page.first("MaxItems"),
            page.body
        );
        let listed = instances_of(&page, &case);
        let wanted = (requested as usize).min(PAGE_BOUND as usize).min(all.len());
        assert_eq!(
            listed.len(),
            wanted,
            "case {case:?}: expected {wanted} instance(s), got {}; body {}",
            listed.len(),
            page.body
        );
        assert_eq!(
            page.first("IsTruncated") == Some("true"),
            listed.len() < all.len(),
            "case {case:?}: IsTruncated must say whether more instances remain; body {}",
            page.body
        );
    }
}

#[tokio::test]
async fn listing_the_instances_leaves_the_account_exactly_as_it_was() {
    // The rule is a report: repeating it answers the same listing, creates and
    // consumes nothing, and the population it reports still moves with a later
    // create.
    let router = router();
    let zones = create_zones(&router, "instance-listing-reads").await;
    let all = populate(&router, &zones).await;
    let before = instance_count(&router).await;

    for attempt in 1..=3 {
        let case = format!("listing {attempt}");
        assert_eq!(
            instances_of(&listing(&router, &case, "").await, &case),
            all,
            "case {case:?}: a read must report what the first read did"
        );
        assert_eq!(
            instance_count(&router).await,
            before,
            "case {case:?}: listing must not change how many instances the account holds"
        );
    }

    // A paged walk is a read as well: paging through the listing changes
    // neither the population nor what an unpaged read reports.
    let mut resume: Option<InstanceView> = None;
    loop {
        let case = format!("a paged read resuming at {resume:?}");
        let query = match &resume {
            Some(at) => marker_query(at, Some(1)),
            None => "?maxitems=1".to_string(),
        };
        let page = listing(&router, &case, &query).await;
        if page.first("IsTruncated") != Some("true") {
            break;
        }
        let markers = next_markers(&page).expect("a truncated page names the next instance");
        resume = all
            .iter()
            .find(|held| {
                (
                    held.hosted_zone_id.as_str(),
                    held.name.as_str(),
                    held.traffic_policy_type.as_str(),
                ) == (markers.0.as_str(), markers.1.as_str(), markers.2.as_str())
            })
            .cloned();
        assert!(
            resume.is_some(),
            "case {case:?}: the markers must name an instance the account holds; body {}",
            page.body
        );
    }
    assert_eq!(
        instance_count(&router).await,
        before,
        "paging through the listing must not change how many instances the account holds"
    );

    let added = create_row(
        &router,
        &zones,
        &InstanceRow {
            case: "an instance created after the listing",
            zone: 1,
            name: "late.example.net",
            expect_name: "late.example.net.",
            rr_type: "A",
            ttl: 45,
        },
        population().len(),
    )
    .await;
    let mut expected = all.clone();
    expected.push(added);
    let expected = in_listing_order(&expected);
    let case = "after a later create";
    assert_eq!(
        instances_of(&listing(&router, case, "").await, case),
        expected,
        "case {case:?}: the listing must follow the state the account is in"
    );
}
