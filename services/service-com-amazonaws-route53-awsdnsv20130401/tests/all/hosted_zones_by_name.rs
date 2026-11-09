// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListHostedZonesByName`.
//!
//! The rules under test, each over a TABLE that spans the class it generalises
//! over rather than the one name that exposed it:
//!
//! 1. The operation REPORTS the hosted zones in state, ordered by name with the
//!    labels reversed and the trailing root dot included, so the order is the
//!    byte order of `com.example.www.` and not of `www.example.com`. Two zones
//!    carrying the same name are ordered by hosted zone id, which makes the
//!    order total.
//! 2. `dnsname` positions the page: the listing begins at the first zone whose
//!    reversed-label key is greater than or equal to the requested name's,
//!    whatever spelling (case, trailing dot) the name is sent in, and the
//!    request's `dnsname`/`hostedzoneid` come back echoed.
//! 3. A page carries at most `MaxItems` zones; `NextDNSName` and
//!    `NextHostedZoneId` name the first zone the page left out, and sending
//!    them back as `dnsname`/`hostedzoneid` resumes exactly there — so walking
//!    the pages yields every zone once, in the same order, at any page size.
//!    `MaxItems` is echoed as the caller sent it.
//! 4. A `dnsname` that is no domain name is `InvalidDomainName` (400), and a
//!    `hostedzoneid` with no `dnsname` beside it positions nothing, so it is
//!    `InvalidInput` (400).
//!
//! The tables are driven over page sizes and start names rather than through a
//! generator crate because the fixture's zone set is small enough that the
//! rows below cover every page boundary the walk can land on.
//!
//! Every test drives the service through the real router, using the same
//! harness the sibling suites use, so what is asserted is the HTTP/restXml
//! behaviour a Route 53 client sees.

use std::collections::HashMap;

use smithy_server_runtime::MultiServiceRouter;

use crate::hosted_zone_records::{call, create_zone_body, router, Reply};

/// The fixture zone names, in the (deliberately scrambled) order they are
/// created in — so nothing below can pass by reporting creation order.
///
/// The set spans what the ordering rule generalises over: several zones under
/// one parent, zones under different top-level labels, a longer name whose
/// top-level label sorts first (`…com.au`), a name whose label ends in a
/// character that sorts BEFORE the dot (`example-x`, hyphen = 45, dot = 46), a
/// Punycode label, and a name sent in upper case.
const CREATE_ORDER: &[&str] = &[
    "www.example.com",
    "example.net",
    "example.com",
    "EXAMPLE.ORG",
    "zebra.example.com",
    "example-x.com",
    "xn--caf-dma.com",
    "alpha.example.com",
    "example.com.au",
    "my-zone.example.com",
];

/// The canonical names of the fixture zones in the ONE order
/// `ListHostedZonesByName` must report them: sorted by the reversed-label form
/// of the name, `au.com.example.` through `org.example.`.
///
/// Note `example-x.com.` (`com.example-x.`) BEFORE `example.com.`
/// (`com.example.`): the hyphen sorts before the dot, which is the documented
/// effect of the trailing dot on the sort order.
const EXPECTED_ORDER: &[&str] = &[
    "example.com.au.",
    "example-x.com.",
    "example.com.",
    "alpha.example.com.",
    "my-zone.example.com.",
    "www.example.com.",
    "zebra.example.com.",
    "xn--caf-dma.com.",
    "example.net.",
    "example.org.",
];

/// Create every fixture zone; returns `canonical name -> bare zone id`.
async fn seed(router: &MultiServiceRouter) -> HashMap<String, String> {
    let mut created = HashMap::new();
    for (index, name) in CREATE_ORDER.iter().enumerate() {
        let reply = call(
            router,
            "POST",
            "/2013-04-01/hostedzone",
            &create_zone_body(name, &format!("by-name-ref-{index}"), ""),
        )
        .await;
        assert_eq!(
            reply.status, 201,
            "creating {name}: expected 201 from CreateHostedZone, got {} with body {}",
            reply.status, reply.body
        );
        let canonical = reply
            .first("HostedZone/Name")
            .unwrap_or_else(|| {
                panic!(
                    "creating {name}: the reply must carry HostedZone/Name; body {}",
                    reply.body
                )
            })
            .to_string();
        let id = bare_id(&reply);
        assert!(
            created.insert(canonical.clone(), id).is_none(),
            "the fixture must not create {canonical} twice"
        );
    }
    created
}

