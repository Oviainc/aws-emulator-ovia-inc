// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the public IPv4 address-pool slice —
//! `DeprovisionPublicIpv4PoolCidr` and the `DescribePublicIpv4Pools` that reads a pool back
//! — driven over the wire the service actually speaks: `ec2Query`, an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=<Operation>`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol"). The envelope is selected
//! by the `ec2` signing name in the request's SigV4 credential scope, so every call here
//! carries one.
//!
//! The rules under test:
//!
//! 1. PROVISIONING AN IPv4 ADDRESS RANGE CREATES THE POOL THAT HOLDS IT — "provisioning an
//!    IPv4 or IPv6 address range … creates a corresponding address pool"
//!    (`ProvisionByoipCidr`) — under the `ipv4pool-ec2-…` id the range reports as its
//!    `PoolId`, carrying the range, the description and the pool tags the request asked
//!    for. An IPv6 range's pool is an `ipv6pool-ec2`, which is NOT a public IPv4 pool.
//! 2. A POOL REPORTS ITS RANGES, DERIVED: each `PoolAddressRanges` entry's first and last
//!    address and address count are the bounds and the size of the block, its available
//!    count is that size less the account's Elastic IP addresses that fall inside the
//!    block, and the pool's totals are the sums of those — so the numbers follow the
//!    ranges and the addresses rather than a stored copy of them, and a read changes
//!    nothing.
//! 3. `DeprovisionPublicIpv4PoolCidr` TAKES EXACTLY THE NAMED RANGE OUT OF THE NAMED POOL:
//!    it answers the pool id and the range it removed, the pool keeps every other range,
//!    and the totals follow. A range named with host bits set is the canonical block.
//! 4. WHAT IT REFUSES: a pool id the account holds no pool for is HTTP 400
//!    `InvalidPublicIpv4Pool.NotFound`, a `Cidr` that is not an IPv4 CIDR block or that the
//!    pool does not hold is `InvalidParameterValue`, and a `DryRun` is answered from the
//!    permission check alone (HTTP 412 `DryRunOperation`) with nothing removed. Each
//!    refusal carries the `RequestID` EC2's error response documents, fresh per response.
//! 5. THE DESCRIBE REPORTS THE STORE: narrowed by the `PoolIds` and the documented
//!    `tag:<key>` / `tag-key` filters the request names, cut into `MaxResults`-sized pages
//!    in pool-id order.
//!
//! Every table row is a distinct input along what those generalise over — which prefix
//! length, which base address, how many ranges a pool holds, which of them is removed,
//! whether the block is written in canonical form, which filter, which page size — so no
//! single recorded call is the specification. The expected bounds and counts are computed
//! in this file from `std::net::Ipv4Addr`, independently of the service's own CIDR reader.
//!
//! The address ranges a row SEEDS are the blocks RFC 5737 (`192.0.2.0/24`,
//! `198.51.100.0/24`, `203.0.113.0/24`) and RFC 2544 (`198.18.0.0/15`) set aside for
//! documentation and benchmarking, so no seeded row names a range that belongs to anybody.
//! A row that PROVISIONS one cannot use those: "The public IPv4 or IPv6 address range"
//! (`ProvisionByoipCidrRequest$Cidr`) excludes every special-purpose block, so those rows
//! use the same public example ranges the sibling `byoip_cidrs` suite provisions.
//!
//! A pool holding SEVERAL ranges is seeded straight into the store, the way
//! `byoip_cidr_lifecycle` and `coip_pools` seed theirs: one BYOIP provision creates a pool
//! with one range, and nothing in the implemented surface provisions a second range to a
//! pool (`ProvisionPublicIpv4PoolCidr` is still a scaffold), so the multi-range rows have
//! no API path. The creation path a caller takes is covered by rule 1 above, over the wire.
//!
//! Each refusal's MESSAGE is checked only for the substitution the service makes — the
//! value the request sent, named back — never for wording this emulator has no rule for.

use std::net::Ipv4Addr;

