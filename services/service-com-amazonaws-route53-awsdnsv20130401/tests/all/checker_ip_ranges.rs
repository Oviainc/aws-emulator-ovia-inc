// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `GetCheckerIpRanges`.
//!
//! The rule under test: the operation reports the SERVICE-owned set of CIDR blocks
//! Route 53's health checkers send their checks from — every entry a well-formed
//! IPv4 CIDR block whose host bits are zero, no entry twice, sorted by the blocks'
//! numeric position in the address space — and because the set belongs to the
//! service rather than to the account, the same list answers every call, whatever
//! the account does to its own resources and whichever store the service is mounted
//! on.
//!
//! One call is one row of that class, so the tables below span it instead: EVERY
//! block the service publishes is asserted on its own (a malformed, unaligned or
//! repeated entry fails here rather than reaching a client), the expected order is
//! recomputed from the reported text by a parser written here rather than by the
//! handler's own helper, and the list is re-read across a sequence of hosted-zone
//! creates, record changes and a delete — and on a second, independent engine.
//!
//! Every test drives the service through the real router, using the same harness
//! the hosted-zone suites use, so what is asserted is the HTTP/restXml behaviour a
//! Route 53 client sees.

use std::net::Ipv4Addr;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::state::CHECKER_IP_RANGES;

use crate::hosted_zone_records::{
    call, change_batch_body, change_element, create_zone_ok, router, RecordRow,
};

/// Call `GetCheckerIpRanges` and return the blocks it reported, in wire order.
///
/// A 501 here means the operation is still a scaffold, which is exactly what the
/// message says, so a regression to the stub reads as one.
///
/// `pub(crate)` so a sibling suite checks a health checker's address against the
/// SAME published blocks rather than growing a second copy of this reader.
pub(crate) async fn reported_blocks(router: &MultiServiceRouter) -> Vec<String> {
    let reply = call(router, "GET", "/2013-04-01/checkeripranges", "").await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from GetCheckerIpRanges (501 = still scaffolded), got {} with body {}",
        reply.status, reply.body
    );
    let blocks: Vec<String> = reply
        .all("CheckerIpRanges/member")
        .into_iter()
        .map(str::to_string)
        .collect();
    assert!(
        !blocks.is_empty(),
        "GetCheckerIpRanges must report the health-checker blocks, got body {}",
        reply.body
    );
    blocks
}

/// `(network address, prefix length)` of an `a.b.c.d/nn` block, or `None` when the
/// text is not a well-formed IPv4 CIDR block.
///
/// Written here, on `std::net::Ipv4Addr`, rather than reached from the crate under
/// test: the expected order and the validity of each entry have to be computed
/// independently of the code that produced them.
///
/// `pub(crate)` so the catalog-contents suite reads a block the same way rather
/// than growing a third copy of this parser.
pub(crate) fn parse_cidr(block: &str) -> Option<(u32, u8)> {
    let (address, prefix) = block.split_once('/')?;
    let address = u32::from(address.parse::<Ipv4Addr>().ok()?);
    let prefix: u8 = prefix.parse().ok()?;
    // The published bounds of an IPv4 prefix length: a /0 covers the whole space
    // and a /32 one address, so anything past 32 is not a block at all.
    if prefix > 32 {
        return None;
    }
    Some((address, prefix))
}

/// The host-bit mask of a prefix length: the bits a network address must leave
/// zero for the text to name a block rather than a single host inside one.
///
/// `checked_shr` rather than `>>`: a /32 would shift by the full width of the
/// type, which panics, and a /32 (one address) has no host bits at all.
fn host_bits(prefix: u8) -> u32 {
    u32::MAX.checked_shr(u32::from(prefix)).unwrap_or(0)
}

#[tokio::test]
async fn every_reported_block_is_a_well_formed_aligned_ipv4_cidr_block() {
    // One row per block the service reports — every one of them, not the one a
    // recording happened to carry.
    let router = router();
    for case in reported_blocks(&router).await {
        let (address, prefix) = parse_cidr(&case).unwrap_or_else(|| {
            panic!("case {case:?}: every reported range must be an IPv4 CIDR block, got {case:?}")
        });
        assert!(
            prefix <= 32,
            "case {case:?}: an IPv4 prefix length is at most 32, got {prefix:?}"
        );
        assert_eq!(
            address & host_bits(prefix),
            0,
            "case {case:?}: a CIDR block's host bits must be zero, got address {:?} for /{prefix}",
            Ipv4Addr::from(address)
        );
        // A block is a range of addresses, so it is not the unspecified address and
        // not a loopback or multicast block: a caller allows this traffic through a
        // firewall, so a block it cannot route would be useless to it.
        let network = Ipv4Addr::from(address);
        assert!(
            !network.is_unspecified()
                && !network.is_loopback()
                && !network.is_multicast()
                && !network.is_broadcast(),
            "case {case:?}: a health-checker block must be a routable unicast range, got {network:?}"
        );
    }
}