/// The bare (`Z…`) id from a reply carrying one `HostedZone`.
fn bare_id(reply: &Reply) -> String {
    reply
        .first("HostedZone/Id")
        .unwrap_or_else(|| panic!("the reply must carry HostedZone/Id; body {}", reply.body))
        .trim_start_matches("/hostedzone/")
        .to_string()
}

/// One `ListHostedZonesByName` request.
///
/// `pub(crate)` so the sibling suite asserting the domain-name refusal sends the
/// same request this one does.
pub(crate) async fn list_by_name(
    router: &MultiServiceRouter,
    dns_name: Option<&str>,
    hosted_zone_id: Option<&str>,
    max_items: Option<i32>,
) -> Reply {
    let mut uri = "/2013-04-01/hostedzonesbyname".to_string();
    let mut separator = '?';
    for (key, value) in [
        ("dnsname", dns_name.map(str::to_string)),
        ("hostedzoneid", hosted_zone_id.map(str::to_string)),
        ("maxitems", max_items.map(|items| items.to_string())),
    ] {
        if let Some(value) = value {
            uri.push(separator);
            uri.push_str(&format!("{key}={value}"));
            separator = '&';
        }
    }
    call(router, "GET", &uri, "").await
}

/// The `(Name, Id)` of every hosted zone in a reply, in document order.
fn listed(reply: &Reply) -> Vec<(String, String)> {
    let names = reply.all("HostedZone/Name");
    let ids = reply.all("HostedZone/Id");
    assert_eq!(
        names.len(),
        ids.len(),
        "every HostedZone must carry both Name and Id; body {}",
        reply.body
    );
    names
        .into_iter()
        .zip(ids)
        .map(|(name, id)| (name.to_string(), id.to_string()))
        .collect()
}

/// The `(Name, Id)` pairs the fixture must report, for `EXPECTED_ORDER[from..]`.
fn expected_from(created: &HashMap<String, String>, from: usize) -> Vec<(String, String)> {
    EXPECTED_ORDER[from..]
        .iter()
        .map(|name| {
            let id = created
                .get(*name)
                .unwrap_or_else(|| panic!("the fixture zone {name} must have been created"));
            ((*name).to_string(), format!("/hostedzone/{id}"))
        })
        .collect()
}

/// The text of `element`, or the empty string when the response omits it — the
/// form the assertions below compare, so an absent element and an empty one are
/// one case.
fn text(reply: &Reply, element: &str) -> String {
    reply.first(element).unwrap_or("").to_string()
}

// ---------------------------------------------------------------------------
// Rule 1: the order the zones come back in
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_listing_is_ordered_by_name_with_the_labels_reversed() {
    let router = router();
    let created = seed(&router).await;

    let reply = list_by_name(&router, None, None, None).await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from ListHostedZonesByName, got {} with body {}",
        reply.status, reply.body
    );
    let actual = listed(&reply);
    let expected = expected_from(&created, 0);
    assert_eq!(
        actual, expected,
        "the listing must be in reversed-label name order; expected {expected:?}, \
         got {actual:?}; body {}",
        reply.body
    );
    assert_eq!(
        text(&reply, "IsTruncated"),
        "false",
        "a listing that fits in one page is not truncated; body {}",
        reply.body
    );
    assert_eq!(
        text(&reply, "MaxItems"),
        "100",
        "expected the default MaxItems to be reported, got {:?}; body {}",
        reply.first("MaxItems"),
        reply.body
    );
    for element in ["DNSName", "HostedZoneId", "NextDNSName", "NextHostedZoneId"] {
        assert!(
            text(&reply, element).is_empty(),
            "a first, untruncated listing carries no {element}, got {:?}; body {}",
            reply.first(element),
            reply.body
        );
    }
}