use ec2::ops::types::{Address, Tag};
use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, PublicIpv4PoolSpec};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, filters, indexed, unescaped};

/// The Region every call here is signed for, and so the network border group a provisioned
/// range and its pool are advertised from.
const REGION: &str = "us-west-2";

/// EC2's error code for a public IPv4 pool id the account holds no pool for.
const NOT_FOUND: &str = "InvalidPublicIpv4Pool.NotFound";

/// EC2's error code for a member value the service cannot accept.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// EC2's error code for a member the operation needs that reached the handler empty.
const MISSING_PARAMETER: &str = "MissingParameter";

/// EC2's error code for a `DryRun` request that would otherwise have been served.
const DRY_RUN_OPERATION: &str = "DryRunOperation";

/// EC2's error code for a `NextToken` this service did not issue.
const INVALID_PAGINATION_TOKEN: &str = "InvalidPaginationToken";

/// The id prefix EC2 spells a public IPv4 pool with (`ipv4pool-ec2-…`).
const POOL_ID_PREFIX: &str = "ipv4pool-ec2-";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// The emulator the tests drive, with the engine the seeded rows write through.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One `DeprovisionPublicIpv4PoolCidr` call carrying `params`.
async fn deprovision(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DeprovisionPublicIpv4PoolCidr", params).await
}

/// One `DeprovisionPublicIpv4PoolCidr` call for `cidr` out of `pool_id`.
async fn deprovision_cidr(
    router: &MultiServiceRouter,
    pool_id: &str,
    cidr: &str,
) -> (u16, String) {
    deprovision(router, &[p("PoolId", pool_id), p("Cidr", cidr)]).await
}

/// One `DescribePublicIpv4Pools` call carrying `params`.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribePublicIpv4Pools", params).await
}

/// One `ProvisionByoipCidr` call carrying `params`.
async fn provision_byoip(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "ProvisionByoipCidr", params).await
}

/// The XML of the one pool `pool_id` names, asserted to have been answered.
async fn described_pool(router: &MultiServiceRouter, case: &str, pool_id: &str) -> String {
    let (status, body) = describe(router, &indexed("PoolId", &[pool_id])).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribePublicIpv4Pools, got {status} with body \
         {body}"
    );
    body
}

/// The pool ids a successful describe reports, in the order it reported them.
async fn described_ids(router: &MultiServiceRouter, case: &str, params: &[(String, String)]) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribePublicIpv4Pools, got {status} with body \
         {body}"
    );
    elements(&body, "poolId")
}

/// The `(first address, last address, address count)` of one IPv4 CIDR block, computed here
/// from the standard library's own address parser rather than from the service's reader, so
/// the table's expectations are independent of the code under test.
fn block_bounds(cidr: &str) -> (String, String, u64) {
    let (address, prefix_length) = cidr.split_once('/').expect("a block carries a prefix length");
    let base = u32::from(address.parse::<Ipv4Addr>().expect("a dotted quad"));
    let prefix_length: u32 = prefix_length.parse().expect("a prefix length");
    let size = 1u64 << (32 - prefix_length);
    let mask = u32::try_from(size - 1).expect("a host mask of an IPv4 block");
    let first = base & !mask;
    (
        Ipv4Addr::from(first).to_string(),
        Ipv4Addr::from(first | mask).to_string(),
        size,
    )
}

/// A tag, as the seeded and provisioned pools carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// Write one pool holding `cidrs` into the store and answer the id minted for it.
///
/// The store's own insert is what mints the id and canonicalises the ranges, so a seeded
/// pool is a pool exactly as a provision would have created it.
fn seed_pool(
    engine: &engine::InMemoryEngine,
    cidrs: &[&str],
    tags: Vec<Tag>,
    description: Option<&str>,
) -> String {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    guard
        .insert_public_ipv4_pool(PublicIpv4PoolSpec {
            description: description.map(str::to_string),
            network_border_group: REGION.to_string(),
            tags,
            cidrs: cidrs.iter().map(|cidr| (*cidr).to_string()).collect(),
        })
        .expect("the store has room for the seeded pool")
}

