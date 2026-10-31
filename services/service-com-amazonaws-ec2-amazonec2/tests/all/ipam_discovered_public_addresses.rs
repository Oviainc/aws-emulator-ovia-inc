// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetIpamDiscoveredPublicAddresses`, driven over the wire the service
//! actually speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. the sample a resource discovery reports for a Region is DERIVED from the account's
//!    stored public addresses: every Elastic IP address the account holds in that Region
//!    appears exactly once, carrying what the store says about it — the allocation it belongs
//!    to, whether a resource holds it, the resource holding it (the instance, its VPC, its
//!    subnet, its primary network interface and the groups around it), its Elastic IP tags,
//!    its network border group, the pool it came from and the instant the sample was taken —
//!    so the report and `DescribeAddresses` cannot disagree about one address;
//! 2. the sample is of the REGION and of that DISCOVERY: an address advertised from another
//!    Region's network border group is not in it, and a Region the discovery does not operate
//!    in has no sample at all rather than another Region's;
//! 3. `Filters` narrow the sample by any member of the reported address (and a filter name
//!    the operation does not resolve is refused rather than read as "no results"), and
//!    `MaxResults` pages it in address order, the `NextToken` of one page resuming the walk
//!    without repeating or dropping an address;
//! 4. a request naming something that is not a resource discovery id, a well-formed id the
//!    account holds no discovery for, or a `NextToken` this service did not issue is refused
//!    with EC2's own code, and `DryRun` on a request that would have succeeded reports that.
//!
//! Each table row is a distinct input along what those rules generalise over — how many
//! addresses the account holds, whether an address is tagged and whether a resource holds it,
//! which Region is asked about and which the discovery operates in, which member a filter
//! names and in what form, how large a page is asked for, and which member of the request is
//! at fault — so no row is the specification.

use std::collections::BTreeSet;