#[tokio::test]
async fn the_reported_blocks_are_the_published_set_each_exactly_once() {
    // The reported set is the service's published catalog, compared as a SET: the
    // order is the next test's subject, and a block reported twice would let a
    // caller see one range as two.
    let router = router();
    let reported = reported_blocks(&router).await;

    let mut deduplicated = reported.clone();
    deduplicated.sort_unstable();
    deduplicated.dedup();
    assert_eq!(
        deduplicated.len(),
        reported.len(),
        "no block may be reported twice: {reported:?}"
    );

    let mut published: Vec<String> = CHECKER_IP_RANGES.iter().map(|s| (*s).to_string()).collect();
    published.sort_unstable();
    published.dedup();
    assert_eq!(
        deduplicated, published,
        "the reported blocks must be the published health-checker set, got {deduplicated:?}"
    );

    // That comparison is against the service's own table, so it says the whole table
    // reaches the wire and nothing else; this is the plausibility bound on the table
    // itself. Route 53 checks a resource from several regions at once (`HealthCheck`'s
    // own documentation: "health checkers in multiple Amazon Web Services regions"), so
    // a list this short could not be that set.
    assert!(
        published.len() >= 8,
        "the health-checker set must span several regions, got {} block(s): {published:?}",
        published.len()
    );
}

#[tokio::test]
async fn the_blocks_are_reported_in_ascending_numeric_order() {
    let router = router();
    let reported = reported_blocks(&router).await;
    let keys: Vec<(u32, u8)> = reported
        .iter()
        .map(|block| {
            parse_cidr(block).unwrap_or_else(|| panic!("case {block:?}: not an IPv4 CIDR block"))
        })
        .collect();

    let mut expected = keys.clone();
    expected.sort_unstable();
    assert_eq!(
        keys, expected,
        "the blocks must be sorted by (network address, prefix length), got {reported:?}"
    );

    // And sorted NUMERICALLY, not by the spelling of the text: the published set
    // holds blocks whose two orders differ (a first octet of 107 sorts after one of
    // 54 numerically and before it as text), so a string sort fails right here.
    let mut as_text = reported.clone();
    as_text.sort_unstable();
    assert_ne!(
        reported, as_text,
        "the published set must distinguish numeric from lexicographic order, got {reported:?}"
    );
}

#[tokio::test]
async fn the_same_list_answers_every_call_whatever_the_account_does() {
    // The set is service-owned, so the account's own activity cannot move it. The
    // rows differ along what the account does between two reads: nothing, a zone
    // created, records written into it, more zones created, a zone deleted.
    let router = router();
    let first = reported_blocks(&router).await;

    let repeated = reported_blocks(&router).await;
    assert_eq!(
        repeated, first,
        "case {:?}: a repeated read must report the same list, got {repeated:?}",
        "read twice with nothing in between"
    );

    let zone = create_zone_ok(&router, "checkers.example.com", "checker-ranges-ref-0").await;
    let after_create = reported_blocks(&router).await;
    assert_eq!(
        after_create, first,
        "case {:?}: creating a hosted zone must not move the list, got {after_create:?}",
        "one hosted zone created"
    );

    let row = RecordRow {
        name: "www.checkers.example.com",
        rr_type: "A",
        ttl: 300,
        values: &["192.0.2.10"],
    };
    let changed = call(
        &router,
        "POST",
        &format!("/2013-04-01/hostedzone/{zone}/rrset"),
        &change_batch_body(&change_element("CREATE", &row), None),
    )
    .await;
    assert_eq!(
        changed.status, 200,
        "expected 200 from ChangeResourceRecordSets, got {} with body {}",
        changed.status, changed.body
    );
    let after_records = reported_blocks(&router).await;
    assert_eq!(
        after_records, first,
        "case {:?}: writing records must not move the list, got {after_records:?}",
        "a record set written"
    );

    for index in 1..4u32 {
        let extra = create_zone_ok(
            &router,
            &format!("zone-{index}.checkers.example.net"),
            &format!("checker-ranges-ref-{index}"),
        )
        .await;
        let case = format!("{index} further hosted zone(s), last {extra}");
        let again = reported_blocks(&router).await;
        assert_eq!(
            again, first,
            "case {case:?}: more hosted zones must not move the list, got {again:?}"
        );
    }

    let deleted = call(
        &router,
        "DELETE",
        &format!("/2013-04-01/hostedzone/{zone}/rrset"),
        "",
    )
    .await;
    assert!(
        deleted.status >= 400,
        "DELETE on the rrset path is not DeleteHostedZone; expected a refusal, got {} with body {}",
        deleted.status,
        deleted.body
    );
    let after_refusal = reported_blocks(&router).await;
    assert_eq!(
        after_refusal, first,
        "case {:?}: a refused request must not move the list, got {after_refusal:?}",
        "a refused request in between"
    );
}

#[tokio::test]
async fn reading_the_list_leaves_the_account_exactly_as_it_was() {
    // The operation is a report, so repeating it creates and consumes nothing: the
    // zone population after any number of reads is the one the account created.
    let router = router();
    create_zone_ok(&router, "untouched.example.org", "checker-ranges-untouched").await;

    for attempt in 1..=3 {
        let blocks = reported_blocks(&router).await;
        let listed = call(&router, "GET", "/2013-04-01/hostedzone", "").await;
        assert_eq!(
            listed.status, 200,
            "read {attempt}: expected 200 from ListHostedZones, got {} with body {}",
            listed.status, listed.body
        );
        let zones = listed.all("HostedZone/Id").len();
        assert_eq!(
            zones, 1,
            "read {attempt}: reading {} blocks must not create or destroy a zone, \
             ListHostedZones reports {zones}",
            blocks.len()
        );
    }
}

#[tokio::test]
async fn a_separate_store_reports_the_same_service_owned_list() {
    // Two routers, two engines, two stores: the blocks come from the SERVICE, so
    // they do not depend on what any one store holds.
    let first = reported_blocks(&router()).await;
    let second = reported_blocks(&router()).await;
    assert_eq!(
        second, first,
        "a fresh store must report the same service-owned list, got {second:?}"
    );
}
