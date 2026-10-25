// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EC2 Instance Connect Endpoint slice:
//! `CreateInstanceConnectEndpoint` and `DescribeInstanceConnectEndpoints`, driven over the
//! wire the service actually speaks — `ec2Query`, through the crate's shared
//! [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. `DescribeInstanceConnectEndpoints` REPORTS THE STORE: the set it answers with is the
//!    account's endpoints narrowed by the `InstanceConnectEndpointIds` and `Filters` the
//!    request names and cut into `MaxResults`-sized pages, and a read changes nothing a
//!    later caller sees;
//! 2. `CreateInstanceConnectEndpoint` MINTS AN ENDPOINT FROM THE REQUEST: every member the
//!    request sets round-trips through the describe, and the members the service owns (the
//!    id, the ARN, the account, the DNS names, the create time, and the VPC and zone of the
//!    endpoint's subnet) are derived rather than taken;
//! 3. the endpoint's `IpAddressType` is the one the request names, and when it names none
//!    the one the SUBNET's CIDR families determine
//!    (`CreateInstanceConnectEndpointRequest$IpAddressType`); a type the subnet has no CIDR
//!    block for is refused;
//! 4. an endpoint is `create-in-progress` and the CLOCK, not a caller, moves it to
//!    `create-complete`;
//! 5. one `ClientToken` names one endpoint.
//!
//! Each table row is a distinct input along what the rule generalises over — which address
//! family the subnet has, which address type the request asks for, which filter, how many
//! values, which ids, which page size — so no row is the specification.

use std::collections::BTreeMap;
use std::time::Duration;

use ec2::ops::types::{SubnetCidrBlockState, SubnetIpv6CidrBlockAssociation};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, INSTANCE_CONNECT_ENDPOINT_ID_PREFIX,
    INSTANCE_CONNECT_ENDPOINT_STATE_CREATE_COMPLETE as STATE_CREATE_COMPLETE,
    INSTANCE_CONNECT_ENDPOINT_STATE_CREATE_IN_PROGRESS as STATE_CREATE_IN_PROGRESS,
    IP_ADDRESS_TYPE_DUALSTACK, IP_ADDRESS_TYPE_IPV4, IP_ADDRESS_TYPE_IPV6, SecurityGroupSpec,
    SubnetSpec, TRANSITION_DELAY, availability_zones_in_region,
    instance_connect_endpoint_dns_name, instance_connect_endpoint_fips_dns_name,
    instance_connect_endpoint_id_is_well_formed,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, filters, indexed};
// The XML readers every suite in this crate shares, rather than a fourth copy of them.
use super::wire::{child, child_text, document};

/// The Region every call here is signed for; the zone names and ids the assertions use are
/// derived from it, and it is the Region the endpoints' ARNs and DNS names name.
const REGION: &str = "us-east-1";

/// The IPv6 CIDR block the dual-stack and the IPv6-only seeded subnets carry.
const SEEDED_IPV6_CIDR_BLOCK: &str = "2600:1f14:abc:1200::/64";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One row of a table that expects a refusal: `(case, the parameters, the error code)`.
type RefusalRow = (&'static str, Params, &'static str);

/// One row of the address-type table: `(the subnet's families, the requested
/// `IpAddressType` or `None`, the `PreserveClientIp` the request carries or `None`, the
/// value that must be reported or `None` for a refusal)`.
type PreserveRow = (
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    Option<&'static str>,
);

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "CreateInstanceConnectEndpoint", params).await
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeInstanceConnectEndpoints", params).await
}

