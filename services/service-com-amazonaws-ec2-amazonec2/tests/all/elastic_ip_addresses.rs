// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Elastic IP address slice: `AllocateAddress` and
//! `DescribeAddresses`, driven over the wire the service actually speaks — `ec2Query`,
//! through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. `AllocateAddress` MINTS AN ADDRESS OUT OF AMAZON'S POOL and persists it, so
//!    `DescribeAddresses` reports the STORED record: every member the service owns (the
//!    allocation id, the address, the pool, the network border group, the domain) is
//!    derived rather than taken from the request, the caller's tags round-trip, and two
//!    allocations never share an address;
//! 2. a request naming a source the account holds no pool for, a network border group the
//!    Region does not advertise from, a network other than `vpc`, or an address it cannot
//!    recover is refused with EC2's own error code and stores NOTHING;
//! 3. `DescribeAddresses` is a REPORT: it narrows by `AllocationIds`, `PublicIps` and each
//!    documented filter, refuses an id or an address the account does not hold, and two
//!    identical calls answer the same thing.
//!
//! Each table row is a distinct input along what the rule generalises over — how many tags
//! and of what shape, which address source, which border group, which filter, how many
//! addresses — so no row is the specification.

use std::collections::BTreeSet;
use std::net::Ipv4Addr;

use service_com_amazonaws_ec2_amazonec2::state::{
    ADDRESS_DOMAIN_VPC, AMAZON_ADDRESS_POOL, MAX_TAGS_PER_RESOURCE,
    network_border_group_of_region,
};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, filters, indexed};

/// The Region every call here is signed for; the network border group the assertions
/// expect is derived from it.
const REGION: &str = "us-west-2";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One row of a table that expects a refusal: `(case, the parameters, the status, the
/// error code)`.
type RefusalRow = (&'static str, Params, u16, &'static str);

