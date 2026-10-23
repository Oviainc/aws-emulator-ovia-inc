// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ModifySubnetAttribute`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=ModifySubnetAttribute`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rule under test is that the operation SETS ONE ATTRIBUTE of the subnet `SubnetId`
//! names — the attribute the request's own member names, to the value that member carries —
//! leaves every other attribute of that subnet as it was, and persists it, so
//! `DescribeSubnets` reports it both as a member of the subnet and through the filter that
//! selects on it. "You can only modify one attribute at a time", so a request naming two
//! is refused, as is one naming none, and a refused request writes nothing.
//!
//! So the tables below span that class rather than one recorded request: EVERY attribute
//! member of `ModifySubnetAttributeRequest`, both boolean values of each boolean one, every
//! member of the model's own `HostnameType` catalog, several device positions, both EC2
//! resource-id forms of a customer-owned pool id — and each refusal the request's own
//! documentation states.

use std::collections::BTreeMap;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, filters, indexed};
use super::wire::{param, Param};

/// The Region every call here is signed for: the one the default network is seeded for.
const REGION: &str = "us-east-1";

/// The response elements the attributes this operation sets are reported through, as the
/// model's own `xmlName`s:
///
/// ```text
/// smithy_query(query="shape",shape="Subnet,PrivateDnsNameOptionsOnLaunch",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
/// ```
///
/// A subnet that carries no value for one of them reports no such element at all, which is
/// how [`attributes`] tells "absent" from "false".
const ATTRIBUTE_ELEMENTS: [&str; 9] = [
    "assignIpv6AddressOnCreation",
    "customerOwnedIpv4Pool",
    "enableDns64",
    "enableLniAtDeviceIndex",
    "enableResourceNameDnsAAAARecord",
    "enableResourceNameDnsARecord",
    "hostnameType",
    "mapCustomerOwnedIpOnLaunch",
    "mapPublicIpOnLaunch",
];

/// A customer-owned IPv4 pool id in the LONG EC2 resource-id form (17 hexadecimal digits).
const COIP_POOL_LONG: &str = "ipv4pool-coip-0abcdef1234567890";

/// The same kind of id in the SHORT form (8 hexadecimal digits), which EC2 resource ids
/// also take.
const COIP_POOL_SHORT: &str = "ipv4pool-coip-0abcdef1";

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it. Every read
/// here goes through the wire, so the engine handle is not needed.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// A VPC with `cidr_block`, as its id — the container the subnets below are created in.
async fn create_vpc(case: &str, router: &MultiServiceRouter, cidr_block: &str) -> String {
    let body = ok(case, router, "CreateVpc", &[param("CidrBlock", cidr_block)]).await;
    element(&body, "vpcId").unwrap_or_else(|| panic!("case {case:?}: no <vpcId> in {body}"))
}

/// A subnet of `vpc_id` with `cidr_block`, as its id.
async fn create_subnet(
    case: &str,
    router: &MultiServiceRouter,
    vpc_id: &str,
    cidr_block: &str,
) -> String {
    let body = ok(
        case,
        router,
        "CreateSubnet",
        &[param("VpcId", vpc_id), param("CidrBlock", cidr_block)],
    )
    .await;
    element(&body, "subnetId").unwrap_or_else(|| panic!("case {case:?}: no <subnetId> in {body}"))
}

/// The XML `DescribeSubnets` answers for the one subnet `subnet_id` names.
async fn describe_one(case: &str, router: &MultiServiceRouter, subnet_id: &str) -> String {
    ok(
        case,
        router,
        "DescribeSubnets",
        &indexed("SubnetId", &[subnet_id]),
    )
    .await
}

/// Every attribute of the subnet `subnet_id` names, as `DescribeSubnets` reports it: the
/// element's text, or `None` when the subnet carries no value for it.
async fn attributes(
    case: &str,
    router: &MultiServiceRouter,
    subnet_id: &str,
) -> BTreeMap<&'static str, Option<String>> {
    let body = describe_one(case, router, subnet_id).await;
    ATTRIBUTE_ELEMENTS
        .iter()
        .map(|name| (*name, element(&body, name)))
        .collect()
}

