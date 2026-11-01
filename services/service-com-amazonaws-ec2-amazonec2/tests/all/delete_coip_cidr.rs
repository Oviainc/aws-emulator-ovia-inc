// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteCoipCidr`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=DeleteCoipCidr`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that the operation TAKES ONE ADDRESS RANGE OUT OF THE STORE and
//! reports what it removed: the range `Cidr` names leaves the pool `CoipPoolId` names, the
//! response carries that range beside the pool's own id and local gateway route table, every
//! other range and every other pool is left alone, and a request that resolves to no range —
//! a pool id no pool of the account has, a `Cidr` that is no address range at all, or a range
//! that pool does not hold — is refused rather than answered as a delete that did nothing.
//!
//! Every table row is a distinct input along what that generalises over — which pool of the
//! seeded set, which position in its list of ranges, how many ranges that pool holds, which
//! spelling of the range's host bits the request uses, and each way a request can fail to
//! resolve a range — so no single recorded call is the specification. The pools come from the
//! `coip_pools` suite's own fixtures, and every range is read back through
//! `DescribeCoipPools`, so a delete is asserted against exactly what the describe reports.
//!
//! The echo ceiling a refused `Cidr` is named up to is this operation's own
//! (`ec2::ops::delete_coip_cidr::MAX_ECHOED_CIDR_CHARS`), so the bound rows below are written
//! against the constant rather than against a number of their own.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::coip_pools::{REGION, describe_one, described_ids, seed};
use super::ec2_query::{call, element, elements, error_code, error_message};

/// The HTTP status EC2 answers `DryRunOperation` with: the request was refused because a
/// precondition the CALLER set holds, not because anything was wrong with it.
const DRY_RUN_STATUS: u16 = 412;

/// One `DeleteCoipCidr` call carrying `params`, as its HTTP status and XML body.
async fn delete(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DeleteCoipCidr", params).await
}

/// One `DeleteCoipCidr` call naming `pool_id` and `cidr`.
async fn delete_cidr(router: &MultiServiceRouter, pool_id: &str, cidr: &str) -> (u16, String) {
    delete(
        router,
        &[
            ("CoipPoolId".to_string(), pool_id.to_string()),
            ("Cidr".to_string(), cidr.to_string()),
        ],
    )
    .await
}

/// Every member the one modelled `CoipCidr` an XML body carries, so what a delete reports can
/// be compared with what the describe reported for the same pool in one assertion.
#[derive(Debug, PartialEq, Eq)]
struct ReportedCidr {
    /// `CoipCidr$Cidr`.
    cidr: Option<String>,
    /// `CoipCidr$CoipPoolId`.
    coip_pool_id: Option<String>,
    /// `CoipCidr$LocalGatewayRouteTableId`.
    local_gateway_route_table_id: Option<String>,
}

/// The one range `xml` reports.
fn reported_cidr(xml: &str) -> ReportedCidr {
    ReportedCidr {
        cidr: element(xml, "cidr"),
        coip_pool_id: element(xml, "coipPoolId"),
        local_gateway_route_table_id: element(xml, "localGatewayRouteTableId"),
    }
}

/// The address ranges `DescribeCoipPools` reports for `pool_id`, in the order it reported
/// them — read out of the pool's own `poolCidrSet`, so a pool that holds none reads as an
/// empty list rather than borrowing another set's items.
async fn described_cidrs(router: &MultiServiceRouter, case: &str, pool_id: &str) -> Vec<String> {
    let body = describe_one(router, case, pool_id).await;
    match element(&body, "poolCidrSet") {
        Some(inner) => elements(&inner, "item"),
        None => Vec::new(),
    }
}

/// The local gateway route table `DescribeCoipPools` reports for `pool_id`.
async fn described_route_table(
    router: &MultiServiceRouter,
    case: &str,
    pool_id: &str,
) -> Option<String> {
    let body = describe_one(router, case, pool_id).await;
    element(&body, "localGatewayRouteTableId")
}