/// One row of the allocate table: `(case, the parameters, the tags that must round-trip)`.
type AllocateRow = (&'static str, Params, Vec<(&'static str, &'static str)>);

/// One row of the describe table: `(case, the narrowing parameters, the ids it selects)`.
type SelectionRow = (&'static str, Params, Vec<String>);

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

async fn allocate(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "AllocateAddress", params).await
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeAddresses", params).await
}

/// An `AllocationId` is `eipalloc-` followed by 8 or 17 lowercase hex digits.
fn is_allocation_id(candidate: &str) -> bool {
    let Some(suffix) = candidate.strip_prefix("eipalloc-") else {
        return false;
    };
    matches!(suffix.len(), 8 | 17) && suffix.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Allocate one address and answer `(allocation id, public ip)`, failing the case when the
/// call did not succeed.
async fn allocate_one(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> (String, String) {
    let (status, body) = allocate(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from AllocateAddress, got {status} with body {body}"
    );
    let allocation_id = element(&body, "allocationId").unwrap_or_default();
    let public_ip = element(&body, "publicIp").unwrap_or_default();
    assert!(
        is_allocation_id(&allocation_id),
        "case {case:?}: expected an eipalloc- allocation id, got {allocation_id:?}"
    );
    (allocation_id, public_ip)
}

/// The allocation ids `DescribeAddresses` reports for `params`, in the order it reported
/// them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeAddresses, got {status} with body {body}"
    );
    elements(&body, "allocationId")
}

/// The form parameters for one `TagSpecification` over `tags`.
fn tag_specification(resource_type: &str, tags: &[(&str, &str)]) -> Params {
    let mut params = vec![p("TagSpecification.1.ResourceType", resource_type)];
    for (index, (key, value)) in tags.iter().enumerate() {
        params.push(p(
            &format!("TagSpecification.1.Tag.{}.Key", index + 1),
            *key,
        ));
        params.push(p(
            &format!("TagSpecification.1.Tag.{}.Value", index + 1),
            *value,
        ));
    }
    params
}

/// RULE 1: whatever the request carries, the allocation's own members are the service's:
/// the address is an Amazon-pool address of the signed Region, the domain is `vpc`, and
/// `DescribeAddresses` reports exactly what the allocate answered, tags and all.
#[tokio::test]
async fn allocate_address_mints_the_address_and_describe_reports_the_store() {
    // Rows differing along the one thing a successful allocate request can vary: the tags
    // it carries (none, one, several, an empty value) and whether it names the Region's own
    // border group or leaves it to the service.
    let rows: Vec<AllocateRow> = vec![
        ("no parameters at all", Vec::new(), Vec::new()),
        (
            "the documented domain",
            vec![p("Domain", ADDRESS_DOMAIN_VPC)],
            Vec::new(),
        ),
        (
            "the Region's own network border group",
            vec![p("NetworkBorderGroup", network_border_group_of_region(REGION))],
            Vec::new(),
        ),
        (
            "one tag",
            tag_specification("elastic-ip", &[("Name", "edge")]),
            vec![("Name", "edge")],
        ),
        (
            "three tags, one with an empty value",
            tag_specification(
                "elastic-ip",
                &[("Name", "edge"), ("Owner", "TeamA"), ("empty", "")],
            ),
            vec![("Name", "edge"), ("Owner", "TeamA"), ("empty", "")],
        ),
        (
            "the documented domain beside tags",
            {
                let mut params = vec![p("Domain", ADDRESS_DOMAIN_VPC)];
                params.extend(tag_specification("elastic-ip", &[("Tier", "web")]));
                params
            },
            vec![("Tier", "web")],
        ),
    ];

    let border_group = network_border_group_of_region(REGION);
    for (case, params, expected_tags) in rows {
        let router = emulator();
        let (status, body) = allocate(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        let allocation_id = element(&body, "allocationId").unwrap_or_default();
        assert!(
            is_allocation_id(&allocation_id),
            "case {case:?}: expected an eipalloc- allocation id, got {allocation_id:?}"
        );
        let public_ip = element(&body, "publicIp").unwrap_or_default();
        let parsed = public_ip.parse::<Ipv4Addr>();
        assert!(
            parsed.is_ok(),
            "case {case:?}: expected an IPv4 Elastic IP address, got {public_ip:?}"
        );
        let octets = parsed.expect("an IPv4 address").octets();
        assert!(
            !matches!(octets[3], 0 | 255),
            "case {case:?}: an Elastic IP address is a host address, got {public_ip:?}"
        );
        assert_eq!(
            element(&body, "domain").as_deref(),
            Some(ADDRESS_DOMAIN_VPC),
            "case {case:?}: expected the vpc domain, got {body}"
        );
        assert_eq!(
            element(&body, "publicIpv4Pool").as_deref(),
            Some(AMAZON_ADDRESS_POOL),
            "case {case:?}: expected Amazon's own pool, got {body}"
        );
        assert_eq!(
            element(&body, "networkBorderGroup").as_deref(),
            Some(border_group.as_str()),
            "case {case:?}: expected the signed Region's border group, got {body}"
        );

        // The round trip: the stored record a later caller sees carries the same members.
        let (status, described) = describe(&router, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeAddresses, got {status} / {described}"
        );
        assert_eq!(
            elements(&described, "allocationId"),
            vec![allocation_id.clone()],
            "case {case:?}: expected the allocated address to be reported, got {described}"
        );
        assert_eq!(
            element(&described, "publicIp").as_deref(),
            Some(public_ip.as_str()),
            "case {case:?}: expected the stored address, got {described}"
        );
        assert_eq!(
            element(&described, "domain").as_deref(),
            Some(ADDRESS_DOMAIN_VPC),
            "case {case:?}: expected the stored domain, got {described}"
        );
        assert_eq!(
            element(&described, "networkBorderGroup").as_deref(),
            Some(border_group.as_str()),
            "case {case:?}: expected the stored border group, got {described}"
        );
        let reported_keys = elements(&described, "key");
        let expected_keys: Vec<String> = expected_tags
            .iter()
            .map(|(key, _)| (*key).to_string())
            .collect();
        assert_eq!(
            reported_keys, expected_keys,
            "case {case:?}: expected the stored tag keys, got {described}"
        );
        let reported_values = elements(&described, "value");
        let expected_values: Vec<String> = expected_tags
            .iter()
            .map(|(_, value)| (*value).to_string())
            .collect();
        assert_eq!(
            reported_values, expected_values,
            "case {case:?}: expected the stored tag values, got {described}"
        );

        // A fresh address is associated with nothing: `AssociateAddress` is what would give
        // it an association, an instance or a private address.
        for absent in [
            "associationId",
            "instanceId",
            "networkInterfaceId",
            "privateIpAddress",
            "carrierIp",
            "customerOwnedIp",
        ] {
            assert_eq!(
                element(&described, absent),
                None,
                "case {case:?}: expected no {absent} on a freshly allocated address, \
                 got {described}"
            );
        }
    }
}

/// RULE 1, as an INVARIANT over a count: however many addresses one account allocates,
/// each gets its own allocation id and its own address, and the store reports all of them
/// in allocation-id order.
#[tokio::test]
async fn every_allocation_gets_its_own_address() {
    // A bounded number of allocations, well above the one the issue's recording made and
    // above the smallest published Elastic IP quota, so the mint is exercised past any
    // single-row case.
    const ALLOCATIONS: usize = 24;

    let router = emulator();
    let mut ids: Vec<String> = Vec::new();
    let mut addresses: BTreeSet<String> = BTreeSet::new();
    for nth in 0..ALLOCATIONS {
        let case = format!("allocation {nth}");
        let (allocation_id, public_ip) = allocate_one(&router, &case, &[]).await;
        assert!(
            addresses.insert(public_ip.clone()),
            "case {case:?}: address {public_ip} was already held by another allocation"
        );
        assert!(
            !ids.contains(&allocation_id),
            "case {case:?}: allocation id {allocation_id} was minted twice"
        );
        ids.push(allocation_id);
    }

    let reported = described_ids(&router, "every allocation", &[]).await;
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(
        reported, sorted,
        "case \"every allocation\": expected every allocation in id order, got {reported:?}"
    );

    // Every address is reported back, exactly once each.
    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "case \"every allocation\": expected HTTP 200, got {status} with body {body}"
    );
    let reported_addresses: BTreeSet<String> = elements(&body, "publicIp").into_iter().collect();
    assert_eq!(
        reported_addresses, addresses,
        "case \"every allocation\": expected the minted addresses, got {reported_addresses:?}"
    );
}

/// RULE 2: every request this service cannot satisfy is refused with EC2's own code, and
/// a refusal stores nothing.
#[tokio::test]
async fn allocate_address_refuses_what_it_cannot_allocate() {
    let other_region_border_group = network_border_group_of_region("eu-central-1");
    let mut too_many_tags = vec![p("TagSpecification.1.ResourceType", "elastic-ip")];
    for nth in 0..=MAX_TAGS_PER_RESOURCE {
        too_many_tags.push(p(
            &format!("TagSpecification.1.Tag.{}.Key", nth + 1),
            format!("k{nth}"),
        ));
        too_many_tags.push(p(
            &format!("TagSpecification.1.Tag.{}.Value", nth + 1),
            format!("v{nth}"),
        ));
    }

    // Rows differing along WHICH parameter the service cannot satisfy: the network, the
    // border group, each of the three address pools, two sources at once, an address that
    // cannot be recovered, a malformed address, and each tag rule.
    let rows: Vec<RefusalRow> = vec![
        (
            "the retired EC2-Classic network",
            vec![p("Domain", "standard")],
            400,
            "InvalidParameterValue",
        ),
        (
            "another Region's network border group",
            vec![p("NetworkBorderGroup", other_region_border_group.clone())],
            400,
            "InvalidParameterValue",
        ),
        (
            "a Wavelength Zone border group",
            vec![p("NetworkBorderGroup", "us-west-2-wl1-las-wlz-1")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a public IPv4 pool the account does not own",
            vec![p("PublicIpv4Pool", "ipv4pool-ec2-0123456789abcdef0")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a customer-owned pool with no Outpost behind it",
            vec![p("CustomerOwnedIpv4Pool", "ipv4pool-coip-0123456789abcdef0")],
            400,
            "InvalidParameterValue",
        ),
        (
            "an IPAM pool the account does not have",
            vec![p("IpamPoolId", "ipam-pool-0123456789abcdef0")],
            400,
            "InvalidParameterValue",
        ),
        (
            "two address sources at once",
            vec![
                p("Address", "203.0.113.5"),
                p("PublicIpv4Pool", "ipv4pool-ec2-0123456789abcdef0"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "an address that is not an IPv4 address",
            vec![p("Address", "not-an-address")],
            400,
            "InvalidParameterValue",
        ),
        (
            "an address this account never released",
            vec![p("Address", "203.0.113.5")],
            400,
            "InvalidAddress.NotFound",
        ),
        (
            "tagging a resource this call does not create",
            tag_specification("instance", &[("Name", "nope")]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a tag key in the reserved namespace",
            tag_specification("elastic-ip", &[("aws:created-by", "me")]),
            400,
            "InvalidParameterValue",
        ),
        (
            "one tag past the documented quota",
            too_many_tags,
            400,
            "TagLimitExceeded",
        ),
        (
            "a dry run that would have succeeded",
            vec![p("DryRun", "true")],
            412,
            "DryRunOperation",
        ),
        (
            "a dry run of a request that would have been refused",
            vec![p("DryRun", "true"), p("Domain", "standard")],
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, params, expected_status, expected_code) in rows {
        let router = emulator();
        let (status, body) = allocate(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert!(
            described_ids(&router, case, &[]).await.is_empty(),
            "case {case:?}: a refused AllocateAddress must store nothing"
        );
    }
}

/// RULE 2: an address the account still holds is in use, not recoverable — the one `Address`
/// case that needs an address already allocated.
#[tokio::test]
async fn allocate_address_refuses_an_address_already_held() {
    let router = emulator();
    let (allocation_id, public_ip) = allocate_one(&router, "the held address", &[]).await;

    let (status, body) = allocate(&router, &[p("Address", public_ip.clone())]).await;
    assert_eq!(
        status, 400,
        "case \"an address this account still holds\": expected HTTP 400, got {status} / {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidIPAddress.InUse",
        "case \"an address this account still holds\": expected InvalidIPAddress.InUse, \
         got {body}"
    );
    assert_eq!(
        described_ids(&router, "the held address", &[]).await,
        vec![allocation_id],
        "case \"an address this account still holds\": the refusal must store nothing"
    );
}

/// RULE 3: `DescribeAddresses` narrows by `AllocationIds`, by `PublicIps` and by each
/// filter the operation documents.
#[tokio::test]
async fn describe_addresses_narrows_by_id_address_and_filter() {
    let router = emulator();
    let border_group = network_border_group_of_region(REGION);

    // Three addresses differing along what the documented filters select on.
    let (first_id, first_ip) = allocate_one(
        &router,
        "the tagged address",
        &tag_specification("elastic-ip", &[("Name", "edge"), ("Owner", "TeamA")]),
    )
    .await;
    let (second_id, second_ip) = allocate_one(
        &router,
        "the differently tagged address",
        &tag_specification("elastic-ip", &[("Name", "core")]),
    )
    .await;
    let (third_id, third_ip) = allocate_one(&router, "the untagged address", &[]).await;
    let all: Vec<String> = {
        let mut ids = vec![first_id.clone(), second_id.clone(), third_id.clone()];
        ids.sort();
        ids
    };

    // Rows differing along WHICH narrowing is asked for and how it is spelled.
    let rows: Vec<SelectionRow> = vec![
        ("no narrowing at all", Vec::new(), all.clone()),
        (
            "one allocation id",
            indexed("AllocationId", &[second_id.as_str()]),
            vec![second_id.clone()],
        ),
        (
            "two allocation ids",
            indexed("AllocationId", &[first_id.as_str(), third_id.as_str()]),
            {
                let mut ids = vec![first_id.clone(), third_id.clone()];
                ids.sort();
                ids
            },
        ),
        (
            "one public address",
            indexed("PublicIp", &[third_ip.as_str()]),
            vec![third_id.clone()],
        ),
        (
            "an id and an address together",
            {
                let mut params = indexed("AllocationId", &[first_id.as_str()]);
                params.extend(indexed("PublicIp", &[second_ip.as_str()]));
                params
            },
            {
                let mut ids = vec![first_id.clone(), second_id.clone()];
                ids.sort();
                ids
            },
        ),
        (
            "the allocation-id filter",
            filters(&[("allocation-id", &[first_id.as_str()])]),
            vec![first_id.clone()],
        ),
        (
            "the public-ip filter",
            filters(&[("public-ip", &[first_ip.as_str()])]),
            vec![first_id.clone()],
        ),
        (
            "the network-border-group filter",
            filters(&[("network-border-group", &[border_group.as_str()])]),
            all.clone(),
        ),
        (
            "the network-border-group filter on another Region",
            filters(&[("network-border-group", &["eu-central-1"])]),
            Vec::new(),
        ),
        (
            "a tag filter",
            filters(&[("tag:Name", &["edge"])]),
            vec![first_id.clone()],
        ),
        (
            "a tag filter with a wildcard",
            filters(&[("tag:Name", &["*e"])]),
            {
                let mut ids = vec![first_id.clone(), second_id.clone()];
                ids.sort();
                ids
            },
        ),
        (
            "a tag filter on a value nothing carries",
            filters(&[("tag:Name", &["absent"])]),
            Vec::new(),
        ),
        (
            "the tag-key filter",
            filters(&[("tag-key", &["Owner"])]),
            vec![first_id.clone()],
        ),
        (
            "two filters joined with AND",
            filters(&[("tag-key", &["Name"]), ("allocation-id", &[second_id.as_str()])]),
            vec![second_id.clone()],
        ),
        (
            "one filter with two values, joined with OR",
            filters(&[("tag:Name", &["edge", "core"])]),
            {
                let mut ids = vec![first_id.clone(), second_id.clone()];
                ids.sort();
                ids
            },
        ),
        (
            "an association filter, nothing being associated",
            filters(&[("instance-id", &["i-0123456789abcdef0"])]),
            Vec::new(),
        ),
        (
            "an id narrowing beside a filter that excludes it",
            {
                let mut params = indexed("AllocationId", &[third_id.as_str()]);
                params.extend(filters(&[("tag-key", &["Name"])]));
                params
            },
            Vec::new(),
        ),
    ];

    for (case, params, expected) in rows {
        let reported = described_ids(&router, case, &params).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    // Reading never changes what a later caller sees: the same request answers the same
    // thing, and every address is still there.
    let first_pass = described_ids(&router, "the first read", &[]).await;
    let second_pass = described_ids(&router, "the second read", &[]).await;
    assert_eq!(
        first_pass, second_pass,
        "case \"two identical reads\": expected {first_pass:?}, got {second_pass:?}"
    );
    assert_eq!(
        second_pass, all,
        "case \"two identical reads\": expected every stored address, got {second_pass:?}"
    );
    assert_ne!(
        first_ip, second_ip,
        "case \"two allocations\": expected distinct addresses, got {first_ip} twice"
    );
}

/// RULE 3: an allocation id or an address the account does not hold is an error, not an
/// absent row — and a malformed id is refused before an unknown one.
#[tokio::test]
async fn describe_addresses_refuses_what_the_account_does_not_hold() {
    let router = emulator();
    let (allocation_id, _) = allocate_one(&router, "the stored address", &[]).await;

    let rows: Vec<RefusalRow> = vec![
        (
            "an allocation id of the right form that names nothing",
            indexed("AllocationId", &["eipalloc-0123456789abcdef0"]),
            400,
            "InvalidAllocationID.NotFound",
        ),
        (
            "an allocation id that is not one",
            indexed("AllocationId", &["eipalloc-nothex"]),
            400,
            "InvalidAllocationID.Malformed",
        ),
        (
            "an id of another resource kind",
            indexed("AllocationId", &["vol-0123456789abcdef0"]),
            400,
            "InvalidAllocationID.Malformed",
        ),
        (
            "a stored id beside an unknown one",
            indexed(
                "AllocationId",
                &[allocation_id.as_str(), "eipalloc-0123456789abcdef0"],
            ),
            400,
            "InvalidAllocationID.NotFound",
        ),
        (
            "an address the account does not hold",
            indexed("PublicIp", &["203.0.113.9"]),
            400,
            "InvalidAddress.NotFound",
        ),
        (
            "an address that is not an IPv4 address",
            indexed("PublicIp", &["not-an-address"]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a filter this operation does not document",
            filters(&[("allocation-ids", &["eipalloc-0123456789abcdef0"])]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a tag filter with no key",
            filters(&[("tag:", &["edge"])]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a dry run that would have succeeded",
            vec![p("DryRun", "true")],
            412,
            "DryRunOperation",
        ),
        (
            "a dry run of a request that would have been refused",
            {
                let mut params = vec![p("DryRun", "true")];
                params.extend(indexed("PublicIp", &["203.0.113.9"]));
                params
            },
            400,
            "InvalidAddress.NotFound",
        ),
    ];

    for (case, params, expected_status, expected_code) in rows {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // None of those refusals touched the store.
    assert_eq!(
        described_ids(&router, "after every refusal", &[]).await,
        vec![allocation_id],
        "case \"after every refusal\": expected the one stored address to be untouched"
    );
}