/// One `ModifySubnetAttribute` of `subnet_id` carrying `params`, as its status and body.
async fn modify(
    router: &MultiServiceRouter,
    subnet_id: &str,
    params: &[Param],
) -> (u16, String) {
    let mut fields = vec![param("SubnetId", subnet_id)];
    fields.extend_from_slice(params);
    call(router, "ModifySubnetAttribute", &fields).await
}

/// One row of the round-trip table: a request, and what the subnet must report afterwards.
struct Row {
    /// What makes this row different from the others.
    case: &'static str,
    /// The IPv4 CIDR block of the fresh subnet this row runs on.
    cidr_block: &'static str,
    /// The `ModifySubnetAttribute` form fields beyond `SubnetId`.
    request: Vec<Param>,
    /// The elements this modification must move, and the text each must then carry
    /// (`None` for "the subnet reports no such element"). Every OTHER element of
    /// [`ATTRIBUTE_ELEMENTS`] must be exactly what it was before the call.
    expected: Vec<(&'static str, Option<&'static str>)>,
}

/// The attribute round trip: every attribute member of `ModifySubnetAttributeRequest`,
/// each applied to a fresh subnet and read back through `DescribeSubnets`.
///
/// The rows differ along what the rule generalises over: which attribute the request names,
/// which value it carries (both booleans, each `HostnameType` member, several device
/// positions, both resource-id forms of a pool id), and how many members one attribute
/// takes (the Outpost rack pair takes two).
fn round_trip_rows() -> Vec<Row> {
    let boolean = |member: &str, value: bool| {
        vec![param(
            &format!("{member}.Value"),
            if value { "true" } else { "false" },
        )]
    };
    vec![
        Row {
            case: "public IPv4 on launch on",
            cidr_block: "10.0.1.0/24",
            request: boolean("MapPublicIpOnLaunch", true),
            expected: vec![("mapPublicIpOnLaunch", Some("true"))],
        },
        Row {
            case: "public IPv4 on launch off",
            cidr_block: "10.0.2.0/24",
            request: boolean("MapPublicIpOnLaunch", false),
            expected: vec![("mapPublicIpOnLaunch", Some("false"))],
        },
        Row {
            case: "IPv6 address on creation on",
            cidr_block: "10.0.3.0/24",
            request: boolean("AssignIpv6AddressOnCreation", true),
            expected: vec![("assignIpv6AddressOnCreation", Some("true"))],
        },
        Row {
            case: "IPv6 address on creation off",
            cidr_block: "10.0.4.0/24",
            request: boolean("AssignIpv6AddressOnCreation", false),
            expected: vec![("assignIpv6AddressOnCreation", Some("false"))],
        },
        Row {
            case: "DNS64 on",
            cidr_block: "10.0.5.0/24",
            request: boolean("EnableDns64", true),
            expected: vec![("enableDns64", Some("true"))],
        },
        Row {
            case: "DNS64 off",
            cidr_block: "10.0.6.0/24",
            request: boolean("EnableDns64", false),
            expected: vec![("enableDns64", Some("false"))],
        },
        Row {
            case: "resource name DNS A record on launch on",
            cidr_block: "10.0.7.0/24",
            request: boolean("EnableResourceNameDnsARecordOnLaunch", true),
            expected: vec![("enableResourceNameDnsARecord", Some("true"))],
        },
        Row {
            case: "resource name DNS AAAA record on launch on",
            cidr_block: "10.0.8.0/24",
            request: boolean("EnableResourceNameDnsAAAARecordOnLaunch", true),
            expected: vec![("enableResourceNameDnsAAAARecord", Some("true"))],
        },
        // Both members of the model's own `HostnameType` catalog, which
        // [`the_hostname_type_catalog_is_the_models_own`] pins this list against.
        Row {
            case: "hostname type resource-name",
            cidr_block: "10.0.9.0/24",
            request: vec![param("PrivateDnsHostnameTypeOnLaunch", "resource-name")],
            expected: vec![("hostnameType", Some("resource-name"))],
        },
        Row {
            case: "hostname type ip-name",
            cidr_block: "10.0.10.0/24",
            request: vec![param("PrivateDnsHostnameTypeOnLaunch", "ip-name")],
            expected: vec![("hostnameType", Some("ip-name"))],
        },
        // "To modify a subnet on an Outpost rack, set both MapCustomerOwnedIpOnLaunch and
        // CustomerOwnedIpv4Pool. These two parameters act as a single attribute", so one
        // row's request carries two members and moves two elements.
        Row {
            case: "customer-owned IPv4 on launch on, long pool id",
            cidr_block: "10.0.11.0/24",
            request: vec![
                param("MapCustomerOwnedIpOnLaunch.Value", "true"),
                param("CustomerOwnedIpv4Pool", COIP_POOL_LONG),
            ],
            expected: vec![
                ("mapCustomerOwnedIpOnLaunch", Some("true")),
                ("customerOwnedIpv4Pool", Some(COIP_POOL_LONG)),
            ],
        },
        Row {
            case: "customer-owned IPv4 on launch on, short pool id",
            cidr_block: "10.0.12.0/24",
            request: vec![
                param("MapCustomerOwnedIpOnLaunch.Value", "true"),
                param("CustomerOwnedIpv4Pool", COIP_POOL_SHORT),
            ],
            expected: vec![
                ("mapCustomerOwnedIpOnLaunch", Some("true")),
                ("customerOwnedIpv4Pool", Some(COIP_POOL_SHORT)),
            ],
        },
        Row {
            case: "customer-owned IPv4 on launch off",
            cidr_block: "10.0.13.0/24",
            request: boolean("MapCustomerOwnedIpOnLaunch", false),
            expected: vec![
                ("mapCustomerOwnedIpOnLaunch", Some("false")),
                ("customerOwnedIpv4Pool", None),
            ],
        },
        // "1 indicates local network interfaces in this subnet are the secondary network
        // interface (eth1)" — the smallest position, and one further out.
        Row {
            case: "local network interfaces at device index 1",
            cidr_block: "10.0.14.0/24",
            request: vec![param("EnableLniAtDeviceIndex", "1")],
            expected: vec![("enableLniAtDeviceIndex", Some("1"))],
        },
        Row {
            case: "local network interfaces at device index 7",
            cidr_block: "10.0.15.0/24",
            request: vec![param("EnableLniAtDeviceIndex", "7")],
            expected: vec![("enableLniAtDeviceIndex", Some("7"))],
        },
        Row {
            case: "local network interfaces not disabled",
            cidr_block: "10.0.16.0/24",
            request: boolean("DisableLniAtDeviceIndex", false),
            expected: vec![("enableLniAtDeviceIndex", None)],
        },
    ]
}