/// The ranges every seeded pool holds, in store order, as the describe reports them: the
/// snapshot the "everything else is left alone" assertions compare against.
async fn snapshot(
    router: &MultiServiceRouter,
    case: &str,
    seeded: &[String],
) -> Vec<(String, Vec<String>)> {
    let mut all = Vec::with_capacity(seeded.len());
    for pool_id in seeded {
        all.push((pool_id.clone(), described_cidrs(router, case, pool_id).await));
    }
    all
}

/// `block` spelled with host bits set: the same range of addresses, written from an address
/// inside it rather than from its base address.
///
/// The seeded ranges are all shorter than a /32, so the address one above the base is inside
/// the block; the caller asserts the spelling really differs from the stored one.
fn with_host_bits_set(block: &str) -> String {
    let (address, prefix_length) = block.split_once('/').expect("a seeded range is a CIDR block");
    let base: std::net::Ipv4Addr = address.parse().expect("a seeded range starts at an address");
    let inside = std::net::Ipv4Addr::from(u32::from(base) + 1);
    format!("{inside}/{prefix_length}")
}

/// Every `(pool position, range position)` pair the fixtures seed, so a table has one row per
/// stored range without naming a count of its own.
async fn every_stored_range() -> Vec<(usize, usize)> {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let mut pairs = Vec::new();
    for (position, pool_id) in seeded.iter().enumerate() {
        let count = described_cidrs(&router, "the seeded ranges", pool_id)
            .await
            .len();
        for index in 0..count {
            pairs.push((position, index));
        }
    }
    assert!(
        pairs.len() > 1,
        "the fixtures must seed more than one address range, got {}",
        pairs.len()
    );
    pairs
}

// ---------------------------------------------------------------------------
// The store is what is deleted, and what is reported
// ---------------------------------------------------------------------------

/// Deleting a range removes exactly it from the pool and answers with that range beside the
/// pool's own id and local gateway route table, as the describe reported them until this call.
/// Every other range of that pool, and every other pool, is left alone. One row per seeded
/// range, so the rows differ in which pool the range belongs to, how many ranges that pool
/// holds and which position in its list the range sits at.
#[tokio::test]
async fn deleting_a_range_removes_exactly_it_and_reports_what_was_stored() {
    for (position, index) in every_stored_range().await {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine);
        let pool_id = seeded[position].clone();
        let case = format!("pool {position} ({pool_id}), range {index}");

        let before = snapshot(&router, &case, &seeded).await;
        let stored = before[position].1.clone();
        let cidr = stored[index].clone();
        let route_table = described_route_table(&router, &case, &pool_id).await;

        let (status, body) = delete_cidr(&router, &pool_id, &cidr).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let expected = ReportedCidr {
            cidr: Some(cidr.clone()),
            coip_pool_id: Some(pool_id.clone()),
            local_gateway_route_table_id: route_table,
        };
        let reported = reported_cidr(&body);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the delete to report the stored range {expected:?}, got \
             {reported:?}"
        );

        let after = snapshot(&router, &case, &seeded).await;
        for (other, (before_pool, before_cidrs)) in before.iter().enumerate() {
            let (after_pool, after_cidrs) = &after[other];
            assert_eq!(
                after_pool, before_pool,
                "case {case:?}: expected pool {other} to still be {before_pool}, got {after_pool}"
            );
            let expected_cidrs: Vec<String> = if other == position {
                before_cidrs
                    .iter()
                    .enumerate()
                    .filter(|(at, _)| *at != index)
                    .map(|(_, range)| range.clone())
                    .collect()
            } else {
                before_cidrs.clone()
            };
            assert_eq!(
                after_cidrs, &expected_cidrs,
                "case {case:?}: expected pool {other} to hold {expected_cidrs:?}, got \
                 {after_cidrs:?}"
            );
        }

        let pools = described_ids(&router, &case, &[]).await;
        assert_eq!(
            pools, seeded,
            "case {case:?}: deleting a range deletes no pool, so expected every pool to still \
             be reported, got {pools:?}"
        );
    }
}