use service_com_amazonaws_ec2_amazonec2::state::{
    DRY_RUN_OPERATION, INVALID_IPAM_RESOURCE_DISCOVERY_ID_MALFORMED,
    INVALID_IPAM_RESOURCE_DISCOVERY_ID_NOT_FOUND, INVALID_PAGINATION_TOKEN,
    INVALID_PARAMETER_VALUE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query;
use super::wire::{child, child_text, document, Param};

/// The Region every call here is signed for, and the home Region of each IPAM it creates.
const REGION: &str = "us-west-2";

/// A second Region of the same partition, for the addresses and the sample requests that are
/// NOT the home Region's.
const OTHER_REGION: &str = "us-east-1";

/// The image every launch here uses; `RunInstances` only needs one that resolves.
const AMI: &str = "ami-0123456789abcdef0";

/// The action under test.
const ACTION: &str = "GetIpamDiscoveredPublicAddresses";

/// The `IpamPublicAddressType` every address this suite allocates reports: it comes out of
/// Amazon's own pool, and the member is from the modelled catalog
/// (`smithy_query(query="shape",shape="IpamPublicAddressType")`).
const AMAZON_OWNED_EIP: &str = "amazon-owned-eip";

/// The two `IpamPublicAddressAssociationStatus` members, from the modelled catalog
/// (`smithy_query(query="shape",shape="IpamPublicAddressAssociationStatus")`).
const ASSOCIATED: &str = "associated";
const DISASSOCIATED: &str = "disassociated";

fn emulator() -> MultiServiceRouter {
    ec2_query::emulator().0
}

/// One form field: `("AddressRegion", "us-west-2")` as the call helpers want it.
fn p(key: &str, value: impl Into<String>) -> Param {
    (key.to_string(), value.into())
}

/// One call of `action` signed for `region`.
async fn api(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[Param],
) -> (u16, String) {
    ec2_query::call(router, region, action, params).await
}

/// One call of `action` signed for [`REGION`] that must succeed, as the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = api(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// Create one IPAM operating in `regions` beside its home Region, and answer the id of the
/// default resource discovery that came up with it: "the default resource discovery is the
/// resource discovery automatically created when you create an IPAM"
/// (`IpamResourceDiscovery$IsDefault`).
async fn default_discovery(case: &str, router: &MultiServiceRouter, regions: &[&str]) -> String {
    let params: Vec<Param> = regions
        .iter()
        .enumerate()
        .map(|(index, region)| p(&format!("OperatingRegion.{}.RegionName", index + 1), *region))
        .collect();
    let body = ok(case, router, "CreateIpam", &params).await;
    ec2_query::element(&body, "defaultResourceDiscoveryId").unwrap_or_else(|| {
        panic!("case {case:?}: CreateIpam must report the default resource discovery: {body}")
    })
}

/// Allocate one Elastic IP address in `region`, with `tags` on it, and answer
/// `(allocation id, public ip)`.
async fn allocate(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    tags: &[(&str, &str)],
) -> (String, String) {
    let mut params = Vec::new();
    if !tags.is_empty() {
        params.push(p("TagSpecification.1.ResourceType", "elastic-ip"));
        for (index, (key, value)) in tags.iter().enumerate() {
            params.push(p(&format!("TagSpecification.1.Tag.{}.Key", index + 1), *key));
            params.push(p(
                &format!("TagSpecification.1.Tag.{}.Value", index + 1),
                *value,
            ));
        }
    }
    let (status, body) = api(router, region, "AllocateAddress", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: AllocateAddress in {region} must succeed, got {status}: {body}"
    );
    let allocation_id = ec2_query::element(&body, "allocationId").unwrap_or_else(|| {
        panic!("case {case:?}: AllocateAddress must mint an allocation id: {body}")
    });
    let public_ip = ec2_query::element(&body, "publicIp")
        .unwrap_or_else(|| panic!("case {case:?}: AllocateAddress must mint an address: {body}"));
    (allocation_id, public_ip)
}

/// Launch one instance in [`REGION`] and answer its id.
async fn launch(case: &str, router: &MultiServiceRouter) -> String {
    let body = ok(
        case,
        router,
        "RunInstances",
        &[p("ImageId", AMI), p("MinCount", "1"), p("MaxCount", "1")],
    )
    .await;
    ec2_query::element(&body, "instanceId")
        .unwrap_or_else(|| panic!("case {case:?}: RunInstances must report the instance: {body}"))
}

/// Associate `allocation_id` with `instance_id`.
async fn associate(case: &str, router: &MultiServiceRouter, allocation_id: &str, instance_id: &str) {
    ok(
        case,
        router,
        "AssociateAddress",
        &[
            p("AllocationId", allocation_id),
            p("InstanceId", instance_id),
        ],
    )
    .await;
}

/// The members of one discovered public address the assertions below read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Discovered {
    address: String,
    allocation_id: Option<String>,
    owner_id: Option<String>,
    region: Option<String>,
    discovery_id: Option<String>,
    association_status: Option<String>,
    address_type: Option<String>,
    service: Option<String>,
    service_resource: Option<String>,
    instance_id: Option<String>,
    vpc_id: Option<String>,
    subnet_id: Option<String>,
    network_interface_id: Option<String>,
    public_ipv4_pool_id: Option<String>,
    network_border_group: Option<String>,
    security_group_ids: Vec<String>,
    eip_tags: Vec<(String, String)>,
    sample_time: Option<String>,
}