// ---------------------------------------------------------------------------
// The catalog of hostname types
// ---------------------------------------------------------------------------

/// The hostname types the table above sets are the model's own `HostnameType` members — no
/// more and no fewer — so a member the model gains cannot be left untested here.
#[tokio::test]
async fn the_hostname_type_catalog_is_the_models_own() {
    let mut used: Vec<&str> = round_trip_rows()
        .iter()
        .flat_map(|row| row.request.clone())
        .filter(|(name, _)| name == "PrivateDnsHostnameTypeOnLaunch")
        .map(|(_, value)| value)
        .map(|value| {
            ec2::ops::types::HOSTNAME_TYPE_VALUES
                .iter()
                .copied()
                .find(|modelled| *modelled == value.as_str())
                .unwrap_or("not a HostnameType member")
        })
        .collect();
    used.sort_unstable();
    used.dedup();
    let mut modelled = ec2::ops::types::HOSTNAME_TYPE_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        used, modelled,
        "case \"HostnameType catalog\": the hostname types under test must be the model's \
         own members, got {used:?} against {modelled:?}"
    );
}

// ---------------------------------------------------------------------------
// The round trip
// ---------------------------------------------------------------------------

/// The rule: the request's own member names the ONE attribute to set, the modification is
/// persisted, and no other attribute of the subnet moves.
#[tokio::test]
async fn each_attribute_is_set_and_read_back_through_describe_subnets() {
    let router = emulator();
    let vpc_id = create_vpc("the round trip's VPC", &router, "10.0.0.0/16").await;

    for row in round_trip_rows() {
        let case = row.case;
        let subnet_id = create_subnet(case, &router, &vpc_id, row.cidr_block).await;
        let before = attributes(case, &router, &subnet_id).await;

        let (status, body) = modify(&router, &subnet_id, &row.request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ModifySubnetAttribute, got {status} \
             with body {body}"
        );

        let after = attributes(case, &router, &subnet_id).await;
        for element_name in ATTRIBUTE_ELEMENTS {
            let actual = after.get(element_name).cloned().flatten();
            match row
                .expected
                .iter()
                .find(|(expected_name, _)| *expected_name == element_name)
            {
                // The attribute the request named: exactly the value it asked for.
                Some((_, expected)) => assert_eq!(
                    actual.as_deref(),
                    *expected,
                    "case {case:?}: expected <{element_name}> to be {expected:?} after the \
                     modification, got {actual:?}"
                ),
                // Every other attribute: untouched.
                None => {
                    let was = before.get(element_name).cloned().flatten();
                    assert_eq!(
                        actual, was,
                        "case {case:?}: <{element_name}> must not move, but it went from \
                         {was:?} to {actual:?}"
                    );
                }
            }
        }
    }
}