/// Deleting every range of a pool in turn empties it and leaves the pool itself in place:
/// the ranges go one at a time, each reported as it was stored, and the pool ends up holding
/// none. The rows are the pools that hold more than one range, so the number of deletes
/// differs per row.
#[tokio::test]
async fn deleting_every_range_of_a_pool_empties_it_without_deleting_the_pool() {
    let positions: Vec<usize> = {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine);
        let mut positions = Vec::new();
        for (position, pool_id) in seeded.iter().enumerate() {
            if described_cidrs(&router, "the seeded pools", pool_id).await.len() > 1 {
                positions.push(position);
            }
        }
        positions
    };

    for position in positions {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine);
        let pool_id = seeded[position].clone();
        let case = format!("pool {position} ({pool_id})");

        // Delete the LAST remaining range each time, so the rows also differ in the order the
        // ranges leave the pool.
        let stored = described_cidrs(&router, &case, &pool_id).await;
        for taken in 0..stored.len() {
            let remaining = described_cidrs(&router, &case, &pool_id).await;
            let cidr = remaining
                .last()
                .unwrap_or_else(|| panic!("case {case:?}: expected a range left after {taken}"))
                .clone();
            let (status, body) = delete_cidr(&router, &pool_id, &cidr).await;
            assert_eq!(
                status, 200,
                "case {case:?}: deleting {cidr} expected HTTP 200, got {status} with body {body}"
            );
            let reported = element(&body, "cidr");
            assert_eq!(
                reported,
                Some(cidr.clone()),
                "case {case:?}: expected the response to name {cidr}, got {reported:?}"
            );
        }

        let left = described_cidrs(&router, &case, &pool_id).await;
        assert!(
            left.is_empty(),
            "case {case:?}: expected the pool to hold no range, got {left:?}"
        );
        let pools = described_ids(&router, &case, &[]).await;
        assert_eq!(
            pools, seeded,
            "case {case:?}: emptying a pool deletes no pool, so expected every pool to still \
             be reported, got {pools:?}"
        );
    }
}

/// A range is named by the ADDRESSES it covers, not by the spelling of its base address: a
/// request whose `Cidr` carries host bits names the block those bits sit in, and the response
/// reports the range as the pool stored it. One row per seeded range.
#[tokio::test]
async fn a_range_is_named_by_the_addresses_it_covers() {
    for (position, index) in every_stored_range().await {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine);
        let pool_id = seeded[position].clone();
        let case = format!("pool {position} ({pool_id}), range {index}");

        let stored = described_cidrs(&router, &case, &pool_id).await;
        let cidr = stored[index].clone();
        let spelled = with_host_bits_set(&cidr);
        assert_ne!(
            spelled, cidr,
            "case {case:?}: the row must send a different spelling of {cidr}"
        );

        let (status, body) = delete_cidr(&router, &pool_id, &spelled).await;
        assert_eq!(
            status, 200,
            "case {case:?}: deleting {spelled} expected HTTP 200, got {status} with body {body}"
        );
        let reported = element(&body, "cidr");
        assert_eq!(
            reported,
            Some(cidr.clone()),
            "case {case:?}: expected the stored spelling {cidr}, got {reported:?}"
        );

        let remaining = described_cidrs(&router, &case, &pool_id).await;
        assert!(
            !remaining.contains(&cidr),
            "case {case:?}: expected {cidr} to be gone, got {remaining:?}"
        );
        assert_eq!(
            remaining.len(),
            stored.len() - 1,
            "case {case:?}: expected exactly one range to go, got {remaining:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A request that resolves to no range
// ---------------------------------------------------------------------------

/// A `CoipPoolId` that resolves to no pool of the account is refused and nothing is removed,
/// and the CODE says HOW the value failed: a well-formed pool id nothing was created for is
/// `InvalidPoolID.NotFound`, a value that READS as a pool id but carries a digit count EC2
/// never mints is `InvalidParameterValue`, and a value that is no pool id at all is
/// `InvalidPoolID.Malformed`. The pool is resolved BEFORE the range, so the answer does not
/// depend on the `Cidr` the request sent. The rows are each way a value can fail to resolve.
#[tokio::test]
async fn a_pool_id_that_resolves_to_no_pool_is_refused() {
    let prefix = ec2::state::COIP_POOL_ID_PREFIX;
    let not_found = ec2::state::INVALID_POOL_ID_NOT_FOUND;
    let bad_value = ec2::state::INVALID_PARAMETER_VALUE;
    let malformed = ec2::state::INVALID_POOL_ID_MALFORMED;
    // The forms a pool id takes are `COIP_POOL_ID_PREFIX` plus 8 or 17 lowercase hex digits,
    // which `ec2::state::coip_pool_id_is_well_formed` is the one reader of.
    let cases = [
        ("the long form", format!("{prefix}00000000000000000"), not_found),
        ("the short form", format!("{prefix}00000000"), not_found),
        ("one digit", format!("{prefix}0"), bad_value),
        ("neither length", format!("{prefix}0000000000000000"), bad_value),
        ("far too many digits", format!("{prefix}{}", "a".repeat(32)), bad_value),
        ("an upper-case suffix", format!("{prefix}0ABCDEF01"), malformed),
        ("a non-hex suffix", format!("{prefix}zzzzzzzz"), malformed),
        (
            "another resource's id",
            "subnet-00000000000000000".to_string(),
            malformed,
        ),
        ("free text", "not-a-pool-at-all".to_string(), malformed),
        ("the empty value", String::new(), malformed),
    ];

    for (case, pool_id, expected_code) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine);
        let before = snapshot(&router, case, &seeded).await;

        // The range is one a seeded pool really holds, so only the pool id can be what fails.
        let cidr = before
            .iter()
            .flat_map(|(_, cidrs)| cidrs.iter())
            .next()
            .expect("the fixtures seed at least one range")
            .clone();
        let (status, body) = delete_cidr(&router, &pool_id, &cidr).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code:?}, got {body}"
        );
        if expected_code != malformed {
            let message = error_message(&body);
            assert_eq!(
                message,
                format!("The pool ID '{pool_id}' does not exist."),
                "case {case:?}: expected the surface's one sentence, got {message:?}"
            );
        }
        let after = snapshot(&router, case, &seeded).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused delete must remove nothing, got {after:?}"
        );
    }
}