/// Give the account an Elastic IP address at `public_ip`, so the availability rule has an
/// allocated address to count.
fn seed_address(engine: &engine::InMemoryEngine, allocation_id: &str, public_ip: &str) {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    guard.addresses.insert(
        allocation_id.to_string(),
        Address {
            allocation_id: Some(allocation_id.to_string()),
            public_ip: Some(public_ip.to_string()),
            ..Address::default()
        },
    );
}

/// The address ranges the store holds for `pool_id`, read straight from the store so it
/// reports what a later caller would be told without a call of its own.
fn stored_cidrs(engine: &engine::InMemoryEngine, pool_id: &str) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard
        .public_ipv4_pools
        .get(pool_id)
        .map(|record| record.cidrs.iter().cloned().collect())
        .unwrap_or_default()
}

/// One accepted-range row: `(case, the Cidr sent, the canonical block it becomes)`.
type RangeRow = (&'static str, &'static str, &'static str);

/// The ranges rule 1 and rule 2 are written against: one past each end of the prefix
/// lengths a BYOIP provision accepts (the published most specific `/24` and a much larger
/// block), two bases inside one documentation range, and a block written with host bits set.
const RANGES: [RangeRow; 5] = [
    (
        "the published most specific prefix",
        "81.3.69.0/24",
        "81.3.69.0/24",
    ),
    ("a /23", "81.3.70.0/23", "81.3.70.0/23"),
    ("a /22 at a non-zero base", "81.3.72.0/22", "81.3.72.0/22"),
    ("a /16", "81.4.0.0/16", "81.4.0.0/16"),
    (
        "a range given with host bits set",
        "81.5.200.33/24",
        "81.5.200.0/24",
    ),
];

/// RULE 1 and RULE 2: a provisioned IPv4 range gives the account one public IPv4 pool
/// holding that range, which the describe reports with the bounds and counts of the block.
#[tokio::test]
async fn a_provisioned_ipv4_range_creates_the_pool_that_holds_it() {
    for (case, requested, canonical) in RANGES {
        // One emulator per row: the rows are independent inputs, not a sequence, and an
        // empty store is what makes "exactly this pool is reported" an assertion about the
        // row.
        let (router, _engine) = emulator();
        let params: Params = vec![
            p("Cidr", requested),
            p("Description", "a documentation range"),
            p("PoolTagSpecification.1.ResourceType", "ipv4pool-ec2"),
            p("PoolTagSpecification.1.Tag.1.Key", "Owner"),
            p("PoolTagSpecification.1.Tag.1.Value", "TeamA"),
        ];
        let (status, body) = provision_byoip(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ProvisionByoipCidr, got {status} with body \
             {body}"
        );
        let pool_id = element(&body, "poolId").unwrap_or_default();
        assert!(
            pool_id.starts_with(POOL_ID_PREFIX),
            "case {case:?}: expected a minted public IPv4 pool id, got {pool_id:?}"
        );

        let described = described_pool(&router, case, &pool_id).await;
        assert_eq!(
            elements(&described, "poolId"),
            vec![pool_id.clone()],
            "case {case:?}: expected exactly the pool the provision created, got {described}"
        );
        let (first, last, count) = block_bounds(canonical);
        assert_eq!(
            element(&described, "firstAddress").unwrap_or_default(),
            first,
            "case {case:?}: expected the block's first address, got {described}"
        );
        assert_eq!(
            element(&described, "lastAddress").unwrap_or_default(),
            last,
            "case {case:?}: expected the block's last address, got {described}"
        );
        for (member, expected) in [
            ("addressCount", count),
            ("availableAddressCount", count),
            ("totalAddressCount", count),
            ("totalAvailableAddressCount", count),
        ] {
            let reported = element(&described, member).unwrap_or_default();
            assert_eq!(
                reported,
                expected.to_string(),
                "case {case:?}: expected {member} {expected} for {canonical}, got {reported:?} \
                 in {described}"
            );
        }
        assert_eq!(
            element(&described, "networkBorderGroup").unwrap_or_default(),
            REGION,
            "case {case:?}: a pool created without a border group is advertised from its \
             Region, got {described}"
        );
        assert_eq!(
            element(&described, "description").unwrap_or_default(),
            "a documentation range",
            "case {case:?}: the description describes the range and the pool, got {described}"
        );
        assert_eq!(
            (
                element(&described, "key").unwrap_or_default(),
                element(&described, "value").unwrap_or_default()
            ),
            ("Owner".to_string(), "TeamA".to_string()),
            "case {case:?}: the pool carries the tags the provision asked for, got {described}"
        );

        // RULE 2: reading twice reports the same pool — a describe writes nothing. The
        // comparison is of the pool SET, since the envelope's `requestId` is per-call.
        let again = described_pool(&router, case, &pool_id).await;
        assert_eq!(
            element(&again, "publicIpv4PoolSet"),
            element(&described, "publicIpv4PoolSet"),
            "case {case:?}: a second describe must report the same pool, got {again}"
        );
    }
}

/// RULE 1, the other side of the family rule: an IPv6 range's corresponding pool is an
/// `ipv6pool-ec2`, so no public IPv4 pool appears for it.
#[tokio::test]
async fn an_ipv6_range_creates_no_public_ipv4_pool() {
    let (router, _engine) = emulator();
    let (status, body) = provision_byoip(&router, &[p("Cidr", "2600:1f14:abc::/48")]).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from ProvisionByoipCidr, got {status} with body {body}"
    );
    let pool_id = element(&body, "poolId").unwrap_or_default();
    assert!(
        !pool_id.starts_with(POOL_ID_PREFIX),
        "an IPv6 range's pool is an ipv6pool-ec2, got {pool_id:?}"
    );
    let reported = described_ids(&router, "an IPv6 range", &[]).await;
    assert!(
        reported.is_empty(),
        "an IPv6 range gives the account no public IPv4 pool, got {reported:?}"
    );
}