#[tokio::test]
async fn zones_sharing_a_name_are_ordered_by_hosted_zone_id() {
    // Route 53 keys a zone by the caller reference, not by the name, so one
    // name may be held by several zones; the id is what orders them, and what
    // lets the next page resume between two zones of the same name.
    let router = router();
    let mut ids = Vec::new();
    for reference in ["dup-a", "dup-b", "dup-c"] {
        let reply = call(
            &router,
            "POST",
            "/2013-04-01/hostedzone",
            &create_zone_body("shared.example.com", reference, ""),
        )
        .await;
        assert_eq!(
            reply.status, 201,
            "creating the zone for {reference}: expected 201, got {} with body {}",
            reply.status, reply.body
        );
        ids.push(bare_id(&reply));
    }
    ids.sort();
    let expected: Vec<(String, String)> = ids
        .iter()
        .map(|id| ("shared.example.com.".to_string(), format!("/hostedzone/{id}")))
        .collect();

    let reply = list_by_name(&router, None, None, None).await;
    let actual = listed(&reply);
    assert_eq!(
        actual, expected,
        "zones sharing a name must come back in hosted-zone-id order; expected \
         {expected:?}, got {actual:?}; body {}",
        reply.body
    );

    // One zone per page: the resume point has to name BOTH the name and the id,
    // or the walk would either repeat or skip the zones sharing the name.
    let mut collected: Vec<(String, String)> = Vec::new();
    let mut resume: Option<(String, String)> = None;
    for step in 0..=expected.len() {
        let reply = list_by_name(
            &router,
            resume.as_ref().map(|(name, _)| name.as_str()),
            resume.as_ref().map(|(_, id)| id.as_str()),
            Some(1),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "step {step}: expected 200, got {} with body {}",
            reply.status, reply.body
        );
        collected.extend(listed(&reply));
        let next_name = text(&reply, "NextDNSName");
        let next_id = text(&reply, "NextHostedZoneId");
        if next_name.is_empty() {
            assert_eq!(
                text(&reply, "IsTruncated"),
                "false",
                "step {step}: a page with no NextDNSName is not truncated; body {}",
                reply.body
            );
            break;
        }
        assert_eq!(
            text(&reply, "IsTruncated"),
            "true",
            "step {step}: a page with a NextDNSName is truncated; body {}",
            reply.body
        );
        resume = Some((next_name, next_id));
    }
    assert_eq!(
        collected, expected,
        "walking one zone at a time must yield each same-named zone exactly once; \
         got {collected:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: dnsname positions the page
// ---------------------------------------------------------------------------

/// One start-name row: the `dnsname` asked for, and the index in
/// [`EXPECTED_ORDER`] the listing must begin at.
struct StartRow {
    case: &'static str,
    dns_name: &'static str,
    from: usize,
}

#[tokio::test]
async fn dnsname_starts_the_listing_at_the_first_zone_at_or_after_it() {
    let rows = [
        StartRow {
            case: "the name of a zone in the middle of the order",
            dns_name: "example.com",
            from: 2,
        },
        StartRow {
            case: "the same name spelled in upper case with the root dot",
            dns_name: "EXAMPLE.COM.",
            from: 2,
        },
        StartRow {
            case: "the name of the last zone in the order",
            dns_name: "example.org",
            from: 9,
        },
        StartRow {
            case: "a name no zone carries, between two zones",
            dns_name: "b.example.com",
            from: 4,
        },
        StartRow {
            case: "a single label sorting before every zone",
            dns_name: "a",
            from: 0,
        },
        StartRow {
            case: "a name past every zone in the order",
            dns_name: "zz.example.org",
            from: EXPECTED_ORDER.len(),
        },
    ];

    let router = router();
    let created = seed(&router).await;

    for row in &rows {
        let reply = list_by_name(&router, Some(row.dns_name), None, None).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200, got {} with body {}",
            row.case, reply.status, reply.body
        );
        let actual = listed(&reply);
        let expected = expected_from(&created, row.from);
        assert_eq!(
            actual, expected,
            "case {:?}: expected the listing {expected:?}, got {actual:?}; body {}",
            row.case, reply.body
        );
        // The request's dnsname comes back exactly as it was sent.
        assert_eq!(
            text(&reply, "DNSName"),
            row.dns_name,
            "case {:?}: expected the DNSName {:?} echoed, got {:?}; body {}",
            row.case,
            row.dns_name,
            reply.first("DNSName"),
            reply.body
        );
    }
}

#[tokio::test]
async fn an_empty_dnsname_lists_from_the_first_zone() {
    // `DNSName` is `len 0..1024`, so an empty one satisfies every modelled
    // constraint; it names no position, exactly as omitting it does.
    let router = router();
    let created = seed(&router).await;
    let reply = list_by_name(&router, Some(""), None, None).await;
    assert_eq!(
        reply.status, 200,
        "expected 200 for an empty dnsname, got {} with body {}",
        reply.status, reply.body
    );
    let actual = listed(&reply);
    let expected = expected_from(&created, 0);
    assert_eq!(
        actual, expected,
        "an empty dnsname must list every zone from the first; expected {expected:?}, \
         got {actual:?}; body {}",
        reply.body
    );
}

#[tokio::test]
async fn the_listing_reports_the_zone_a_create_just_wrote() {
    // The round trip: the zone CreateHostedZone wrote is the one GetHostedZone
    // reports AND the one this listing positions `dnsname` at.
    let router = router();
    let _created = seed(&router).await;
    let made = call(
        &router,
        "POST",
        "/2013-04-01/hostedzone",
        &create_zone_body("Round-Trip.Example.Com", "by-name-round-trip", ""),
    )
    .await;
    assert_eq!(
        made.status, 201,
        "expected 201 from CreateHostedZone, got {} with body {}",
        made.status, made.body
    );
    let id = bare_id(&made);

    let read = call(&router, "GET", &format!("/2013-04-01/hostedzone/{id}"), "").await;
    assert_eq!(
        read.status, 200,
        "expected 200 from GetHostedZone, got {} with body {}",
        read.status, read.body
    );
    assert_eq!(
        read.first("HostedZone/Name"),
        Some("round-trip.example.com."),
        "GetHostedZone must report the zone's canonical name; body {}",
        read.body
    );

    let reply = list_by_name(&router, Some("round-trip.example.com"), None, Some(1)).await;
    assert_eq!(
        listed(&reply),
        vec![(
            "round-trip.example.com.".to_string(),
            format!("/hostedzone/{id}")
        )],
        "the listing must start at the zone GetHostedZone just reported; body {}",
        reply.body
    );
    assert_eq!(
        reply.first("HostedZone/CallerReference"),
        Some("by-name-round-trip"),
        "the listed zone must carry the caller reference it was created with; body {}",
        reply.body
    );
}

// ---------------------------------------------------------------------------
// Rule 3: pagination
// ---------------------------------------------------------------------------

#[tokio::test]
async fn walking_the_pages_yields_every_zone_once_at_any_page_size() {
    // The page sizes span 1, sizes that divide the zone count evenly and
    // unevenly, the exact count, one past it, and one past the documented
    // maximum: `MaxItems` is an Integer with no modelled range, and Route 53
    // "returns up to 100 items in each response", so 101 is one past that
    // bound and still answers a full listing.
    let page_sizes = [
        None,
        Some(1),
        Some(2),
        Some(3),
        Some(4),
        Some(5),
        Some(9),
        Some(10),
        Some(11),
        Some(101),
    ];

    let router = router();
    let created = seed(&router).await;
    let expected = expected_from(&created, 0);

    for max_items in page_sizes {
        let mut collected: Vec<(String, String)> = Vec::new();
        let mut resume: Option<(String, String)> = None;
        let mut pages = 0usize;
        loop {
            let reply = list_by_name(
                &router,
                resume.as_ref().map(|(name, _)| name.as_str()),
                resume.as_ref().map(|(_, id)| id.as_str()),
                max_items,
            )
            .await;
            assert_eq!(
                reply.status, 200,
                "case {max_items:?}: expected 200, got {} with body {}",
                reply.status, reply.body
            );
            // The caller's own MaxItems is echoed back.
            assert_eq!(
                text(&reply, "MaxItems"),
                max_items.unwrap_or(100).to_string(),
                "case {max_items:?}: expected MaxItems {:?} echoed, got {:?}; body {}",
                max_items.unwrap_or(100),
                reply.first("MaxItems"),
                reply.body
            );

            let page = listed(&reply);
            let cap = max_items.unwrap_or(100).clamp(1, 100) as usize;
            assert!(
                page.len() <= cap,
                "case {max_items:?}: a page carries at most {cap} zones, got {}; body {}",
                page.len(),
                reply.body
            );
            collected.extend(page);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {max_items:?}: the walk must terminate, got {pages} pages; body {}",
                reply.body
            );

            let next_name = text(&reply, "NextDNSName");
            let next_id = text(&reply, "NextHostedZoneId");
            if next_name.is_empty() {
                assert_eq!(
                    text(&reply, "IsTruncated"),
                    "false",
                    "case {max_items:?}: a page with no NextDNSName is not truncated; body {}",
                    reply.body
                );
                assert!(
                    next_id.is_empty(),
                    "case {max_items:?}: an untruncated page carries no NextHostedZoneId, \
                     got {next_id:?}; body {}",
                    reply.body
                );
                break;
            }
            // The resume point names the first zone the page left out — the
            // name canonically, the id BARE (no `/hostedzone/` prefix).
            let (expected_name, expected_id) = &expected[collected.len()];
            assert_eq!(
                (next_name.as_str(), next_id.as_str()),
                (
                    expected_name.as_str(),
                    expected_id.trim_start_matches("/hostedzone/")
                ),
                "case {max_items:?}: the resume point must name the next zone to return; \
                 body {}",
                reply.body
            );
            resume = Some((next_name, next_id));
        }

        assert_eq!(
            collected, expected,
            "case {max_items:?}: walking the pages must yield every zone once, in \
             reversed-label order; got {collected:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: refusals
// ---------------------------------------------------------------------------

/// A DNS label of `octets` `a` characters. Every octet is one character, so the
/// label's length in characters is its length in octets.
fn label(octets: usize) -> String {
    "a".repeat(octets)
}

/// One refused request: what makes it bad, and the modelled refusal it earns.
struct RefusalRow {
    case: &'static str,
    dns_name: Option<String>,
    hosted_zone_id: Option<String>,
    code: &'static str,
}

#[tokio::test]
async fn a_bad_dnsname_or_a_lone_hostedzoneid_is_refused() {
    let router = router();
    let created = seed(&router).await;
    let real_id = created
        .get("example.com.")
        .expect("the fixture zone example.com. must have been created")
        .clone();

    // The published DNS bounds (the DNS domain name format the `DNSName`
    // member's documentation points at): a label carries at most 63 octets, and
    // a whole name at most 255 including the dots. Each is tested AT the bound
    // (accepted, in the test below) and one octet past it (refused, here). The
    // modelled `@length` on `DNSName` is 0..1024, so every name below is well
    // within the field and the handler is what judges it.
    let rows = [
        RefusalRow {
            case: "a label one octet past the published maximum",
            dns_name: Some(format!("{}.example.com", label(64))),
            hosted_zone_id: None,
            code: "InvalidDomainName",
        },
        RefusalRow {
            case: "a name one octet past the published maximum",
            dns_name: Some([label(63), label(63), label(63), label(63)].join(".")),
            hosted_zone_id: None,
            code: "InvalidDomainName",
        },
        RefusalRow {
            case: "a hostedzoneid naming a real zone, with no dnsname",
            dns_name: None,
            hosted_zone_id: Some(real_id),
            code: "InvalidInput",
        },
        RefusalRow {
            case: "a hostedzoneid naming no zone, with no dnsname",
            dns_name: None,
            hosted_zone_id: Some("ZZZZZZZZZZZZZZZ".to_string()),
            code: "InvalidInput",
        },
        RefusalRow {
            case: "a hostedzoneid with an EMPTY dnsname beside it",
            dns_name: Some(String::new()),
            hosted_zone_id: Some("ZZZZZZZZZZZZZZZ".to_string()),
            code: "InvalidInput",
        },
    ];

    for row in &rows {
        let reply = list_by_name(
            &router,
            row.dns_name.as_deref(),
            row.hosted_zone_id.as_deref(),
            None,
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

#[tokio::test]
async fn a_dnsname_at_the_published_dns_bounds_is_accepted() {
    // The other side of each bound: a 63-octet label, and a name of exactly 255
    // octets counting the dot after every label (63 + 63 + 63 + 62 + 4 dots).
    let longest_name = [label(63), label(63), label(63), label(62)].join(".");
    assert_eq!(
        longest_name.len() + 1,
        255,
        "the fixture name must be exactly the published maximum, is {}",
        longest_name.len() + 1
    );
    let rows = [
        (
            // Reversed, this is `com.example.aaa…`, which sorts after
            // `com.example.` and before `com.example.alpha.`.
            "a label at the published maximum",
            format!("{}.example.com", label(63)),
            3,
        ),
        (
            // Reversed, this starts with `aaa…`, which sorts before every
            // fixture zone.
            "a name at the published maximum",
            longest_name,
            0,
        ),
    ];

    let router = router();
    let created = seed(&router).await;

    for (case, dns_name, from) in &rows {
        let reply = list_by_name(&router, Some(dns_name), None, None).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} with body {}",
            reply.status, reply.body
        );
        let actual = listed(&reply);
        let expected = expected_from(&created, *from);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the listing {expected:?}, got {actual:?}; body {}",
            reply.body
        );
    }
}