/// A `Cidr` that is no address range at all names no range of any pool, so it is refused as a
/// bad parameter value and nothing is removed. The rows are each way a value can fail to be an
/// IPv4 CIDR block: no prefix length, a prefix length longer than an IPv4 address, a prefix
/// length that is not a number, a base that is not an address, an address with too few or too
/// many octets, an octet past 255, free text, the empty value — and an IPv6 block, which is no
/// range of a customer-owned IPv4 pool.
#[tokio::test]
async fn a_cidr_that_is_no_address_range_is_refused() {
    let cases = [
        ("no prefix length", "10.0.0.0"),
        ("a prefix length past 32", "10.0.0.0/33"),
        ("a prefix length that is not a number", "10.0.0.0/abc"),
        ("an empty prefix length", "10.0.0.0/"),
        ("too few octets", "10.0.0/24"),
        ("too many octets", "10.0.0.0.0/24"),
        ("an octet past 255", "10.0.0.256/24"),
        ("an IPv6 block", "2001:db8::/32"),
        ("free text", "not-a-range-at-all"),
        ("the empty value", ""),
    ];

    for (case, cidr) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine);
        let before = snapshot(&router, case, &seeded).await;
        let pool_id = seeded[0].clone();

        let (status, body) = delete_cidr(&router, &pool_id, cidr).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::INVALID_PARAMETER_VALUE,
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            format!("Value ({cidr}) for parameter cidr is invalid. This is not a valid CIDR block."),
            "case {case:?}: expected the service's rejected-value sentence, got {message:?}"
        );
        let after = snapshot(&router, case, &seeded).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused delete must remove nothing, got {after:?}"
        );
    }
}