/// The ranges a seeded multi-range pool holds, in address order. Different prefix lengths
/// and bases, so "the named one went and the others stayed" is an assertion about the range
/// the request named rather than about position alone.
const SEEDED: [&str; 3] = ["192.0.2.0/24", "198.18.0.0/23", "203.0.113.0/26"];

/// One removal row: `(case, the Cidr sent, the canonical block it names)`.
type RemovalRow = (&'static str, &'static str, &'static str);

/// RULE 3 over every range of a pool that holds several, and over both forms a caller can
/// write a block in.
const REMOVALS: [RemovalRow; 4] = [
    ("the first range", "192.0.2.0/24", "192.0.2.0/24"),
    ("the middle range", "198.18.0.0/23", "198.18.0.0/23"),
    ("the last range", "203.0.113.0/26", "203.0.113.0/26"),
    (
        "a range named with host bits set",
        "198.18.1.200/23",
        "198.18.0.0/23",
    ),
];

/// RULE 3: the call takes exactly the named range out of the named pool, and the pool's
/// ranges and totals follow.
#[tokio::test]
async fn deprovisioning_takes_exactly_the_named_range_out() {
    for (case, requested, canonical) in REMOVALS {
        let (router, engine) = emulator();
        let pool_id = seed_pool(&engine, &SEEDED, vec![tag("Owner", "TeamA")], None);

        let (status, body) = deprovision_cidr(&router, &pool_id, requested).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "poolId").unwrap_or_default(),
            pool_id,
            "case {case:?}: the response names the pool the range came out of, got {body}"
        );
        assert_eq!(
            elements(&body, "item"),
            vec![canonical.to_string()],
            "case {case:?}: the response reports the one range it removed, got {body}"
        );

        // The store holds exactly the other ranges — nothing else was touched.
        let left: Vec<String> = SEEDED
            .iter()
            .filter(|cidr| **cidr != canonical)
            .map(|cidr| (*cidr).to_string())
            .collect();
        let mut held = stored_cidrs(&engine, &pool_id);
        held.sort();
        let mut expected = left.clone();
        expected.sort();
        assert_eq!(
            held, expected,
            "case {case:?}: expected the pool to keep {expected:?}, got {held:?}"
        );

        // And the describe reports them, with the totals of what is left.
        let described = described_pool(&router, case, &pool_id).await;
        let mut reported = elements(&described, "firstAddress");
        reported.sort();
        let mut first_addresses: Vec<String> = left
            .iter()
            .map(|cidr| block_bounds(cidr).0)
            .collect();
        first_addresses.sort();
        assert_eq!(
            reported, first_addresses,
            "case {case:?}: expected the remaining ranges {first_addresses:?}, got {reported:?} \
             in {described}"
        );
        let total: u64 = left.iter().map(|cidr| block_bounds(cidr).2).sum();
        for member in ["totalAddressCount", "totalAvailableAddressCount"] {
            let answered = element(&described, member).unwrap_or_default();
            assert_eq!(
                answered,
                total.to_string(),
                "case {case:?}: expected {member} {total} after the removal, got {answered:?} \
                 in {described}"
            );
        }

        // Asking again finds no such range: the removal is not something a repeat undoes or
        // repeats.
        let (status, body) = deprovision_cidr(&router, &pool_id, requested).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a range already taken out is no range of the pool, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PARAMETER_VALUE,
            "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {body}"
        );
    }
}