/// One step of the sequence below: the case, the `ModifySubnetAttribute` fields beyond
/// `SubnetId`, and the attribute elements the subnet must report afterwards.
type Step<'a> = (&'a str, Vec<Param>, Vec<(&'a str, Option<&'a str>)>);

/// The attributes are INDEPENDENT and they PERSIST: a sequence of single-attribute
/// modifications of one subnet leaves each attribute at the value its own last modification
/// set, and no other.
///
/// The steps also cover the one attribute that CLEARS a member rather than setting it:
/// "specify true to indicate that local network interfaces at the current position should be
/// disabled" (`DisableLniAtDeviceIndex`).
#[tokio::test]
async fn a_sequence_of_modifications_leaves_each_attribute_where_its_own_call_put_it() {
    let router = emulator();
    let vpc_id = create_vpc("the sequence's VPC", &router, "10.1.0.0/16").await;
    let subnet_id = create_subnet("the sequence's subnet", &router, &vpc_id, "10.1.1.0/24").await;

    // (case, the request's fields, the whole expected attribute state afterwards)
    let steps: Vec<Step<'_>> = vec![
        (
            "public IPv4 on launch first",
            vec![param("MapPublicIpOnLaunch.Value", "true")],
            vec![("mapPublicIpOnLaunch", Some("true"))],
        ),
        (
            "then DNS64",
            vec![param("EnableDns64.Value", "true")],
            vec![
                ("mapPublicIpOnLaunch", Some("true")),
                ("enableDns64", Some("true")),
            ],
        ),
        (
            "then the hostname type",
            vec![param("PrivateDnsHostnameTypeOnLaunch", "resource-name")],
            vec![
                ("mapPublicIpOnLaunch", Some("true")),
                ("enableDns64", Some("true")),
                ("hostnameType", Some("resource-name")),
            ],
        ),
        (
            "then local network interfaces at eth1",
            vec![param("EnableLniAtDeviceIndex", "1")],
            vec![
                ("mapPublicIpOnLaunch", Some("true")),
                ("enableDns64", Some("true")),
                ("hostnameType", Some("resource-name")),
                ("enableLniAtDeviceIndex", Some("1")),
            ],
        ),
        (
            "then disable them again",
            vec![param("DisableLniAtDeviceIndex.Value", "true")],
            vec![
                ("mapPublicIpOnLaunch", Some("true")),
                ("enableDns64", Some("true")),
                ("hostnameType", Some("resource-name")),
                ("enableLniAtDeviceIndex", None),
            ],
        ),
        (
            "then public IPv4 on launch back off",
            vec![param("MapPublicIpOnLaunch.Value", "false")],
            vec![
                ("mapPublicIpOnLaunch", Some("false")),
                ("enableDns64", Some("true")),
                ("hostnameType", Some("resource-name")),
                ("enableLniAtDeviceIndex", None),
            ],
        ),
    ];

    for (case, request, expected) in steps {
        let (status, body) = modify(&router, &subnet_id, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ModifySubnetAttribute, got {status} \
             with body {body}"
        );
        let held = attributes(case, &router, &subnet_id).await;
        for (element_name, wanted) in expected {
            let actual = held.get(element_name).cloned().flatten();
            assert_eq!(
                actual.as_deref(),
                wanted,
                "case {case:?}: expected <{element_name}> to be {wanted:?}, got {actual:?}"
            );
        }
    }
}