/// A `Cidr` the pool the request names holds no range for is refused with the code this
/// service answers an address range it holds nothing for (`InvalidCidr.NotFound`), and nothing
/// is removed. The rows are each way a well-formed range can fail to be one of THAT pool's: a
/// range another seeded pool holds, a range no pool holds, a block inside a range it holds, a
/// block that contains one, and a range of the pool that was already deleted
/// — a delete is not idempotent.
#[tokio::test]
async fn a_range_the_pool_does_not_hold_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let pool_id = seeded[0].clone();
    let stored = described_cidrs(&router, "the subject pool", &pool_id).await;
    let own = stored
        .first()
        .expect("the first seeded pool holds a range")
        .clone();
    // A range of another pool: the first range any OTHER seeded pool holds.
    let mut elsewhere = None;
    for other in &seeded[1..] {
        let cidrs = described_cidrs(&router, "another pool", other).await;
        if let Some(first) = cidrs.first() {
            elsewhere = Some(first.clone());
            break;
        }
    }
    let elsewhere = elsewhere.expect("another seeded pool holds a range");
    // A block strictly inside the pool's own range, and one that strictly contains it: both
    // name a different set of addresses, so neither is the range the pool holds. The row only
    // makes sense for a seeded range that HAS a neighbour on each side, which is asserted
    // rather than assumed so a fixture change says so instead of testing the wrong code.
    let (base, prefix_length) = own.split_once('/').expect("a seeded range is a CIDR block");
    let prefix_length: u8 = prefix_length.parse().expect("a prefix length");
    assert!(
        (1..32).contains(&prefix_length),
        "the seeded range {own} must have a longer and a shorter prefix to compare against, \
         got /{prefix_length}"
    );
    let inside = format!("{base}/{}", prefix_length + 1);
    let around = format!("{base}/{}", prefix_length - 1);

    let cases = [
        ("a range of another pool", elsewhere.as_str()),
        ("a range no pool holds", "203.0.113.0/24"),
        ("a block inside its own range", inside.as_str()),
        ("a block around its own range", around.as_str()),
    ];
    for (case, cidr) in cases {
        let before = snapshot(&router, case, &seeded).await;
        let (status, body) = delete_cidr(&router, &pool_id, cidr).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::INVALID_CIDR_NOT_FOUND,
            "case {case:?}: expected InvalidCidr.NotFound, got {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            format!("The CIDR ({cidr}) does not exist."),
            "case {case:?}: expected the service's range-not-found sentence naming the range \
             the request sent, got {message:?}"
        );
        let after = snapshot(&router, case, &seeded).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused delete must remove nothing, got {after:?}"
        );
    }

    // The same range twice: the first delete removes it, so the second resolves nothing.
    let (status, body) = delete_cidr(&router, &pool_id, &own).await;
    assert_eq!(
        status, 200,
        "first delete: expected HTTP 200, got {status} with body {body}"
    );
    let (status, body) = delete_cidr(&router, &pool_id, &own).await;
    assert_eq!(
        status, 400,
        "second delete: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        ec2::state::INVALID_CIDR_NOT_FOUND,
        "second delete: expected InvalidCidr.NotFound, got {body}"
    );
    let message = error_message(&body);
    assert_eq!(
        message,
        format!("The CIDR ({own}) does not exist."),
        "second delete: expected the range to be reported gone, got {message:?}"
    );
    let pools = described_ids(&router, "second delete", &[]).await;
    assert_eq!(
        pools, seeded,
        "second delete: no pool may be removed, got {pools:?}"
    );
}

/// A refused `Cidr` is named in the message only up to the echo bound this crate keeps on it:
/// `DeleteCoipCidrRequest$Cidr` carries no `@length`, so an arbitrarily long value is bounded
/// before it is quoted back. Rows: a value AT the bound, which is named in full, and one past
/// it, which is named up to the bound.
#[tokio::test]
async fn an_over_long_cidr_is_named_only_up_to_the_echo_bound() {
    let bound = ec2::ops::delete_coip_cidr::MAX_ECHOED_CIDR_CHARS;
    for (case, length) in [("at the bound", bound), ("past the bound", bound + 16)] {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine);
        let cidr = "9".repeat(length);

        let (status, body) = delete_cidr(&router, &seeded[0], &cidr).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let message = error_message(&body);
        let echoed: String = cidr.chars().take(bound).collect();
        assert_eq!(
            message,
            format!(
                "Value ({echoed}) for parameter cidr is invalid. This is not a valid CIDR block."
            ),
            "case {case:?}: expected the message to name at most {bound} characters, got \
             {message:?}"
        );
        let stored = described_cidrs(&router, case, &seeded[0]).await;
        assert!(
            !stored.is_empty(),
            "case {case:?}: a refused delete must remove nothing, got {stored:?}"
        );
    }
}