/// RULE 4, the pool: every id shape the account holds no pool for is refused with the
/// catalog's own code, and nothing is removed.
#[tokio::test]
async fn a_pool_the_account_does_not_hold_is_refused() {
    // One well-formed id of each kind a caller could send — the 17-hex-digit form EC2
    // mints, a short legacy-length one, an id of ANOTHER pool family, and a value that is
    // no pool id at all — so the refusal is about the pool not being the account's rather
    // than about the id's shape.
    const UNKNOWN: [(&str, &str); 4] = [
        ("a well-formed id nothing holds", "ipv4pool-ec2-00000000000000000"),
        ("a short id", "ipv4pool-ec2-1a2b3c4d"),
        ("an IPv6 pool id", "ipv6pool-ec2-0abcdef1234567890"),
        ("a value that is no pool id", "not-a-pool"),
    ];
    for (case, pool_id) in UNKNOWN {
        let (router, engine) = emulator();
        // A pool the account DOES hold, so "not found" cannot be an empty store answering.
        let held = seed_pool(&engine, &SEEDED, Vec::new(), None);

        let (status, body) = deprovision_cidr(&router, pool_id, SEEDED[0]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            NOT_FOUND,
            "case {case:?}: expected {NOT_FOUND}, got {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert!(
            message.contains(pool_id),
            "case {case:?}: the refusal names the id the request sent, got {message:?}"
        );
        assert_eq!(
            stored_cidrs(&engine, &held).len(),
            SEEDED.len(),
            "case {case:?}: a refused call must remove nothing, got {:?}",
            stored_cidrs(&engine, &held)
        );
    }
}

/// RULE 4, the range: a `Cidr` that is not an IPv4 CIDR block, or one the pool does not
/// hold, is refused and nothing is removed.
#[tokio::test]
async fn a_cidr_that_is_no_range_of_the_pool_is_refused() {
    // The two reasons, one row per form: values that are not an IPv4 block at all (no
    // prefix length, a prefix past the address, an IPv6 block, text), and blocks that ARE
    // IPv4 blocks but not ranges THIS pool holds (one inside a range it holds, one
    // containing it, one it never had).
    const REFUSED: [(&str, &str); 7] = [
        ("no prefix length", "192.0.2.0"),
        ("a prefix past the address", "192.0.2.0/33"),
        ("an IPv6 block", "2001:db8::/32"),
        ("text", "banana"),
        ("a block inside a range the pool holds", "192.0.2.128/25"),
        ("a block containing a range the pool holds", "192.0.0.0/16"),
        ("a block the pool never held", "198.51.100.0/24"),
    ];
    for (case, cidr) in REFUSED {
        let (router, engine) = emulator();
        let pool_id = seed_pool(&engine, &SEEDED, Vec::new(), None);

        let (status, body) = deprovision_cidr(&router, &pool_id, cidr).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PARAMETER_VALUE,
            "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert!(
            message.contains(cidr),
            "case {case:?}: the refusal names the value the request sent, got {message:?}"
        );
        let held = stored_cidrs(&engine, &pool_id);
        assert_eq!(
            held.len(),
            SEEDED.len(),
            "case {case:?}: a refused call must remove nothing, got {held:?}"
        );
    }
}