/// Every address of one `GetIpamDiscoveredPublicAddresses` response, in the order it reported
/// them.
///
/// Read through the XML parser rather than by substring, so a member of a nested tag or
/// security-group item is never mistaken for one of the address's own.
fn discovered(case: &str, body: &str) -> Vec<Discovered> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .filter(|node| child(*node, "address").is_some())
        .map(|item| Discovered {
            address: child_text(item, "address").unwrap_or_default(),
            allocation_id: child_text(item, "addressAllocationId"),
            owner_id: child_text(item, "addressOwnerId"),
            region: child_text(item, "addressRegion"),
            discovery_id: child_text(item, "ipamResourceDiscoveryId"),
            association_status: child_text(item, "associationStatus"),
            address_type: child_text(item, "addressType"),
            service: child_text(item, "service"),
            service_resource: child_text(item, "serviceResource"),
            instance_id: child_text(item, "instanceId"),
            vpc_id: child_text(item, "vpcId"),
            subnet_id: child_text(item, "subnetId"),
            network_interface_id: child_text(item, "networkInterfaceId"),
            public_ipv4_pool_id: child_text(item, "publicIpv4PoolId"),
            network_border_group: child_text(item, "networkBorderGroup"),
            security_group_ids: child(item, "securityGroupSet")
                .map(|set| {
                    set.children()
                        .filter(|node| node.is_element())
                        .filter_map(|group| child_text(group, "groupId"))
                        .collect()
                })
                .unwrap_or_default(),
            eip_tags: child(item, "tags")
                .and_then(|tags| child(tags, "eipTagSet"))
                .map(|set| {
                    set.children()
                        .filter(|node| node.is_element())
                        .map(|tag| {
                            (
                                child_text(tag, "key").unwrap_or_default(),
                                child_text(tag, "value").unwrap_or_default(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
            sample_time: child_text(item, "sampleTime"),
        })
        .collect()
}

/// The addresses of one response, sorted — the set a row's expectation names.
fn reported_addresses(case: &str, body: &str) -> Vec<String> {
    let mut addresses: Vec<String> = discovered(case, body)
        .into_iter()
        .map(|found| found.address)
        .collect();
    addresses.sort();
    addresses
}

/// One sample request for `region` through `discovery`, with `extra` beside it.
fn sample_params(discovery: &str, region: &str, extra: Vec<Param>) -> Vec<Param> {
    let mut params = vec![
        p("IpamResourceDiscoveryId", discovery),
        p("AddressRegion", region),
    ];
    params.extend(extra);
    params
}

// ---------------------------------------------------------------------------
// What the sample reports
// ---------------------------------------------------------------------------

/// One row of the table below: the case label, the tags the address is allocated with, and
/// whether a resource of the account holds it.
type AddressRow = (
    &'static str,
    &'static [(&'static str, &'static str)],
    bool,
);

/// One address a row of that table allocated, as the assertions read it back.
struct Allocated {
    /// The row's case label, named by every assertion about this address.
    row: &'static str,
    allocation_id: String,
    public_ip: String,
    attached: bool,
    tags: Vec<(String, String)>,
}

/// Rule 1: every Elastic IP address the account holds in the Region is in the sample exactly
/// once, carrying what the STORE says about it.
///
/// The rows differ along what that generalises over: how many tags the address carries (none,
/// one, two) and whether a resource of the account holds it.
#[tokio::test]
async fn sample_reports_every_elastic_ip_of_the_region_with_what_the_store_holds() {
    let router = emulator();
    let case = "three addresses, one tagged twice, one on an instance";
    let discovery = default_discovery(case, &router, &[]).await;

    let rows: [AddressRow; 3] = [
        ("untagged and unattached", &[], false),
        (
            "two tags, unattached",
            &[("Name", "edge"), ("tier", "1")],
            false,
        ),
        ("one tag, on an instance", &[("Name", "gateway")], true),
    ];

    let instance = launch(case, &router).await;
    let mut expected: Vec<Allocated> = Vec::new();
    for (row, tags, attach) in rows {
        let (allocation_id, public_ip) = allocate(row, &router, REGION, tags).await;
        if attach {
            associate(row, &router, &allocation_id, &instance).await;
        }
        expected.push(Allocated {
            row,
            allocation_id,
            public_ip,
            attached: attach,
            tags: tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
        });
    }

    let body = ok(
        case,
        &router,
        ACTION,
        &sample_params(&discovery, REGION, Vec::new()),
    )
    .await;
    let found = discovered(case, &body);
    assert_eq!(
        found.len(),
        expected.len(),
        "case {case:?}: expected {} discovered addresses, got {found:?} in {body}",
        expected.len()
    );

    let account = engine::emulator_account_id().to_string();
    for Allocated {
        row,
        allocation_id,
        public_ip,
        attached,
        tags,
    } in &expected
    {
        let entry = found
            .iter()
            .find(|entry| entry.address == *public_ip)
            .unwrap_or_else(|| {
                panic!("case {row:?}: expected the address {public_ip} in {found:?}")
            });
        assert_eq!(
            entry.allocation_id.as_deref(),
            Some(allocation_id.as_str()),
            "case {row:?}: expected the allocation {allocation_id}, got {entry:?}"
        );
        assert_eq!(
            entry.owner_id.as_deref(),
            Some(account.as_str()),
            "case {row:?}: expected the owning account {account}, got {entry:?}"
        );
        assert_eq!(
            entry.region.as_deref(),
            Some(REGION),
            "case {row:?}: expected the address Region {REGION}, got {entry:?}"
        );
        assert_eq!(
            entry.discovery_id.as_deref(),
            Some(discovery.as_str()),
            "case {row:?}: expected the discovery {discovery}, got {entry:?}"
        );
        assert_eq!(
            entry.address_type.as_deref(),
            Some(AMAZON_OWNED_EIP),
            "case {row:?}: expected the address type {AMAZON_OWNED_EIP}, got {entry:?}"
        );
        // "The ID of the public IPv4 pool that the resource with the assigned IP address is
        // from": an address out of Amazon's pool is from no pool of the account.
        assert_eq!(
            entry.public_ipv4_pool_id, None,
            "case {row:?}: an Amazon-pool address names no public IPv4 pool, got {entry:?}"
        );
        assert_eq!(
            entry.network_border_group.as_deref(),
            Some(REGION),
            "case {row:?}: expected the Region's network border group, got {entry:?}"
        );
        assert_eq!(
            &entry.eip_tags, tags,
            "case {row:?}: expected the Elastic IP tags {tags:?}, got {entry:?}"
        );
        // Nothing in this account is a NAT gateway or any other service-managed holder, so
        // no address of it names a service.
        assert_eq!(
            (entry.service.as_deref(), entry.service_resource.as_deref()),
            (None, None),
            "case {row:?}: no service holds this address, got {entry:?}"
        );
        let sample_time = entry
            .sample_time
            .as_deref()
            .unwrap_or_else(|| panic!("case {row:?}: expected a sample time, got {entry:?}"));
        assert!(
            ec2_query::as_timestamp(sample_time).is_some(),
            "case {row:?}: expected an ISO 8601 sample time, got {sample_time:?}"
        );

        if *attached {
            assert_eq!(
                entry.association_status.as_deref(),
                Some(ASSOCIATED),
                "case {row:?}: expected the status {ASSOCIATED}, got {entry:?}"
            );
            assert_eq!(
                entry.instance_id.as_deref(),
                Some(instance.as_str()),
                "case {row:?}: expected the instance {instance}, got {entry:?}"
            );
            // Where the holder sits and what surrounds it, read off the resource rather than
            // invented: the instance's own VPC, subnet, primary interface and groups.
            for (member, value) in [
                ("vpcId", entry.vpc_id.as_deref()),
                ("subnetId", entry.subnet_id.as_deref()),
                ("networkInterfaceId", entry.network_interface_id.as_deref()),
            ] {
                assert!(
                    value.is_some_and(|value| !value.is_empty()),
                    "case {row:?}: expected a {member} from the holding instance, got {entry:?}"
                );
            }
            assert!(
                !entry.security_group_ids.is_empty(),
                "case {row:?}: expected the holder's security groups, got {entry:?}"
            );
        } else {
            assert_eq!(
                entry.association_status.as_deref(),
                Some(DISASSOCIATED),
                "case {row:?}: expected the status {DISASSOCIATED}, got {entry:?}"
            );
            assert_eq!(
                (
                    entry.instance_id.as_deref(),
                    entry.vpc_id.as_deref(),
                    entry.subnet_id.as_deref(),
                    entry.network_interface_id.as_deref()
                ),
                (None, None, None, None),
                "case {row:?}: an address nothing holds names no holder, got {entry:?}"
            );
            assert!(
                entry.security_group_ids.is_empty(),
                "case {row:?}: an address nothing holds names no group, got {entry:?}"
            );
        }
    }

    // The report is of the SAME addresses `DescribeAddresses` reports: one store, two reads.
    let described = ok(case, &router, "DescribeAddresses", &[]).await;
    let described_ids: BTreeSet<String> = ec2_query::elements(&described, "allocationId")
        .into_iter()
        .collect();
    let discovered_ids: BTreeSet<String> = found
        .iter()
        .filter_map(|entry| entry.allocation_id.clone())
        .collect();
    assert_eq!(
        discovered_ids, described_ids,
        "case {case:?}: the sample and DescribeAddresses must report one set of allocations, \
         got {discovered_ids:?} and {described_ids:?}"
    );

    // "The oldest successful resource discovery time": the oldest sample of the addresses
    // reported, so it is no later than any of them.
    let oldest = ec2_query::member(&body, "oldestSampleTime")
        .and_then(|value| ec2_query::as_timestamp(&value))
        .unwrap_or_else(|| panic!("case {case:?}: expected an oldestSampleTime in {body}"));
    for entry in &found {
        let at = entry
            .sample_time
            .as_deref()
            .and_then(ec2_query::as_timestamp)
            .unwrap_or_else(|| panic!("case {case:?}: expected a sample time on {entry:?}"));
        assert!(
            oldest <= at,
            "case {case:?}: expected the oldest sample time {oldest} to be no later than \
             {at}, on {entry:?}"
        );
    }
}

/// Rule 2: the sample is of the Region asked about, through a discovery that operates in it.
///
/// The rows differ along what that generalises over: which Region's addresses are asked for,
/// whether the discovery operates in it, and whether the Region holds an address at all.
#[tokio::test]
async fn sample_is_of_the_region_asked_about_and_only_a_region_the_discovery_operates_in() {
    let router = emulator();
    let case = "one address per Region, a discovery operating in both";
    // An IPAM operating in its home Region and in one other: "operating Regions are Amazon
    // Web Services Regions where the IPAM is allowed to manage IP address CIDRs"
    // (`Ipam$OperatingRegions`).
    let both = default_discovery(case, &router, &[OTHER_REGION]).await;
    // A second IPAM operating in its home Region only, so the other Region is one it does
    // not monitor.
    let home_only = default_discovery(case, &router, &[]).await;

    let (_, here) = allocate(case, &router, REGION, &[]).await;
    let (_, there) = allocate(case, &router, OTHER_REGION, &[]).await;

    // (case, discovery, Region asked about, the addresses expected)
    let rows: [(&str, &str, &str, Vec<String>); 4] = [
        (
            "home Region of a discovery operating in both",
            &both,
            REGION,
            vec![here.clone()],
        ),
        (
            "other Region of a discovery operating in both",
            &both,
            OTHER_REGION,
            vec![there.clone()],
        ),
        (
            "home Region of a discovery operating only there",
            &home_only,
            REGION,
            vec![here.clone()],
        ),
        (
            "a Region the discovery does not operate in",
            &home_only,
            OTHER_REGION,
            Vec::new(),
        ),
    ];

    for (row, discovery, region, mut expected) in rows {
        let body = ok(row, &router, ACTION, &sample_params(discovery, region, Vec::new())).await;
        expected.sort();
        assert_eq!(
            reported_addresses(row, &body),
            expected,
            "case {row:?}: expected the addresses {expected:?} in {body}"
        );
        // A Region with no sampled address has no oldest sample time either.
        let oldest = ec2_query::member(&body, "oldestSampleTime");
        assert_eq!(
            oldest.is_some(),
            !expected.is_empty(),
            "case {row:?}: expected an oldestSampleTime only for a sample that reported an \
             address, got {oldest:?} in {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Narrowing the sample and paging it
// ---------------------------------------------------------------------------

/// Rule 3, first half: a filter selects the addresses whose member it names matches, and a
/// name this operation does not resolve is refused.
///
/// The rows differ along what that generalises over: which member the filter names, whether
/// the value is exact or carries EC2's `*` wildcard, whether it selects one address, several
/// or none, and how a tag is named.
#[tokio::test]
async fn filters_narrow_the_sample_and_an_unresolvable_filter_name_is_refused() {
    let router = emulator();
    let case = "filters over three addresses";
    let discovery = default_discovery(case, &router, &[]).await;
    let instance = launch(case, &router).await;
    let (plain_allocation, plain) = allocate(case, &router, REGION, &[]).await;
    let (_, tagged) = allocate(case, &router, REGION, &[("Name", "edge")]).await;
    let (held_allocation, held) = allocate(case, &router, REGION, &[]).await;
    associate(case, &router, &held_allocation, &instance).await;

    // (case, filter name, filter values, the addresses expected)
    let rows: [(&str, &str, Vec<String>, Vec<String>); 9] = [
        (
            "one address by its own value",
            "address",
            vec![tagged.clone()],
            vec![tagged.clone()],
        ),
        (
            "one address by the allocation holding it",
            "address-allocation-id",
            vec![plain_allocation.clone()],
            vec![plain.clone()],
        ),
        (
            "every address of Amazon's pool",
            "address-type",
            vec![AMAZON_OWNED_EIP.to_string()],
            vec![plain.clone(), tagged.clone(), held.clone()],
        ),
        (
            "the addresses nothing holds",
            "association-status",
            vec![DISASSOCIATED.to_string()],
            vec![plain.clone(), tagged.clone()],
        ),
        (
            "the address an instance holds",
            "instance-id",
            vec![instance.clone()],
            vec![held.clone()],
        ),
        (
            "a tag value named by its key",
            "tag:Name",
            vec!["edge".to_string()],
            vec![tagged.clone()],
        ),
        (
            "every address carrying a tag key",
            "tag-key",
            vec!["Name".to_string()],
            vec![tagged.clone()],
        ),
        (
            "a wildcard over the pool's own range",
            "address",
            vec!["54.*".to_string()],
            vec![plain.clone(), tagged.clone(), held.clone()],
        ),
        (
            "a value no address carries",
            "network-border-group",
            vec![OTHER_REGION.to_string()],
            Vec::new(),
        ),
    ];

    for (row, name, values, mut expected) in rows {
        let mut extra = vec![p("Filter.1.Name", name)];
        for (index, value) in values.iter().enumerate() {
            extra.push(p(&format!("Filter.1.Value.{}", index + 1), value.clone()));
        }
        let body = ok(row, &router, ACTION, &sample_params(&discovery, REGION, extra)).await;
        expected.sort();
        assert_eq!(
            reported_addresses(row, &body),
            expected,
            "case {row:?}: expected the addresses {expected:?} in {body}"
        );
    }

    // Two filters are joined with an AND, so one that holds and one that cannot select
    // nothing together.
    let both = ok(
        "an AND of two filters",
        &router,
        ACTION,
        &sample_params(
            &discovery,
            REGION,
            vec![
                p("Filter.1.Name", "association-status"),
                p("Filter.1.Value.1", DISASSOCIATED),
                p("Filter.2.Name", "address"),
                p("Filter.2.Value.1", held.clone()),
            ],
        ),
    )
    .await;
    assert!(
        reported_addresses("an AND of two filters", &both).is_empty(),
        "case {:?}: the AND of two filters selects nothing, got {both}",
        "an AND of two filters"
    );

    // A filter name the operation does not resolve is refused, whether it is a misspelling or
    // a `tag:` with no key at all.
    for (row, name) in [
        ("a filter name nothing answers", "public-address"),
        ("a tag filter naming no key", "tag:"),
    ] {
        let (status, body) = api(
            &router,
            REGION,
            ACTION,
            &sample_params(
                &discovery,
                REGION,
                vec![p("Filter.1.Name", name), p("Filter.1.Value.1", "anything")],
            ),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {row:?}: expected HTTP 400 for the filter {name:?}, got {status}: {body}"
        );
        assert_eq!(
            ec2_query::error_code(&body),
            INVALID_PARAMETER_VALUE,
            "case {row:?}: expected {INVALID_PARAMETER_VALUE} for the filter {name:?}, \
             got {body}"
        );
        assert!(
            ec2_query::error_message(&body).contains(name),
            "case {row:?}: expected the refusal to name the filter {name:?}, got {}",
            ec2_query::error_message(&body)
        );
    }
}

/// Rule 3, second half: `MaxResults` pages the sample in address order and the `NextToken` of
/// one page resumes the walk, so following the tokens reports every address exactly once.
///
/// The rows differ along what that generalises over: the page size asked for, relative to how
/// many addresses the Region holds (its smallest documented value, one that divides the set,
/// one that does not, and one larger than the whole set).
#[tokio::test]
async fn max_results_pages_the_sample_in_address_order() {
    let router = emulator();
    let case = "twelve addresses";
    let discovery = default_discovery(case, &router, &[]).await;
    let mut all = Vec::new();
    for index in 0..12 {
        let (_, public_ip) = allocate(&format!("address {index}"), &router, REGION, &[]).await;
        all.push(public_ip);
    }
    all.sort();

    // The model's own range for the member is 5..1000
    // (`smithy_query(query="operation",operation="GetIpamDiscoveredPublicAddresses")`), so 5
    // is the smallest page a request may ask for.
    for size in [5_usize, 6, 7, 1000] {
        let row = format!("MaxResults={size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut extra = vec![p("MaxResults", size.to_string())];
            if let Some(next) = token.clone() {
                extra.push(p("NextToken", next));
            }
            let body = ok(&row, &router, ACTION, &sample_params(&discovery, REGION, extra)).await;
            let page: Vec<String> = discovered(&row, &body)
                .into_iter()
                .map(|entry| entry.address)
                .collect();
            assert!(
                page.len() <= size,
                "case {row:?}: expected at most {size} addresses in a page, got {page:?}"
            );
            let mut sorted = page.clone();
            sorted.sort();
            assert_eq!(
                page, sorted,
                "case {row:?}: expected the page in address order, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "case {row:?}: the walk must end, got {pages} pages for {} addresses",
                all.len()
            );
            token = ec2_query::member(&body, "nextToken").filter(|value| !value.is_empty());
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, all,
            "case {row:?}: expected the whole sample once, in address order, got {walked:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// What the operation refuses
// ---------------------------------------------------------------------------

/// Rule 4: a request whose discovery id or `NextToken` the service cannot resolve is refused
/// with EC2's own code, and a `DryRun` of a request that would have succeeded reports that and
/// reads nothing.
///
/// The rows differ along what that generalises over: which member is at fault and in what way
/// — an id of the wrong form, an id of the right form naming nothing, an id of a NEIGHBOURING
/// resource of the same prefix, and a token this service never issued.
#[tokio::test]
async fn an_unresolvable_discovery_id_or_token_is_refused_and_a_dry_run_reports_it() {
    let router = emulator();
    let case = "refusals";
    let discovery = default_discovery(case, &router, &[]).await;
    allocate(case, &router, REGION, &[]).await;

    // (case, discovery id, next token, expected code)
    let rows: [(&str, &str, Option<&str>, &str); 5] = [
        (
            "not an id of any resource",
            "not-an-id",
            None,
            INVALID_IPAM_RESOURCE_DISCOVERY_ID_MALFORMED,
        ),
        (
            "an id of the neighbouring association resource",
            "ipam-res-disco-assoc-0123456789abcdef0",
            None,
            INVALID_IPAM_RESOURCE_DISCOVERY_ID_MALFORMED,
        ),
        (
            "a well-formed long id naming no discovery",
            "ipam-res-disco-0123456789abcdef0",
            None,
            INVALID_IPAM_RESOURCE_DISCOVERY_ID_NOT_FOUND,
        ),
        (
            "a well-formed short id naming no discovery",
            "ipam-res-disco-01234567",
            None,
            INVALID_IPAM_RESOURCE_DISCOVERY_ID_NOT_FOUND,
        ),
        (
            "a token this service did not issue",
            discovery.as_str(),
            Some("not-a-token"),
            INVALID_PAGINATION_TOKEN,
        ),
    ];

    for (row, discovery_id, token, expected) in rows {
        let extra = token.map(|value| vec![p("NextToken", value)]).unwrap_or_default();
        let (status, body) = api(
            &router,
            REGION,
            ACTION,
            &sample_params(discovery_id, REGION, extra),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {row:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            ec2_query::error_code(&body),
            expected,
            "case {row:?}: expected {expected}, got {body}"
        );
    }

    // "If you have the required permissions, the error response is DryRunOperation": the
    // request would have succeeded, so that is what it is told.
    let (status, body) = api(
        &router,
        REGION,
        ACTION,
        &sample_params(&discovery, REGION, vec![p("DryRun", "true")]),
    )
    .await;
    assert_eq!(
        status, 412,
        "case {case:?}: expected HTTP 412 from a dry run, got {status} with body {body}"
    );
    assert_eq!(
        ec2_query::error_code(&body),
        DRY_RUN_OPERATION,
        "case {case:?}: expected {DRY_RUN_OPERATION} from a dry run, got {body}"
    );
}