/// A `Cidr` the pool holds no range for is named up to the SAME echo bound, even though it
/// parsed: a prefix length may be written with any number of leading zeros, so a readable
/// value is no shorter than an unreadable one. Rows: a zero-padded range AT the bound, which
/// is named in full, and one past it, which is named up to the bound.
#[tokio::test]
async fn an_over_long_but_readable_cidr_is_named_only_up_to_the_echo_bound() {
    let bound = ec2::ops::delete_coip_cidr::MAX_ECHOED_CIDR_CHARS;
    // A range no seeded pool holds, so only its length is under test here.
    let unheld = "203.0.113.0/24";
    for (case, padding) in [
        ("at the bound", bound - unheld.len()),
        ("past the bound", bound),
    ] {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine);
        let cidr = format!("203.0.113.0/{}24", "0".repeat(padding));

        let (status, body) = delete_cidr(&router, &seeded[0], &cidr).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::INVALID_CIDR_NOT_FOUND,
            "case {case:?}: the value is a readable range, so expected InvalidCidr.NotFound, \
             got {body}"
        );
        let message = error_message(&body);
        let echoed: String = cidr.chars().take(bound).collect();
        assert_eq!(
            message,
            format!("The CIDR ({echoed}) does not exist."),
            "case {case:?}: expected the message to name at most {bound} characters, got \
             {message:?}"
        );
    }
}

/// A request that leaves out a member needed to name one range is refused rather than answered
/// with an empty success. Both members are `@required`, so the boundary validator is what
/// refuses each of these — before this operation's handler runs. Rows: no `Cidr`, no
/// `CoipPoolId`, neither.
#[tokio::test]
async fn a_request_that_names_no_range_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let before = snapshot(&router, "the seeded pools", &seeded).await;
    let cidr = before[0].1[0].clone();

    let cases: [(&str, Vec<(String, String)>); 3] = [
        (
            "no Cidr",
            vec![("CoipPoolId".to_string(), seeded[0].clone())],
        ),
        ("no CoipPoolId", vec![("Cidr".to_string(), cidr)]),
        ("neither", Vec::new()),
    ];

    for (case, params) in cases {
        let (status, body) = delete(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let after = snapshot(&router, case, &seeded).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused delete must remove nothing, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and deletes nothing,
/// whatever the pool and the range it names; `DryRun=false` is an ordinary delete. The rows are
/// a request that would have succeeded, one naming a pool the store does not hold, and one
/// naming a value that is no address range: the flag asks whether the caller is permitted,
/// which is not a question about either member.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_deleting() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let before = snapshot(&router, "the seeded pools", &seeded).await;
    let pool_id = seeded[0].clone();
    let cidr = before[0].1[0].clone();
    let unknown_pool = format!("{}00000000000000000", ec2::state::COIP_POOL_ID_PREFIX);

    for (case, pool, range) in [
        ("a stored range", pool_id.as_str(), cidr.as_str()),
        ("an unknown pool", unknown_pool.as_str(), cidr.as_str()),
        ("a value that is no range", pool_id.as_str(), "not-a-range"),
    ] {
        let (status, body) = delete(
            &router,
            &[
                ("CoipPoolId".to_string(), pool.to_string()),
                ("Cidr".to_string(), range.to_string()),
                ("DryRun".to_string(), "true".to_string()),
            ],
        )
        .await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::DRY_RUN_OPERATION,
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        let after = snapshot(&router, case, &seeded).await;
        assert_eq!(
            after, before,
            "case {case:?}: a dry run must delete nothing, got {after:?}"
        );
    }

    // `DryRun=false` is no dry run at all: the delete happens.
    let (status, body) = delete(
        &router,
        &[
            ("CoipPoolId".to_string(), pool_id.clone()),
            ("Cidr".to_string(), cidr.clone()),
            ("DryRun".to_string(), "false".to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "DryRun=false: expected HTTP 200, got {status} with body {body}"
    );
    let remaining = described_cidrs(&router, "DryRun=false", &pool_id).await;
    assert!(
        !remaining.contains(&cidr),
        "DryRun=false: expected {cidr} to be deleted, got {remaining:?}"
    );
}