/// RULE 4, the dry run: answered from the permission check alone, before any pool is looked
/// up, and nothing is removed.
#[tokio::test]
async fn a_dry_run_removes_nothing() {
    // Both a pool the account holds and one it does not: `DryRun` asks about PERMISSION, so
    // the answer cannot depend on the pool.
    let (router, engine) = emulator();
    let pool_id = seed_pool(&engine, &SEEDED, Vec::new(), None);
    for (case, pool) in [
        ("a pool the account holds", pool_id.clone()),
        (
            "a pool it does not",
            "ipv4pool-ec2-00000000000000000".to_string(),
        ),
    ] {
        let (status, body) = deprovision(
            &router,
            &[p("PoolId", pool), p("Cidr", SEEDED[0]), p("DryRun", "true")],
        )
        .await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            DRY_RUN_OPERATION,
            "case {case:?}: expected {DRY_RUN_OPERATION}, got {body}"
        );
        let held = stored_cidrs(&engine, &pool_id);
        assert_eq!(
            held.len(),
            SEEDED.len(),
            "case {case:?}: a dry run must remove nothing, got {held:?}"
        );
    }
}

/// RULE 4, the request's own form: a member the operation needs that reaches the handler
/// empty is refused as missing, before any pool is looked up.
#[tokio::test]
async fn a_request_missing_a_member_is_refused() {
    // `PoolId` and `Cidr` are modelled `required` but `clientOptional`, so a request that
    // sends the field empty reaches the handler with an empty string — one row each.
    let (router, engine) = emulator();
    let pool_id = seed_pool(&engine, &SEEDED, Vec::new(), None);
    let rows: Vec<(&str, Params)> = vec![
        (
            "an empty PoolId",
            vec![p("PoolId", ""), p("Cidr", SEEDED[0])],
        ),
        (
            "an empty Cidr",
            vec![p("PoolId", pool_id.clone()), p("Cidr", "")],
        ),
        (
            "both empty",
            vec![p("PoolId", ""), p("Cidr", "")],
        ),
    ];
    for (case, params) in rows {
        let (status, body) = deprovision(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            MISSING_PARAMETER,
            "case {case:?}: expected {MISSING_PARAMETER}, got {body}"
        );
        let held = stored_cidrs(&engine, &pool_id);
        assert_eq!(
            held.len(),
            SEEDED.len(),
            "case {case:?}: a refused call must remove nothing, got {held:?}"
        );
    }
}