/// The modification reaches the STORE, not just the subnet's own projection: the
/// `DescribeSubnets` filter that selects on an attribute selects the subnet by its new
/// value and no longer by its old one.
///
/// The rows are the documented filter names (`DescribeSubnetsRequest$Filters`) of the
/// attributes this operation sets, each with the modification that moves it.
#[tokio::test]
async fn a_modified_attribute_is_what_the_describe_subnets_filter_selects_on() {
    let router = emulator();
    let vpc_id = create_vpc("the filter table's VPC", &router, "10.2.0.0/16").await;
    // (case, the filter name, the request's fields, the filter value that must now select
    // the subnet, a filter value that must no longer select it)
    let cases: Vec<(&str, &str, Vec<Param>, &str, &str)> = vec![
        (
            "public IPv4 on launch",
            "map-public-ip-on-launch",
            vec![param("MapPublicIpOnLaunch.Value", "true")],
            "true",
            "false",
        ),
        (
            "DNS64",
            "enable-dns64",
            vec![param("EnableDns64.Value", "true")],
            "true",
            "false",
        ),
        (
            "the hostname type",
            "private-dns-name-options-on-launch.hostname-type",
            vec![param("PrivateDnsHostnameTypeOnLaunch", "resource-name")],
            "resource-name",
            "ip-name",
        ),
        (
            "the resource name DNS A record",
            "private-dns-name-options-on-launch.enable-resource-name-dns-a-record",
            vec![param("EnableResourceNameDnsARecordOnLaunch.Value", "true")],
            "true",
            "false",
        ),
        (
            "the device index of local network interfaces",
            "enable-lni-at-device-index",
            vec![param("EnableLniAtDeviceIndex", "3")],
            "3",
            "1",
        ),
        (
            "the customer-owned IPv4 pool",
            "customer-owned-ipv4-pool",
            vec![
                param("MapCustomerOwnedIpOnLaunch.Value", "true"),
                param("CustomerOwnedIpv4Pool", COIP_POOL_LONG),
            ],
            COIP_POOL_LONG,
            "ipv4pool-coip-00000000",
        ),
    ];

    for (index, (case, filter, request, selects, misses)) in cases.iter().enumerate() {
        let cidr_block = format!("10.2.{index}.0/24");
        let subnet_id = create_subnet(case, &router, &vpc_id, &cidr_block).await;
        let (status, body) = modify(&router, &subnet_id, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ModifySubnetAttribute, got {status} \
             with body {body}"
        );

        for (value, must_select) in [(*selects, true), (*misses, false)] {
            let mut params = filters(&[(filter, &[value])]);
            params.extend(indexed("SubnetId", &[&subnet_id]));
            let body = ok(case, &router, "DescribeSubnets", &params).await;
            let selected = super::ec2_query::elements(&body, "subnetId");
            assert_eq!(
                selected.contains(&subnet_id),
                must_select,
                "case {case:?}: the filter {filter}={value:?} must {} select {subnet_id}, \
                 got {selected:?}",
                if must_select { "" } else { "not" }
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The documented refusals
// ---------------------------------------------------------------------------

/// Every refusal this operation makes, each with the EC2 error code and HTTP status it
/// answers — and the proof that a refused request wrote nothing.
///
/// The rows differ along what makes the request bad: two attributes in one request ("you
/// can only modify one attribute at a time"), no attribute at all, a hostname type outside
/// the model's own `HostnameType` enum (refused by that constraint, before the handler
/// runs), the primary network interface as a local-network-interface device position ("a
/// local network interface cannot be the primary network interface (eth0)"), each half of
/// the Outpost rack pair without the other, a pool id that is not a pool id, and a subnet
/// id no subnet of the account has.
#[tokio::test]
async fn a_bad_request_is_refused_with_the_ec2_error_code_for_it_and_writes_nothing() {
    let router = emulator();
    let vpc_id = create_vpc("the refusal table's VPC", &router, "10.3.0.0/16").await;
    let subnet_id = create_subnet("the refusal table's subnet", &router, &vpc_id, "10.3.1.0/24")
        .await;
    let untouched = attributes("before every refusal", &router, &subnet_id).await;

    /// A subnet id of the right shape that no subnet of the account has.
    const ABSENT_SUBNET: &str = "subnet-0000000000000beef";
    // (case, the subnet the request names, its fields beyond SubnetId, the HTTP status, the
    // EC2 error code)
    let cases: Vec<(&str, &str, Vec<Param>, u16, &str)> = vec![
        (
            "two attributes in one request",
            &subnet_id,
            vec![
                param("MapPublicIpOnLaunch.Value", "true"),
                param("EnableDns64.Value", "true"),
            ],
            400,
            ec2::state::INVALID_PARAMETER_COMBINATION,
        ),
        (
            "two attributes, one of them the hostname type",
            &subnet_id,
            vec![
                param("PrivateDnsHostnameTypeOnLaunch", "ip-name"),
                param("EnableLniAtDeviceIndex", "1"),
            ],
            400,
            ec2::state::INVALID_PARAMETER_COMBINATION,
        ),
        (
            "no attribute named at all",
            &subnet_id,
            Vec::new(),
            400,
            ec2::state::MISSING_PARAMETER,
        ),
        (
            "a hostname type the model does not define",
            &subnet_id,
            vec![param("PrivateDnsHostnameTypeOnLaunch", "instance-name")],
            400,
            // The modelled `HostnameType` enum constraint refuses it BEFORE the handler
            // runs, the same way `DescribeVpcAttribute`'s `Attribute` is refused.
            "ValidationError",
        ),
        (
            "a hostname type that is a member in the wrong casing",
            &subnet_id,
            vec![param("PrivateDnsHostnameTypeOnLaunch", "IP-Name")],
            400,
            "ValidationError",
        ),
        (
            "the primary network interface as a device position",
            &subnet_id,
            vec![param("EnableLniAtDeviceIndex", "0")],
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a negative device position",
            &subnet_id,
            vec![param("EnableLniAtDeviceIndex", "-1")],
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "customer-owned IPv4 on launch without a pool",
            &subnet_id,
            vec![param("MapCustomerOwnedIpOnLaunch.Value", "true")],
            400,
            ec2::state::INVALID_PARAMETER_COMBINATION,
        ),
        (
            "a pool without customer-owned IPv4 on launch",
            &subnet_id,
            vec![param("CustomerOwnedIpv4Pool", COIP_POOL_LONG)],
            400,
            ec2::state::INVALID_PARAMETER_COMBINATION,
        ),
        (
            "a pool id that is not a pool id",
            &subnet_id,
            vec![
                param("MapCustomerOwnedIpOnLaunch.Value", "true"),
                param("CustomerOwnedIpv4Pool", "not-a-pool-id"),
            ],
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a pool id of the right prefix but the wrong digits",
            &subnet_id,
            vec![
                param("MapCustomerOwnedIpOnLaunch.Value", "true"),
                param("CustomerOwnedIpv4Pool", "ipv4pool-coip-NOTHEX12"),
            ],
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a subnet that does not exist",
            ABSENT_SUBNET,
            vec![param("MapPublicIpOnLaunch.Value", "true")],
            400,
            ec2::state::INVALID_SUBNET_ID_NOT_FOUND,
        ),
    ];

    for (case, subnet, request, status_expected, code_expected) in cases {
        let (status, body) = modify(&router, subnet, &request).await;
        assert_eq!(
            status, status_expected,
            "case {case:?}: expected HTTP {status_expected} from ModifySubnetAttribute, \
             got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, code_expected,
            "case {case:?}: expected error code {code_expected:?}, got {code:?} with body \
             {body}"
        );
    }

    let held = attributes("after every refusal", &router, &subnet_id).await;
    assert_eq!(
        held, untouched,
        "case \"after every refusal\": a refused modification must write nothing, but the \
         subnet went from {untouched:?} to {held:?}"
    );
}