/// `TagSpecification.1` form fields tagging the endpoint a create mints.
fn tag_params(tags: &[(&str, &str)]) -> Params {
    let mut params = vec![p(
        "TagSpecification.1.ResourceType",
        "instance-connect-endpoint",
    )];
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

/// The network a suite's endpoints are created into: one VPC with a subnet per address
/// family, plus a second VPC whose security group belongs to another network.
struct Fixture {
    router: MultiServiceRouter,
    engine: std::sync::Arc<engine::InMemoryEngine>,
    vpc_id: String,
    /// The subnet with an IPv4 CIDR block only.
    ipv4_subnet: String,
    /// The subnet with both an IPv4 and an IPv6 CIDR block.
    dual_subnet: String,
    /// The subnet with an IPv6 CIDR block only.
    ipv6_subnet: String,
    /// The default security group of [`Fixture::vpc_id`], which a create that names none
    /// associates with the endpoint.
    default_group: String,
    /// Another security group of that same VPC.
    extra_group: String,
    /// A security group of a DIFFERENT VPC.
    foreign_group: String,
}

/// Seed the network the tables are written against, the way the creating operations would
/// write it: the IPv6 CIDR blocks are attached to the stored subnets directly, because
/// `CreateSubnet` answers IPv6 with a 501 in this emulator (it holds no IPv6 subnet of its
/// own), and this suite is about what an endpoint does with the subnet it finds.
fn fixture() -> Fixture {
    let (router, engine) = super::ec2_query::emulator();
    let zones = availability_zones_in_region(REGION);
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();

    let vpc_id = guard.insert_vpc(
        "10.0.0.0/16",
        false,
        "default",
        "available",
        REGION,
        Vec::new(),
    );
    let default_group = guard.insert_default_security_group(&vpc_id, REGION);
    let extra_group = guard.insert_security_group(SecurityGroupSpec {
        vpc_id: vpc_id.clone(),
        group_name: "endpoint-access".to_string(),
        description: "the group an endpoint row names explicitly".to_string(),
        region: REGION.to_string(),
        tags: Vec::new(),
        ip_permissions: Vec::new(),
        ip_permissions_egress: Vec::new(),
    });
    let foreign_vpc = guard.insert_vpc(
        "10.1.0.0/16",
        false,
        "default",
        "available",
        REGION,
        Vec::new(),
    );
    let foreign_group = guard.insert_default_security_group(&foreign_vpc, REGION);

    let mut subnet_in = |index: usize, cidr_block: &str| {
        guard.insert_subnet(SubnetSpec {
            vpc_id: vpc_id.clone(),
            cidr_block: cidr_block.to_string(),
            availability_zone: zones[index].zone_name.clone(),
            availability_zone_id: zones[index].zone_id.clone(),
            region: REGION.to_string(),
            state: "available".to_string(),
            default_for_az: false,
            map_public_ip_on_launch: false,
            tags: Vec::new(),
        })
    };
    let ipv4_subnet = subnet_in(0, "10.0.0.0/20");
    let dual_subnet = subnet_in(1, "10.0.16.0/20");
    let ipv6_subnet = subnet_in(2, "10.0.32.0/20");

    let association = SubnetIpv6CidrBlockAssociation {
        association_id: Some(guard.next_id("subnet-cidr-assoc")),
        ipv6_cidr_block: Some(SEEDED_IPV6_CIDR_BLOCK.to_string()),
        ipv6_cidr_block_state: Some(SubnetCidrBlockState {
            state: Some("associated".to_string()),
            status_message: None,
        }),
        ipv6_address_attribute: None,
        ip_source: None,
    };
    if let Some(subnet) = guard.subnets.get_mut(&dual_subnet) {
        subnet.ipv6_cidr_block_association_set = Some(vec![association.clone()]);
    }
    if let Some(subnet) = guard.subnets.get_mut(&ipv6_subnet) {
        // "Indicates whether this is an IPv6 only subnet" (`Subnet$Ipv6Native`): no IPv4
        // CIDR block at all.
        subnet.cidr_block = None;
        subnet.ipv6_native = Some(true);
        subnet.ipv6_cidr_block_association_set = Some(vec![association]);
    }
    drop(guard);

    Fixture {
        router,
        engine,
        vpc_id,
        ipv4_subnet,
        dual_subnet,
        ipv6_subnet,
        default_group,
        extra_group,
        foreign_group,
    }
}

impl Fixture {
    /// How many endpoints the store holds, read straight off the state the handlers share.
    fn stored_endpoints(&self) -> usize {
        self.engine
            .state::<Ec2State>()
            .lock()
            .instance_connect_endpoints
            .len()
    }
}

/// One endpoint of a response body, as the ec2Query XML reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct EndpointRow {
    id: String,
    arn: String,
    owner_id: String,
    state: String,
    dns_name: String,
    fips_dns_name: String,
    vpc_id: String,
    subnet_id: String,
    availability_zone: String,
    availability_zone_id: String,
    ip_address_type: String,
    preserve_client_ip: String,
    created_at: String,
    security_group_ids: Vec<String>,
    tags: BTreeMap<String, String>,
}

/// The text of `node`'s `name` child, or the empty string when it has none — the shared
/// [`child_text`] with the absent case flattened, so a row compares as plain text.
fn text_of(node: roxmltree::Node<'_, '_>, name: &str) -> String {
    child_text(node, name).unwrap_or_default()
}