/// RULE 4, the envelope: every refusal carries the request id EC2's error response
/// documents — `<Response><Errors><Error/></Errors><RequestID/></Response>` — and a fresh
/// one per response, so two refusals are distinguishable.
#[tokio::test]
async fn a_refusal_carries_its_own_request_id() {
    let (router, engine) = emulator();
    let pool_id = seed_pool(&engine, &SEEDED, Vec::new(), None);
    // One row per refusal this operation answers, so the id is pinned on the not-found, the
    // bad-value and the dry-run paths rather than on one of them.
    let rows: Vec<(&str, Params)> = vec![
        (
            "a pool the account does not hold",
            vec![
                p("PoolId", "ipv4pool-ec2-00000000000000000"),
                p("Cidr", SEEDED[0]),
            ],
        ),
        (
            "a value that is no CIDR block",
            vec![p("PoolId", pool_id.clone()), p("Cidr", "banana")],
        ),
        (
            "a dry run",
            vec![
                p("PoolId", pool_id.clone()),
                p("Cidr", SEEDED[0]),
                p("DryRun", "true"),
            ],
        ),
    ];
    let mut seen: Vec<String> = Vec::new();
    for (case, params) in rows {
        let (status, body) = deprovision(&router, &params).await;
        assert!(
            status >= 400,
            "case {case:?}: expected a refusal, got {status} with body {body}"
        );
        let request_id = element(&body, "RequestID").unwrap_or_default();
        // The v4 UUID form the success envelope's own id takes: 36 characters, the four
        // hyphens in the 8-4-4-4-12 grouping, and hexadecimal everywhere else.
        assert_eq!(
            request_id.len(),
            36,
            "case {case:?}: expected a 36-character request id, got {request_id:?} in {body}"
        );
        let groups: Vec<&str> = request_id.split('-').collect();
        assert_eq!(
            groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12],
            "case {case:?}: expected the 8-4-4-4-12 grouping, got {request_id:?}"
        );
        assert!(
            groups
                .iter()
                .all(|group| group.chars().all(|c| c.is_ascii_hexdigit())),
            "case {case:?}: expected a hexadecimal request id, got {request_id:?}"
        );
        assert!(
            !seen.contains(&request_id),
            "case {case:?}: each response carries its own id, got {request_id:?} twice"
        );
        seen.push(request_id);
    }
}

/// RULE 2, the availability rule: the addresses the account holds out of a range are not
/// addresses that range still has to give.
#[tokio::test]
async fn an_allocated_address_lowers_the_available_count() {
    // Rows differing in how many of the pool's addresses are allocated, and from which of
    // its ranges.
    let cases: [(&str, Vec<(&str, &str)>); 4] = [
        ("no address allocated", Vec::new()),
        (
            "one address out of the first range",
            vec![("eipalloc-0000000000000001", "192.0.2.5")],
        ),
        (
            "two addresses out of one range",
            vec![
                ("eipalloc-0000000000000001", "192.0.2.5"),
                ("eipalloc-0000000000000002", "192.0.2.200"),
            ],
        ),
        (
            "one address out of each of two ranges",
            vec![
                ("eipalloc-0000000000000001", "192.0.2.5"),
                ("eipalloc-0000000000000003", "198.18.1.7"),
            ],
        ),
    ];
    for (case, allocated) in cases {
        let (router, engine) = emulator();
        let pool_id = seed_pool(&engine, &SEEDED, Vec::new(), None);
        for (allocation_id, public_ip) in &allocated {
            seed_address(&engine, allocation_id, public_ip);
        }
        // An address OUTSIDE every range of the pool never lowers a count.
        seed_address(&engine, "eipalloc-000000000000000f", "198.51.100.9");

        let described = described_pool(&router, case, &pool_id).await;
        let total: u64 = SEEDED.iter().map(|cidr| block_bounds(cidr).2).sum();
        let expected_available = total - allocated.len() as u64;
        let answered = element(&described, "totalAvailableAddressCount").unwrap_or_default();
        assert_eq!(
            answered,
            expected_available.to_string(),
            "case {case:?}: expected {expected_available} of {total} addresses available, got \
             {answered:?} in {described}"
        );
        assert_eq!(
            element(&described, "totalAddressCount").unwrap_or_default(),
            total.to_string(),
            "case {case:?}: an allocation does not change how many addresses the pool has, \
             got {described}"
        );
    }
}

/// RULE 5: the describe reports the store, narrowed by `PoolIds` and the documented
/// filters, in pool-id order.
#[tokio::test]
async fn the_describe_reports_the_store_narrowed_and_filtered() {
    let (router, engine) = emulator();
    // Three pools differing in the tags they carry — none, one, two — which is what the two
    // documented filters select on.
    let first = seed_pool(&engine, &["192.0.2.0/24"], Vec::new(), None);
    let second = seed_pool(
        &engine,
        &["198.18.0.0/23"],
        vec![tag("Owner", "TeamA")],
        Some("the second"),
    );
    let third = seed_pool(
        &engine,
        &["203.0.113.0/26"],
        vec![tag("Owner", "TeamB"), tag("Tier", "prod")],
        None,
    );
    let mut all = vec![first.clone(), second.clone(), third.clone()];
    all.sort();

    // Every pool, in id order, and the narrowings of it.
    let cases: Vec<(&str, Params, Vec<String>)> = vec![
        ("every pool", Vec::new(), all.clone()),
        (
            "one id",
            indexed("PoolId", &[second.as_str()]),
            vec![second.clone()],
        ),
        (
            "two ids",
            indexed("PoolId", &[third.as_str(), first.as_str()]),
            {
                let mut want = vec![first.clone(), third.clone()];
                want.sort();
                want
            },
        ),
        (
            "a tag filter on key and value",
            filters(&[("tag:Owner", &["TeamA"])]),
            vec![second.clone()],
        ),
        (
            "a tag filter with two values",
            filters(&[("tag:Owner", &["TeamA", "TeamB"])]),
            {
                let mut want = vec![second.clone(), third.clone()];
                want.sort();
                want
            },
        ),
        (
            "a tag-key filter",
            filters(&[("tag-key", &["Tier"])]),
            vec![third.clone()],
        ),
        (
            "two filters, joined with AND",
            filters(&[("tag-key", &["Owner"]), ("tag:Tier", &["prod"])]),
            vec![third.clone()],
        ),
        (
            "a tag value nothing carries",
            filters(&[("tag:Owner", &["TeamC"])]),
            Vec::new(),
        ),
    ];
    for (case, params, expected) in cases {
        let reported = described_ids(&router, case, &params).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    // A filter name the member does not document is refused rather than read as "no
    // results" — including one a SIBLING describe documents.
    for (case, name) in [
        ("a filter of another describe", "coip-pool.pool-id"),
        ("a plausible but undocumented filter", "pool-id"),
        ("a tag prefix with no key", "tag:"),
    ] {
        let (status, body) = describe(&router, &filters(&[(name, &["anything"])])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PARAMETER_VALUE,
            "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {body}"
        );
    }
}

/// RULE 5, the paging: each published page size cuts the answer into pages of that many
/// pools in id order, and only a page with a pool after it carries a token.
#[tokio::test]
async fn the_describe_pages_in_pool_id_order() {
    let (router, engine) = emulator();
    let mut all = Vec::new();
    for cidr in ["192.0.2.0/24", "198.18.0.0/23", "203.0.113.0/26"] {
        all.push(seed_pool(&engine, &[cidr], Vec::new(), None));
    }
    all.sort();

    // `PoolMaxResults` publishes `range 1..10`: both bounds, and the sizes between that
    // change how many pages there are.
    for page_size in [1usize, 2, 3, 10] {
        let case = format!("MaxResults {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut params = vec![p("MaxResults", page_size.to_string())];
            if let Some(next) = token.as_deref() {
                params.push(p("NextToken", next));
            }
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, "poolId");
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} pools, got {page:?}"
            );
            seen.extend(page);
            token = element(&body, "nextToken");
            if token.is_none() {
                break;
            }
            assert!(
                seen.len() <= all.len(),
                "case {case:?}: paging must terminate, got {seen:?}"
            );
        }
        assert_eq!(
            seen, all,
            "case {case:?}: expected every pool once, in id order, got {seen:?}"
        );
    }

    // A token this service did not issue is refused rather than read as a position.
    for (case, token) in [
        ("text", "not-a-token"),
        ("an id", all[0].as_str()),
        ("an empty token", ""),
    ] {
        let (status, body) = describe(&router, &[p("NextToken", token)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PAGINATION_TOKEN,
            "case {case:?}: expected {INVALID_PAGINATION_TOKEN}, got {body}"
        );
    }
}