/// The texts of the items of `node`'s `set` child — an ec2Query list is a `<xSet>` of
/// `<item>`s.
fn set_items(node: roxmltree::Node<'_, '_>, set: &str) -> Vec<String> {
    child(node, set)
        .map(|list| {
            list.children()
                .filter(roxmltree::Node::is_element)
                .map(|item| item.text().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The `tagSet` of `node`, as a key -> value map.
fn tag_map(node: roxmltree::Node<'_, '_>) -> BTreeMap<String, String> {
    child(node, "tagSet")
        .map(|list| {
            list.children()
                .filter(roxmltree::Node::is_element)
                .map(|item| (text_of(item, "key"), text_of(item, "value")))
                .collect()
        })
        .unwrap_or_default()
}

/// Every endpoint a response body reports, in document order. An endpoint is any element
/// carrying an `instanceConnectEndpointId` child, which is both the create response's one
/// endpoint and each item of a describe's set.
fn endpoint_rows(case: &str, body: &str) -> Vec<EndpointRow> {
    let document = document(case, body);
    document
        .descendants()
        .filter(|node| {
            node.is_element() && child(*node, "instanceConnectEndpointId").is_some()
        })
        .map(|node| EndpointRow {
            id: text_of(node, "instanceConnectEndpointId"),
            arn: text_of(node, "instanceConnectEndpointArn"),
            owner_id: text_of(node, "ownerId"),
            state: text_of(node, "state"),
            dns_name: text_of(node, "dnsName"),
            fips_dns_name: text_of(node, "fipsDnsName"),
            vpc_id: text_of(node, "vpcId"),
            subnet_id: text_of(node, "subnetId"),
            availability_zone: text_of(node, "availabilityZone"),
            availability_zone_id: text_of(node, "availabilityZoneId"),
            ip_address_type: text_of(node, "ipAddressType"),
            preserve_client_ip: text_of(node, "preserveClientIp"),
            created_at: text_of(node, "createdAt"),
            security_group_ids: set_items(node, "securityGroupIdSet"),
            tags: tag_map(node),
        })
        .collect()
}

/// The one endpoint a successful create reports.
async fn create_endpoint(router: &MultiServiceRouter, case: &str, params: &[(String, String)]) -> EndpointRow {
    let (status, body) = create(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateInstanceConnectEndpoint, \
         got {status} with body {body}"
    );
    let rows = endpoint_rows(case, &body);
    assert_eq!(
        rows.len(),
        1,
        "case {case:?}: expected exactly one endpoint in the create response, got {rows:?}"
    );
    rows.into_iter().next().unwrap_or_else(|| unreachable!())
}

/// The endpoint ids a successful describe reports, in the order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeInstanceConnectEndpoints, \
         got {status} with body {body}"
    );
    endpoint_rows(case, &body)
        .into_iter()
        .map(|row| row.id)
        .collect()
}

/// An `InstanceConnectEndpointId` is `eice-` followed by 8 or 17 lowercase hex digits — the
/// service's own id rule, imported rather than re-derived so the two cannot drift.
fn is_endpoint_id(candidate: &str) -> bool {
    instance_connect_endpoint_id_is_well_formed(candidate)
}

// ---------------------------------------------------------------------------
// RULE 2: a create mints the endpoint from the request, a describe reports the store
// ---------------------------------------------------------------------------

/// RULE 2 and RULE 1 together: every member the request sets round-trips through
/// `DescribeInstanceConnectEndpoints`, the service-owned members are derived, and the
/// describe reports exactly what the create answered.
#[tokio::test]
async fn a_created_endpoint_round_trips_through_the_describe() {
    let fixture = fixture();
    let case = "an endpoint with two tags and two named security groups";
    let mut params = vec![
        p("SubnetId", fixture.ipv4_subnet.clone()),
        p("PreserveClientIp", "true"),
    ];
    params.extend(indexed(
        "SecurityGroupId",
        &[&fixture.extra_group, &fixture.default_group],
    ));
    params.extend(tag_params(&[("Name", "bastion"), ("Owner", "TeamA")]));
    let created = create_endpoint(&fixture.router, case, &params).await;

    assert!(
        is_endpoint_id(&created.id),
        "case {case:?}: expected a minted endpoint id, got {:?}",
        created.id
    );
    let zones = availability_zones_in_region(REGION);
    let expected = EndpointRow {
        id: created.id.clone(),
        arn: format!(
            "arn:aws:ec2:{REGION}:{account}:instance-connect-endpoint/{id}",
            account = engine::EMULATOR_ACCOUNT_ID,
            id = created.id,
        ),
        owner_id: engine::EMULATOR_ACCOUNT_ID.to_string(),
        state: STATE_CREATE_IN_PROGRESS.to_string(),
        dns_name: created.dns_name.clone(),
        fips_dns_name: created.fips_dns_name.clone(),
        vpc_id: fixture.vpc_id.clone(),
        subnet_id: fixture.ipv4_subnet.clone(),
        availability_zone: zones[0].zone_name.clone(),
        availability_zone_id: zones[0].zone_id.clone(),
        ip_address_type: IP_ADDRESS_TYPE_IPV4.to_string(),
        preserve_client_ip: "true".to_string(),
        created_at: created.created_at.clone(),
        // The ids a request names are reported in the order the service resolved them,
        // which is the request's own order.
        security_group_ids: vec![fixture.extra_group.clone(), fixture.default_group.clone()],
        tags: [
            ("Name".to_string(), "bastion".to_string()),
            ("Owner".to_string(), "TeamA".to_string()),
        ]
        .into_iter()
        .collect(),
    };
    assert_eq!(
        created, expected,
        "case {case:?}: expected the create to answer {expected:?}, got {created:?}"
    );
    assert!(
        !created.created_at.is_empty(),
        "case {case:?}: a created endpoint carries a create time, got {created:?}"
    );

    // The DNS names are DERIVED from the endpoint's id and the Region it was created in,
    // which is what makes them stable across reads and unique per endpoint.
    for (member, value) in [
        ("dnsName", &created.dns_name),
        ("fipsDnsName", &created.fips_dns_name),
    ] {
        assert!(
            value.starts_with(&format!("{}.", created.id)) && value.contains(REGION),
            "case {case:?}: expected {member} to name the endpoint and the Region, got {value:?}"
        );
    }
    assert_ne!(
        created.dns_name, created.fips_dns_name,
        "case {case:?}: the FIPS name is a different name, got {created:?}"
    );

    // RULE 1: the describe reports the store, so it answers exactly what was stored — and
    // reading twice does not change it.
    for read in 1..=2 {
        let (status, body) = describe(
            &fixture.router,
            &indexed("InstanceConnectEndpointId", &[&created.id]),
        )
        .await;
        assert_eq!(
            status, 200,
            "read {read}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            endpoint_rows(case, &body),
            vec![created.clone()],
            "read {read}: the describe must report the endpoint the create stored"
        );
    }

    // A second endpoint gets its own id, ARN and DNS names.
    let other = create_endpoint(
        &fixture.router,
        "a second endpoint in the dual-stack subnet",
        &[p("SubnetId", fixture.dual_subnet.clone())],
    )
    .await;
    assert_ne!(
        other.id, created.id,
        "two endpoints must not share an id, got {other:?} and {created:?}"
    );
    assert_ne!(
        other.dns_name, created.dns_name,
        "two endpoints must not share a DNS name, got {other:?} and {created:?}"
    );
    // The endpoint's network facts come from its SUBNET, not from the request.
    assert_eq!(
        (
            other.vpc_id.as_str(),
            other.availability_zone.as_str(),
            other.availability_zone_id.as_str()
        ),
        (
            fixture.vpc_id.as_str(),
            zones[1].zone_name.as_str(),
            zones[1].zone_id.as_str()
        ),
        "an endpoint reports the VPC and zone of its subnet, got {other:?}"
    );
    // "If you don't specify a security group, the default security group for your VPC will
    // be associated with the endpoint."
    assert_eq!(
        other.security_group_ids,
        vec![fixture.default_group.clone()],
        "an endpoint created without a security group takes its VPC's default, got {other:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 3: the address type the request names, else the subnet's own families
// ---------------------------------------------------------------------------

/// RULE 3 over the subnet's address families crossed with every value of the modelled
/// `IpAddressType` enum and with naming none: the type is the request's when the subnet has
/// a CIDR block for it, the subnet's own when the request names none, and a refusal when
/// the subnet has no CIDR block for the type asked for.
///
/// The catalog of types is the modelled enum
/// (`smithy_query(query="shape",shape="IpAddressType")`), so no value is left untested.
#[tokio::test]
async fn the_address_type_follows_the_request_then_the_subnets_cidr_families() {
    // (the subnet's families, the requested type or None, the type that must be reported)
    let rows: Vec<(&str, Option<&str>, Option<&str>)> = vec![
        ("ipv4", None, Some(IP_ADDRESS_TYPE_IPV4)),
        ("ipv4", Some(IP_ADDRESS_TYPE_IPV4), Some(IP_ADDRESS_TYPE_IPV4)),
        ("ipv4", Some(IP_ADDRESS_TYPE_DUALSTACK), None),
        ("ipv4", Some(IP_ADDRESS_TYPE_IPV6), None),
        ("dual", None, Some(IP_ADDRESS_TYPE_DUALSTACK)),
        ("dual", Some(IP_ADDRESS_TYPE_IPV4), Some(IP_ADDRESS_TYPE_IPV4)),
        (
            "dual",
            Some(IP_ADDRESS_TYPE_DUALSTACK),
            Some(IP_ADDRESS_TYPE_DUALSTACK),
        ),
        ("dual", Some(IP_ADDRESS_TYPE_IPV6), Some(IP_ADDRESS_TYPE_IPV6)),
        ("ipv6", None, Some(IP_ADDRESS_TYPE_IPV6)),
        ("ipv6", Some(IP_ADDRESS_TYPE_IPV6), Some(IP_ADDRESS_TYPE_IPV6)),
        ("ipv6", Some(IP_ADDRESS_TYPE_IPV4), None),
        ("ipv6", Some(IP_ADDRESS_TYPE_DUALSTACK), None),
    ];
    for (families, requested, expected) in rows {
        let fixture = fixture();
        let subnet_id = match families {
            "ipv4" => fixture.ipv4_subnet.clone(),
            "dual" => fixture.dual_subnet.clone(),
            _ => fixture.ipv6_subnet.clone(),
        };
        let case = format!("a {families} subnet asked for {requested:?}");
        let mut params = vec![p("SubnetId", subnet_id)];
        if let Some(requested) = requested {
            params.push(p("IpAddressType", requested));
        }
        match expected {
            Some(address_type) => {
                let created = create_endpoint(&fixture.router, &case, &params).await;
                assert_eq!(
                    created.ip_address_type, address_type,
                    "case {case:?}: expected ipAddressType={address_type:?}, got {created:?}"
                );
                // The stored endpoint reports the same type a describe later reads.
                let ids = described_ids(&fixture.router, &case, &[]).await;
                assert_eq!(
                    ids,
                    vec![created.id.clone()],
                    "case {case:?}: expected the describe to report the one endpoint"
                );
            }
            None => {
                let (status, body) = create(&fixture.router, &params).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with body {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "InvalidParameterCombination",
                    "case {case:?}: expected InvalidParameterCombination, got {body}"
                );
                assert_eq!(
                    fixture.stored_endpoints(),
                    0,
                    "case {case:?}: a refused create must store nothing"
                );
            }
        }
    }
}

/// RULE 3's note: "`PreserveClientIp` is only supported on IPv4 EC2 Instance Connect
/// Endpoints. To use `PreserveClientIp`, the value for `IpAddressType` must be `ipv4`"
/// (`CreateInstanceConnectEndpointRequest$PreserveClientIp`), whose "Default: false" is not
/// using it.
#[tokio::test]
async fn preserve_client_ip_is_refused_outside_an_ipv4_endpoint() {
    // (the subnet's families, the requested type or None, PreserveClientIp or None,
    //  the value that must be reported, or None for a refusal)
    let rows: Vec<PreserveRow> = vec![
        ("ipv4", None, None, Some("false")),
        ("ipv4", None, Some("false"), Some("false")),
        ("ipv4", None, Some("true"), Some("true")),
        ("dual", Some(IP_ADDRESS_TYPE_IPV4), Some("true"), Some("true")),
        ("dual", None, Some("true"), None),
        ("dual", Some(IP_ADDRESS_TYPE_IPV6), Some("true"), None),
        ("ipv6", None, Some("true"), None),
        ("ipv6", None, Some("false"), Some("false")),
        ("ipv6", None, None, Some("false")),
    ];
    for (families, address_type, preserve, expected) in rows {
        let fixture = fixture();
        let subnet_id = match families {
            "ipv4" => fixture.ipv4_subnet.clone(),
            "dual" => fixture.dual_subnet.clone(),
            _ => fixture.ipv6_subnet.clone(),
        };
        let case =
            format!("a {families} subnet, ipAddressType={address_type:?}, preserve={preserve:?}");
        let mut params = vec![p("SubnetId", subnet_id)];
        if let Some(address_type) = address_type {
            params.push(p("IpAddressType", address_type));
        }
        if let Some(preserve) = preserve {
            params.push(p("PreserveClientIp", preserve));
        }
        match expected {
            Some(value) => {
                let created = create_endpoint(&fixture.router, &case, &params).await;
                assert_eq!(
                    created.preserve_client_ip, value,
                    "case {case:?}: expected preserveClientIp={value:?}, got {created:?}"
                );
            }
            None => {
                let (status, body) = create(&fixture.router, &params).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with body {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "InvalidParameterCombination",
                    "case {case:?}: expected InvalidParameterCombination, got {body}"
                );
                assert_eq!(
                    fixture.stored_endpoints(),
                    0,
                    "case {case:?}: a refused create must store nothing"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 4: the clock, not a caller, completes the creation
// ---------------------------------------------------------------------------

/// RULE 4: a created endpoint is `create-in-progress`, every read inside the span still
/// says so, and the clock — nothing the caller does — moves it to `create-complete`. The
/// `state` filter agrees with what each endpoint reports at the same instant.
#[tokio::test]
async fn a_created_endpoint_is_create_in_progress_until_the_clock_completes_it() {
    let fixture = fixture();
    let mut ids = Vec::new();
    // Three endpoints in three different subnets, so the transition is not a property of
    // one row. Each endpoint's own span is read right after its own create — never after
    // the whole loop — so what these assertions observe is the span the service keeps and
    // not how long the rest of the suite took on the machine running it.
    for subnet_id in [
        &fixture.ipv4_subnet,
        &fixture.dual_subnet,
        &fixture.ipv6_subnet,
    ] {
        let case = format!("an endpoint in {subnet_id}");
        let created = create_endpoint(&fixture.router, &case, &[p("SubnetId", subnet_id.clone())]).await;
        assert_eq!(
            created.state, STATE_CREATE_IN_PROGRESS,
            "case {case:?}: expected a fresh endpoint to be {STATE_CREATE_IN_PROGRESS}, \
             got {created:?}"
        );

        // The span is never zero: a caller reading straight after its own create still
        // sees the transient state, and the `state` filter agrees with what the endpoint
        // reports at that instant.
        let in_progress = described_ids(
            &fixture.router,
            &format!("{case}, state=create-in-progress inside its span"),
            &filters(&[("state", &[STATE_CREATE_IN_PROGRESS])]),
        )
        .await;
        assert!(
            in_progress.contains(&created.id),
            "case {case:?}: expected {} among the endpoints the \
             state={STATE_CREATE_IN_PROGRESS} filter reports inside its span, got \
             {in_progress:?}",
            created.id
        );
        let complete = described_ids(
            &fixture.router,
            &format!("{case}, state=create-complete inside its span"),
            &filters(&[("state", &[STATE_CREATE_COMPLETE])]),
        )
        .await;
        assert!(
            !complete.contains(&created.id),
            "case {case:?}: expected {} not to be {STATE_CREATE_COMPLETE} inside its \
             span, got {complete:?}",
            created.id
        );

        ids.push(created.id);
    }
    ids.sort();

    past_the_transition().await;

    for id in &ids {
        // Twice: the materialised state is stable, not consumed by the read.
        for read in 1..=2 {
            let (status, body) =
                describe(&fixture.router, &indexed("InstanceConnectEndpointId", &[id])).await;
            assert_eq!(
                status, 200,
                "read {read}: expected HTTP 200, got {status} with body {body}"
            );
            let rows = endpoint_rows("one endpoint after the span", &body);
            assert_eq!(
                rows.first().map(|row| row.state.as_str()),
                Some(STATE_CREATE_COMPLETE),
                "read {read}: expected {id} to be {STATE_CREATE_COMPLETE} once the \
                 transition is due, got {body}"
            );
        }
    }
    assert_eq!(
        described_ids(
            &fixture.router,
            "state=create-complete after the span",
            &filters(&[("state", &[STATE_CREATE_COMPLETE])])
        )
        .await,
        ids,
        "case \"state=create-complete after the span\": expected every endpoint"
    );
    assert!(
        described_ids(
            &fixture.router,
            "state=create-in-progress after the span",
            &filters(&[("state", &[STATE_CREATE_IN_PROGRESS])])
        )
        .await
        .is_empty(),
        "case \"state=create-in-progress after the span\": no endpoint is in progress any more"
    );
}

// ---------------------------------------------------------------------------
// RULE 1: narrowing by ids and by every documented filter
// ---------------------------------------------------------------------------

/// Three endpoints spread over the fixture's subnets and tags, for the filter and paging
/// tables to name rows by: the ids in the order the describe reports them.
async fn seeded_endpoints(fixture: &Fixture) -> Vec<String> {
    let mut ids = Vec::new();
    for (subnet_id, tags) in [
        (&fixture.ipv4_subnet, vec![("Name", "web"), ("Owner", "TeamA")]),
        (&fixture.dual_subnet, vec![("Name", "batch"), ("Owner", "TeamB")]),
        (&fixture.ipv6_subnet, vec![("Project", "atlas")]),
    ] {
        let case = format!("a seeded endpoint in {subnet_id}");
        let mut params = vec![p("SubnetId", subnet_id.clone())];
        params.extend(tag_params(&tags));
        ids.push(create_endpoint(&fixture.router, &case, &params).await.id);
    }
    ids.sort();
    ids
}

/// RULE 1 over every filter name `DescribeInstanceConnectEndpointsRequest$Filters`
/// documents, plus the AND of two filters, the OR of two values and EC2's `*` wildcard.
#[tokio::test]
async fn every_documented_filter_selects_the_endpoints_it_names() {
    let fixture = fixture();
    let ids = seeded_endpoints(&fixture).await;
    let all = ids.clone();

    let rows: Vec<(&str, Params, Vec<String>)> = vec![
        ("no filter at all", Vec::new(), all.clone()),
        (
            "instance-connect-endpoint-id, one value",
            filters(&[("instance-connect-endpoint-id", &[ids[1].as_str()])]),
            vec![ids[1].clone()],
        ),
        (
            "instance-connect-endpoint-id, two values joined with OR",
            filters(&[(
                "instance-connect-endpoint-id",
                &[ids[0].as_str(), ids[2].as_str()],
            )]),
            vec![ids[0].clone(), ids[2].clone()],
        ),
        (
            "instance-connect-endpoint-id with EC2's wildcard",
            filters(&[(
                "instance-connect-endpoint-id",
                &[&format!("{INSTANCE_CONNECT_ENDPOINT_ID_PREFIX}*")],
            )]),
            all.clone(),
        ),
        (
            "subnet-id",
            filters(&[("subnet-id", &[fixture.dual_subnet.as_str()])]),
            vec![ids[1].clone()],
        ),
        (
            "vpc-id, the VPC every endpoint is in",
            filters(&[("vpc-id", &[fixture.vpc_id.as_str()])]),
            all.clone(),
        ),
        (
            "vpc-id of another VPC",
            filters(&[("vpc-id", &["vpc-00000000000000000"])]),
            Vec::new(),
        ),
        (
            "tag:Owner",
            filters(&[("tag:Owner", &["TeamB"])]),
            vec![ids[1].clone()],
        ),
        (
            "tag-key",
            filters(&[("tag-key", &["Owner"])]),
            vec![ids[0].clone(), ids[1].clone()],
        ),
        (
            "tag-value",
            filters(&[("tag-value", &["atlas"])]),
            vec![ids[2].clone()],
        ),
        (
            "two filters joined with AND",
            filters(&[
                ("tag-key", &["Owner"]),
                ("subnet-id", &[fixture.ipv4_subnet.as_str()]),
            ]),
            vec![ids[0].clone()],
        ),
        (
            "two filters joined with AND that no endpoint satisfies",
            filters(&[
                ("tag-value", &["atlas"]),
                ("subnet-id", &[fixture.ipv4_subnet.as_str()]),
            ]),
            Vec::new(),
        ),
    ];
    for (case, params, expected) in rows {
        let mut selected = described_ids(&fixture.router, case, &params).await;
        selected.sort();
        let mut expected = expected;
        expected.sort();
        assert_eq!(
            selected, expected,
            "case {case:?}: expected {expected:?}, got {selected:?}"
        );
    }

    // The `state` filter selects on a value the CLOCK owns, so it is read against an
    // endpoint created immediately before the read rather than as a row of the table
    // above: what it must show is that the filter selects on the state the endpoint
    // reports, not how long the rows before it took.
    let case = "state, the one a freshly created endpoint is in";
    let fresh = create_endpoint(
        &fixture.router,
        case,
        &[p("SubnetId", fixture.ipv4_subnet.clone())],
    )
    .await;
    let selected = described_ids(
        &fixture.router,
        case,
        &filters(&[("state", &[STATE_CREATE_IN_PROGRESS])]),
    )
    .await;
    assert!(
        selected.contains(&fresh.id),
        "case {case:?}: expected {} among the endpoints the \
         state={STATE_CREATE_IN_PROGRESS} filter reports, got {selected:?}",
        fresh.id
    );
}

/// RULE 1: a filter name the operation does not document is refused rather than read as "no
/// results", and the ids it reports are narrowed only by ids it can resolve.
#[tokio::test]
async fn an_unresolvable_filter_or_id_is_refused() {
    let fixture = fixture();
    let ids = seeded_endpoints(&fixture).await;

    let rows: Vec<RefusalRow> = vec![
        (
            "a filter name the operation does not document",
            filters(&[("endpoint-id", &[ids[0].as_str()])]),
            "InvalidParameterValue",
        ),
        (
            "a tag filter with no key",
            filters(&[("tag:", &["TeamA"])]),
            "InvalidParameterValue",
        ),
        (
            "a filter name that differs only in case",
            filters(&[("Subnet-Id", &[fixture.ipv4_subnet.as_str()])]),
            "InvalidParameterValue",
        ),
        (
            "an id that is not shaped like an endpoint id",
            indexed("InstanceConnectEndpointId", &["not-an-endpoint"]),
            "InvalidInstanceConnectEndpointId.Malformed",
        ),
        (
            "an id with a non-hex suffix",
            indexed("InstanceConnectEndpointId", &["eice-zzzzzzzz"]),
            "InvalidInstanceConnectEndpointId.Malformed",
        ),
        (
            "a well-formed id no endpoint of the account has",
            indexed("InstanceConnectEndpointId", &["eice-00000000000000099"]),
            "InvalidInstanceConnectEndpointId.NotFound",
        ),
        (
            "a known id beside an unknown one",
            indexed(
                "InstanceConnectEndpointId",
                &[ids[0].as_str(), "eice-00000000000000099"],
            ),
            "InvalidInstanceConnectEndpointId.NotFound",
        ),
        (
            "a page token this service did not mint",
            vec![p("NextToken", "not-a-token")],
            "InvalidPaginationToken",
        ),
    ];
    for (case, params, code) in rows {
        let (status, body) = describe(&fixture.router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got {body}"
        );
    }

    // A create cannot name a subnet or a security group the account does not have either.
    let create_rows: Vec<RefusalRow> = vec![
        (
            "a subnet no subnet of the account has",
            vec![p("SubnetId", "subnet-00000000000000099")],
            "InvalidSubnetID.NotFound",
        ),
        (
            "a security group no group of the account has",
            {
                let mut params = vec![p("SubnetId", fixture.ipv4_subnet.clone())];
                params.extend(indexed("SecurityGroupId", &["sg-00000000000000099"]));
                params
            },
            "InvalidGroup.NotFound",
        ),
        (
            "a security group of another VPC",
            {
                let mut params = vec![p("SubnetId", fixture.ipv4_subnet.clone())];
                params.extend(indexed("SecurityGroupId", &[&fixture.foreign_group]));
                params
            },
            "InvalidParameterValue",
        ),
        (
            "a tag specification naming another resource type",
            vec![
                p("SubnetId", fixture.ipv4_subnet.clone()),
                p("TagSpecification.1.ResourceType", "subnet"),
                p("TagSpecification.1.Tag.1.Key", "Name"),
                p("TagSpecification.1.Tag.1.Value", "bastion"),
            ],
            "InvalidParameterValue",
        ),
    ];
    for (case, params, code) in create_rows {
        let (status, body) = create(&fixture.router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got {body}"
        );
        assert_eq!(
            fixture.stored_endpoints(),
            ids.len(),
            "case {case:?}: a refused create must store nothing"
        );
    }
}

/// RULE 1: the answer is cut into `MaxResults`-sized pages, and walking the `NextToken`s
/// visits every endpoint exactly once, in id order, whatever the page size.
#[tokio::test]
async fn paging_walks_every_endpoint_once_at_each_page_size() {
    let fixture = fixture();
    let ids = seeded_endpoints(&fixture).await;

    // Each page size from one up to one past the number of endpoints, plus the published
    // bounds of the modelled `MaxResults` range (1..50).
    for page_size in [1_usize, 2, 3, 4, 50] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params = vec![p("MaxResults", page_size.to_string())];
            if let Some(token) = token.as_deref() {
                params.push(p("NextToken", token));
            }
            let (status, body) = describe(&fixture.router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page: Vec<String> = endpoint_rows(&case, &body)
                .into_iter()
                .map(|row| row.id)
                .collect();
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} endpoints, got {page:?}"
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= ids.len() + 1,
                "case {case:?}: paging must terminate, got {seen:?}"
            );
            match element(&body, "nextToken") {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            seen, ids,
            "case {case:?}: paging must visit every endpoint once, in id order"
        );
    }

    // One past each published bound of the modelled `@range`: refused at the boundary,
    // before the handler runs.
    for max_results in ["0", "51"] {
        let case = format!("MaxResults={max_results}");
        let (status, body) = describe(&fixture.router, &[p("MaxResults", max_results)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a MaxResults outside the modelled range, \
             got {status} with body {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 5: one ClientToken names one endpoint
// ---------------------------------------------------------------------------

/// RULE 5: "Unique, case-sensitive identifier that you provide to ensure the idempotency of
/// the request" — a replay of the same token answers the endpoint that token minted and
/// creates nothing; another token mints another endpoint.
#[tokio::test]
async fn one_client_token_names_one_endpoint() {
    let fixture = fixture();
    let rows = [
        ("a lowercase token", "idem-a"),
        ("the same text in another case", "IDEM-A"),
        ("a token of a different shape", "7f3c-0001"),
    ];
    let mut minted: Vec<String> = Vec::new();
    for (case, token) in rows {
        let params = vec![
            p("SubnetId", fixture.ipv4_subnet.clone()),
            p("ClientToken", token),
        ];
        let first = create_endpoint(&fixture.router, case, &params).await;
        assert_eq!(
            element(
                &create(&fixture.router, &params).await.1,
                "clientToken"
            )
            .as_deref(),
            Some(token),
            "case {case:?}: the response echoes the token the request provided"
        );
        let replayed = create_endpoint(&fixture.router, case, &params).await;
        assert_eq!(
            replayed.id, first.id,
            "case {case:?}: a replay of one token answers the endpoint it minted"
        );
        assert!(
            !minted.contains(&first.id),
            "case {case:?}: a different token must mint a different endpoint, got {:?}",
            first.id
        );
        minted.push(first.id);
    }
    assert_eq!(
        fixture.stored_endpoints(),
        rows.len(),
        "one endpoint per distinct token, not one per request"
    );
    let described = described_ids(&fixture.router, "every endpoint", &[]).await;
    let mut expected = minted;
    expected.sort();
    assert_eq!(
        described, expected,
        "the describe reports exactly the endpoints the tokens minted"
    );
}

/// RULE 5's other half: a token names ONE request, so reusing it with any different
/// idempotency-relevant parameter is refused `IdempotentParameterMismatch` rather than
/// answered with the endpoint the first request minted — and nothing is stored for it.
///
/// The rows differ along each member a create resolves: the subnet, the address type, the
/// client-IP flag, the security groups and the tags.
#[tokio::test]
async fn reusing_a_client_token_with_different_parameters_is_refused() {
    let fixture = fixture();
    let token = "idem-mismatch";
    /// The request the token names, plus whatever a row changes about it.
    fn request(fixture: &Fixture, token: &str, extra: Params) -> Params {
        let mut params = vec![
            p("SubnetId", fixture.dual_subnet.clone()),
            p("ClientToken", token),
            p("IpAddressType", IP_ADDRESS_TYPE_IPV4),
            p("PreserveClientIp", "true"),
        ];
        params.extend(tag_params(&[("Name", "bastion")]));
        params.extend(extra);
        params
    }
    let original = request(&fixture, token, Vec::new());
    let first = create_endpoint(&fixture.router, "the request the token names", &original).await;

    // Each row is the same request with ONE member changed — except the address-type row,
    // where the type the request asks for is also what forbids `PreserveClientIp`.
    let rows: Vec<(&str, Params)> = vec![
        (
            "another subnet",
            {
                let mut params = request(&fixture, token, Vec::new());
                params[0] = p("SubnetId", fixture.ipv4_subnet.clone());
                params
            },
        ),
        (
            "another address type, and so no preserved client IP",
            {
                let mut params = vec![
                    p("SubnetId", fixture.dual_subnet.clone()),
                    p("ClientToken", token),
                    p("IpAddressType", IP_ADDRESS_TYPE_DUALSTACK),
                ];
                params.extend(tag_params(&[("Name", "bastion")]));
                params
            },
        ),
        (
            "another client-IP flag",
            {
                let mut params = request(&fixture, token, Vec::new());
                params[3] = p("PreserveClientIp", "false");
                params
            },
        ),
        (
            "another security group",
            request(
                &fixture,
                token,
                indexed("SecurityGroupId", &[&fixture.extra_group]),
            ),
        ),
        (
            "another tag value",
            {
                let mut params = vec![
                    p("SubnetId", fixture.dual_subnet.clone()),
                    p("ClientToken", token),
                    p("IpAddressType", IP_ADDRESS_TYPE_IPV4),
                    p("PreserveClientIp", "true"),
                ];
                params.extend(tag_params(&[("Name", "jump")]));
                params
            },
        ),
        (
            "no tags at all",
            vec![
                p("SubnetId", fixture.dual_subnet.clone()),
                p("ClientToken", token),
                p("IpAddressType", IP_ADDRESS_TYPE_IPV4),
                p("PreserveClientIp", "true"),
            ],
        ),
    ];
    for (case, params) in rows {
        let (status, body) = create(&fixture.router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "IdempotentParameterMismatch",
            "case {case:?}: expected IdempotentParameterMismatch, got {body}"
        );
        assert_eq!(
            fixture.stored_endpoints(),
            1,
            "case {case:?}: a refused replay must store nothing"
        );
    }

    // The request the token DOES name still replays, and answers the endpoint it minted.
    let replayed = create_endpoint(&fixture.router, "the same request again", &original).await;
    assert_eq!(
        replayed.id, first.id,
        "the request a token names still answers the endpoint it minted"
    );

    // A DryRun of a MISMATCHED replay is refused with the reason it would really fail on,
    // not with DryRunOperation; a dry run of the request the token names reports that it
    // would have succeeded.
    let mut mismatched = request(&fixture, token, vec![p("DryRun", "true")]);
    mismatched[0] = p("SubnetId", fixture.ipv4_subnet.clone());
    for (case, params, status, code) in [
        (
            "a dry run that reuses the token with another subnet",
            mismatched,
            400,
            "IdempotentParameterMismatch",
        ),
        (
            "a dry run of the request the token names",
            request(&fixture, token, vec![p("DryRun", "true")]),
            412,
            "DryRunOperation",
        ),
    ] {
        let (actual, body) = create(&fixture.router, &params).await;
        assert_eq!(
            actual, status,
            "case {case:?}: expected HTTP {status}, got {actual} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got {body}"
        );
    }
    assert_eq!(
        fixture.stored_endpoints(),
        1,
        "no dry run stores an endpoint"
    );
}

/// `DryRun` asks whether the request WOULD have succeeded: HTTP 412 `DryRunOperation`, and
/// on the create nothing is stored.
#[tokio::test]
async fn dry_run_answers_without_touching_the_store() {
    let fixture = fixture();
    let ids = seeded_endpoints(&fixture).await;

    for (case, action, params) in [
        (
            "a create that would have succeeded",
            "CreateInstanceConnectEndpoint",
            vec![
                p("SubnetId", fixture.ipv4_subnet.clone()),
                p("DryRun", "true"),
            ],
        ),
        (
            "a describe that would have succeeded",
            "DescribeInstanceConnectEndpoints",
            vec![p("DryRun", "true")],
        ),
    ] {
        let (status, body) = call(&fixture.router, REGION, action, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );
    }
    assert_eq!(
        fixture.stored_endpoints(),
        ids.len(),
        "a dry run stores nothing"
    );
    assert_eq!(
        described_ids(&fixture.router, "every endpoint after the dry runs", &[]).await,
        ids,
        "a dry run leaves the store exactly as it was"
    );
}

/// `SecurityGroupIds` names a SET of associations, so a replay of one token that names the
/// same groups in another order is the SAME request: it answers the endpoint the token
/// minted and stores nothing new. The rows differ in how the same two groups are ordered.
#[tokio::test]
async fn a_replay_may_name_the_same_security_groups_in_any_order() {
    let fixture = fixture();
    let token = "idem-group-order";
    let request = |ids: &[&str]| -> Params {
        let mut params = vec![
            p("SubnetId", fixture.dual_subnet.clone()),
            p("ClientToken", token),
        ];
        params.extend(indexed("SecurityGroupId", ids));
        params
    };

    let first = create_endpoint(
        &fixture.router,
        "the request the token names",
        &request(&[&fixture.default_group, &fixture.extra_group]),
    )
    .await;

    for (case, ids) in [
        (
            "the same order",
            [fixture.default_group.clone(), fixture.extra_group.clone()],
        ),
        (
            "the reverse order",
            [fixture.extra_group.clone(), fixture.default_group.clone()],
        ),
    ] {
        let replayed = create_endpoint(
            &fixture.router,
            case,
            &request(&[ids[0].as_str(), ids[1].as_str()]),
        )
        .await;
        assert_eq!(
            replayed, first,
            "case {case:?}: a replay naming the same groups must answer the endpoint the \
             token minted"
        );
    }
    assert_eq!(
        fixture.stored_endpoints(),
        1,
        "a replay stores nothing, whichever order it names the groups in"
    );
}

/// One row of the DNS-suffix table: a Region and the DNS suffix its partition publishes.
///
/// The suffixes are the `dnsSuffix` each partition publishes in the AWS SDK partition
/// metadata (`endpoints.json`), the same source the crate's `NON_COMMERCIAL_PARTITIONS`
/// table names: `aws` and `aws-us-gov` `amazonaws.com`, `aws-cn` `amazonaws.com.cn`,
/// `aws-iso` `c2s.ic.gov`, `aws-iso-b` `sc2s.sgov.gov`, `aws-iso-e` `cloud.adc-e.uk`,
/// `aws-iso-f` `csp.hci.ic.gov`. Every partition is covered, not only the Region the rest
/// of this suite calls in.
const DNS_SUFFIX_TABLE: [(&str, &str); 7] = [
    ("eu-central-1", "amazonaws.com"),
    ("cn-northwest-1", "amazonaws.com.cn"),
    ("us-gov-east-1", "amazonaws.com"),
    ("us-iso-west-1", "c2s.ic.gov"),
    ("us-isob-east-1", "sc2s.sgov.gov"),
    ("us-isof-south-1", "csp.hci.ic.gov"),
    ("eu-isoe-west-1", "cloud.adc-e.uk"),
];

/// An endpoint's `DnsName` and `FipsDnsName` name the endpoint, the Region it was created
/// in, and the DNS suffix of THAT REGION'S PARTITION — so an endpoint outside the
/// commercial partition does not report a commercial name. This is the rule the created
/// endpoint's names in [`a_created_endpoint_round_trips_through_the_describe`] are one row
/// of; the table below spans every partition the crate knows.
#[test]
fn the_endpoint_dns_names_carry_the_regions_partition_suffix() {
    let endpoint_id = format!("{INSTANCE_CONNECT_ENDPOINT_ID_PREFIX}0123456789abcdef0");
    assert!(
        instance_connect_endpoint_id_is_well_formed(&endpoint_id),
        "the table's endpoint id must be a well-formed one, got {endpoint_id:?}"
    );

    for (region, suffix) in DNS_SUFFIX_TABLE {
        let plain = instance_connect_endpoint_dns_name(&endpoint_id, region);
        let fips = instance_connect_endpoint_fips_dns_name(&endpoint_id, region);
        for (member, name) in [("DnsName", &plain), ("FipsDnsName", &fips)] {
            assert!(
                name.starts_with(&format!("{endpoint_id}.")),
                "case {region:?}/{member}: expected the name to start with the endpoint id \
                 {endpoint_id:?}, got {name:?}"
            );
            assert!(
                name.ends_with(&format!(".{region}.{suffix}")),
                "case {region:?}/{member}: expected the name to end in the Region and its \
                 partition's DNS suffix {suffix:?}, got {name:?}"
            );
        }
        assert_ne!(
            plain, fips,
            "case {region:?}: the FIPS name is a different name from the plain one"
        );
    }
}
