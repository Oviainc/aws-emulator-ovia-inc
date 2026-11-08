// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ModifyTransitGateway` and the readers it round-trips through —
//! `DescribeTransitGateways` and `DescribeTransitGatewayRouteTables` — driven over the wire
//! the service actually speaks: `ec2Query`, an `application/x-www-form-urlencoded` POST
//! whose body carries `Action=ModifyTransitGateway`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! The rule under test is that a modification applies THE MEMBERS THE REQUEST CARRIES to
//! the stored transit gateway it names and leaves every other member exactly as it was: the
//! description and each option the request carries replace the stored value, a value outside
//! a published range or enum is refused and writes nothing, the CIDR block set is the stored
//! one with the removals taken out and the additions put in, a default route table role
//! names a route table of that gateway (minting the gateway's default table when a newly
//! enabled role has none), and the gateway is answered in the transient `modifying` state
//! the clock then owes `available`. Each table row is a distinct input along what that
//! generalises over — which member the request carries, each option's value, the ASN at both
//! published bounds of both published ranges, the CIDR block family, size and count, which
//! attachment kind the gateway holds — so the one case a recording happens to contain is one
//! row among many rather than the specification.

use std::sync::Arc;
use std::time::Duration;

use ec2::ops::types::{
    PeeringTgwInfo, TransitGatewayClientVpnAttachment, TransitGatewayConnect,
    TransitGatewayPeeringAttachment, TransitGatewayVpcAttachment,
};
use ec2::state::{
    AMAZON_SIDE_ASN_16_BIT, AMAZON_SIDE_ASN_32_BIT, ComAmazonawsEc2Amazonec2State as Ec2State,
    DEFAULT_AMAZON_SIDE_ASN, ENCRYPTION_STATE_DISABLED, ENCRYPTION_STATE_DISABLING,
    ENCRYPTION_STATE_ENABLED, ENCRYPTION_STATE_ENABLING, OPTION_DISABLE, OPTION_ENABLE,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE, TRANSIT_GATEWAY_ATTACHMENT_STATE_REJECTED,
    TRANSIT_GATEWAY_STATE_AVAILABLE, TRANSIT_GATEWAY_STATE_MODIFYING, TRANSITION_DELAY,
    TransitGatewayAttachmentView,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, filters, indexed};
use super::wire::{Param, child, child_text, document, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// Long enough past [`TRANSITION_DELAY`] that a move the clock owes is certainly due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// The description every gateway in this suite is created with, so a row that changes it is
/// visibly different from one that leaves it alone.
const BASELINE_DESCRIPTION: &str = "the hub gateway";

/// The IPv4 CIDR block every gateway here starts with.
const BASELINE_IPV4_BLOCK: &str = "10.0.0.0/24";

/// The IPv6 CIDR block every gateway here starts with.
const BASELINE_IPV6_BLOCK: &str = "2001:db8::/64";

/// The emulator the tests drive, with the engine handle the rows that seed an attachment
/// reach the store through.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One call of `action` signed for the Region above.
async fn invoke(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One `ModifyTransitGateway` call carrying `params`, as its HTTP status and XML body.
async fn modify(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    invoke(router, "ModifyTransitGateway", params).await
}

/// `ModifyTransitGateway` requiring success, and the body it answered.
async fn modify_ok(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    let (status, body) = modify(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ModifyTransitGateway, got {status} with \
         body {body}"
    );
    body
}

/// `CreateTransitGateway` requiring success, and the id of the gateway it created.
async fn create_ok(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    let (status, body) = invoke(router, "CreateTransitGateway", params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateTransitGateway, got {status} with \
         body {body}"
    );
    member(case, &body, "transitGatewayId")
        .unwrap_or_else(|| panic!("case {case:?}: the created gateway must carry an id: {body}"))
}

/// `DescribeTransitGateways` for one gateway, requiring success.
async fn describe_ok(case: &str, router: &MultiServiceRouter, transit_gateway_id: &str) -> String {
    let (status, body) = invoke(
        router,
        "DescribeTransitGateways",
        &[param("TransitGatewayIds.1", transit_gateway_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeTransitGateways, got {status} with \
         body {body}"
    );
    body
}

/// `DescribeTransitGatewayRouteTables` for one route table, requiring success.
async fn describe_route_table_ok(
    case: &str,
    router: &MultiServiceRouter,
    route_table_id: &str,
) -> String {
    let (status, body) = invoke(
        router,
        "DescribeTransitGatewayRouteTables",
        &indexed("TransitGatewayRouteTableIds", &[route_table_id]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeTransitGatewayRouteTables, got \
         {status} with body {body}"
    );
    body
}

/// The ids of every transit gateway route table of `transit_gateway_id`, in the order
/// `DescribeTransitGatewayRouteTables` reports them.
async fn describe_route_tables_of(
    case: &str,
    router: &MultiServiceRouter,
    transit_gateway_id: &str,
) -> Vec<String> {
    let (status, body) = invoke(
        router,
        "DescribeTransitGatewayRouteTables",
        &filters(&[("transit-gateway-id", &[transit_gateway_id])]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeTransitGatewayRouteTables, got \
         {status} with body {body}"
    );
    elements(&body, "transitGatewayRouteTableId")
}

/// `read` applied to the one transit gateway `body` reports.
///
/// The gateway is found by the member only a gateway carries, so the same reader works on a
/// `ModifyTransitGateway` result and on a `DescribeTransitGateways` page of one.
fn read_gateway<T>(case: &str, body: &str, read: impl FnOnce(roxmltree::Node<'_, '_>) -> T) -> T {
    let parsed = document(case, body);
    let gateway = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "transitGatewayId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no transit gateway in {body}"));
    read(gateway)
}

/// A scalar member of the one transit gateway `body` reports.
fn member(case: &str, body: &str, name: &str) -> Option<String> {
    read_gateway(case, body, |gateway| child_text(gateway, name))
}

/// A member of the `options` of the one transit gateway `body` reports.
fn option(case: &str, body: &str, name: &str) -> Option<String> {
    read_gateway(case, body, |gateway| {
        child(gateway, "options").and_then(|options| child_text(options, name))
    })
}

/// The `<item>` texts of a named set inside the one gateway's `options`.
fn option_items(case: &str, body: &str, name: &str) -> Vec<String> {
    read_gateway(case, body, |gateway| {
        let Some(set) = child(gateway, "options").and_then(|options| child(options, name)) else {
            return Vec::new();
        };
        set.children()
            .filter(|node| node.is_element() && node.tag_name().name() == "item")
            .map(|item| item.text().unwrap_or_default().to_string())
            .collect()
    })
}

/// The `encryptionState` of the gateway's `options/encryptionSupport`, absent when the
/// gateway reports no encryption support at all.
fn encryption_state(case: &str, body: &str) -> Option<String> {
    read_gateway(case, body, |gateway| {
        child(gateway, "options")
            .and_then(|options| child(options, "encryptionSupport"))
            .and_then(|support| child_text(support, "encryptionState"))
    })
}

/// Every member of a transit gateway a modification can reach, as a response reports them.
///
/// Read and asserted as ONE value, so each row pins both what the request changed and what
/// it left alone rather than only the member it carried.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    description: Option<String>,
    amazon_side_asn: Option<String>,
    auto_accept_shared_attachments: Option<String>,
    default_route_table_association: Option<String>,
    default_route_table_propagation: Option<String>,
    vpn_ecmp_support: Option<String>,
    dns_support: Option<String>,
    security_group_referencing_support: Option<String>,
    multicast_support: Option<String>,
    cidr_blocks: Vec<String>,
}

impl Reported {
    /// What a gateway created by [`baseline_params`] reports, before any modification: the
    /// values that request chose, and every option it left out at the default
    /// `TransitGatewayRequestOptions`'s own member docs publish.
    fn baseline() -> Self {
        Self {
            description: Some(BASELINE_DESCRIPTION.to_string()),
            amazon_side_asn: Some(DEFAULT_AMAZON_SIDE_ASN.to_string()),
            auto_accept_shared_attachments: Some(OPTION_ENABLE.to_string()),
            default_route_table_association: Some(OPTION_ENABLE.to_string()),
            default_route_table_propagation: Some(OPTION_ENABLE.to_string()),
            vpn_ecmp_support: Some(OPTION_ENABLE.to_string()),
            dns_support: Some(OPTION_ENABLE.to_string()),
            security_group_referencing_support: Some(OPTION_ENABLE.to_string()),
            multicast_support: Some(OPTION_ENABLE.to_string()),
            cidr_blocks: vec![
                BASELINE_IPV4_BLOCK.to_string(),
                BASELINE_IPV6_BLOCK.to_string(),
            ],
        }
    }

    fn with_description(mut self, description: &str) -> Self {
        self.description = Some(description.to_string());
        self
    }

    fn with_asn(mut self, asn: i64) -> Self {
        self.amazon_side_asn = Some(asn.to_string());
        self
    }

    fn with_cidr_blocks(mut self, blocks: &[&str]) -> Self {
        self.cidr_blocks = blocks.iter().map(|block| (*block).to_string()).collect();
        self
    }
}

/// Everything [`Reported`] holds, read off one response body.
fn reported(case: &str, body: &str) -> Reported {
    Reported {
        description: member(case, body, "description"),
        amazon_side_asn: option(case, body, "amazonSideAsn"),
        auto_accept_shared_attachments: option(case, body, "autoAcceptSharedAttachments"),
        default_route_table_association: option(case, body, "defaultRouteTableAssociation"),
        default_route_table_propagation: option(case, body, "defaultRouteTablePropagation"),
        vpn_ecmp_support: option(case, body, "vpnEcmpSupport"),
        dns_support: option(case, body, "dnsSupport"),
        security_group_referencing_support: option(
            case,
            body,
            "securityGroupReferencingSupport",
        ),
        multicast_support: option(case, body, "multicastSupport"),
        cidr_blocks: option_items(case, body, "transitGatewayCidrBlocks"),
    }
}

/// The `CreateTransitGateway` request every gateway in this suite starts from: a description,
/// two options set AGAINST their published defaults (so a modification towards the default is
/// as visible as one away from it), the one option no modification can reach
/// (`MulticastSupport`), and one CIDR block of each family.
fn baseline_params() -> Vec<Param> {
    vec![
        param("Description", BASELINE_DESCRIPTION),
        param("Options.AutoAcceptSharedAttachments", OPTION_ENABLE),
        param("Options.SecurityGroupReferencingSupport", OPTION_ENABLE),
        param("Options.MulticastSupport", OPTION_ENABLE),
        param("Options.TransitGatewayCidrBlocks.1", BASELINE_IPV4_BLOCK),
        param("Options.TransitGatewayCidrBlocks.2", BASELINE_IPV6_BLOCK),
    ]
}

/// The `TransitGatewayId=` request member every modification carries.
fn id_param(transit_gateway_id: &str) -> Param {
    param("TransitGatewayId", transit_gateway_id)
}

/// `params` with the gateway id in front of them, as one modification request.
fn request(transit_gateway_id: &str, params: &[Param]) -> Vec<Param> {
    let mut fields = vec![id_param(transit_gateway_id)];
    fields.extend(params.iter().cloned());
    fields
}

// ---------------------------------------------------------------------------
// What a modification applies, and what it leaves alone
// ---------------------------------------------------------------------------

/// The rule: `ModifyTransitGateway` applies the members the request carries to the gateway it
/// names — the description, each option, and the CIDR block set with the removals taken out
/// and the additions put in — leaves every member the request does not carry as it was, and
/// every one of them reads back through `DescribeTransitGateways`.
///
/// The rows differ in which member the request carries, each option's value, the ASN at both
/// published bounds of both published ranges, and the family, size and count of the CIDR
/// blocks added and removed.
#[tokio::test]
async fn modify_transit_gateway_applies_the_members_the_request_carries() {
    let (router, _engine) = emulator();
    // (case, the modification's form fields, what the gateway must then report)
    let cases: Vec<(&str, Vec<Param>, Reported)> = vec![
        ("nothing but the gateway id", Vec::new(), Reported::baseline()),
        (
            "a new description",
            vec![param("Description", "the spoke gateway")],
            Reported::baseline().with_description("the spoke gateway"),
        ),
        (
            "DNS support turned off",
            vec![param("Options.DnsSupport", OPTION_DISABLE)],
            Reported {
                dns_support: Some(OPTION_DISABLE.to_string()),
                ..Reported::baseline()
            },
        ),
        (
            "ECMP support turned off",
            vec![param("Options.VpnEcmpSupport", OPTION_DISABLE)],
            Reported {
                vpn_ecmp_support: Some(OPTION_DISABLE.to_string()),
                ..Reported::baseline()
            },
        ),
        (
            "automatic acceptance turned off",
            vec![param("Options.AutoAcceptSharedAttachments", OPTION_DISABLE)],
            Reported {
                auto_accept_shared_attachments: Some(OPTION_DISABLE.to_string()),
                ..Reported::baseline()
            },
        ),
        (
            "security group referencing turned off",
            vec![param(
                "Options.SecurityGroupReferencingSupport",
                OPTION_DISABLE,
            )],
            Reported {
                security_group_referencing_support: Some(OPTION_DISABLE.to_string()),
                ..Reported::baseline()
            },
        ),
        (
            "default route table association turned off",
            vec![param("Options.DefaultRouteTableAssociation", OPTION_DISABLE)],
            Reported {
                default_route_table_association: Some(OPTION_DISABLE.to_string()),
                ..Reported::baseline()
            },
        ),
        (
            "default route table propagation turned off",
            vec![param("Options.DefaultRouteTablePropagation", OPTION_DISABLE)],
            Reported {
                default_route_table_propagation: Some(OPTION_DISABLE.to_string()),
                ..Reported::baseline()
            },
        ),
        (
            "two options at once",
            vec![
                param("Options.DnsSupport", OPTION_DISABLE),
                param("Options.VpnEcmpSupport", OPTION_DISABLE),
            ],
            Reported {
                dns_support: Some(OPTION_DISABLE.to_string()),
                vpn_ecmp_support: Some(OPTION_DISABLE.to_string()),
                ..Reported::baseline()
            },
        ),
        (
            "the description and an option together",
            vec![
                param("Description", "the edge gateway"),
                param("Options.DnsSupport", OPTION_DISABLE),
            ],
            Reported {
                dns_support: Some(OPTION_DISABLE.to_string()),
                ..Reported::baseline().with_description("the edge gateway")
            },
        ),
        (
            "the 16-bit ASN range's floor",
            vec![param(
                "Options.AmazonSideAsn",
                &AMAZON_SIDE_ASN_16_BIT.start().to_string(),
            )],
            Reported::baseline().with_asn(*AMAZON_SIDE_ASN_16_BIT.start()),
        ),
        (
            "the 16-bit ASN range's ceiling",
            vec![param(
                "Options.AmazonSideAsn",
                &AMAZON_SIDE_ASN_16_BIT.end().to_string(),
            )],
            Reported::baseline().with_asn(*AMAZON_SIDE_ASN_16_BIT.end()),
        ),
        (
            "the 32-bit ASN range's floor",
            vec![param(
                "Options.AmazonSideAsn",
                &AMAZON_SIDE_ASN_32_BIT.start().to_string(),
            )],
            Reported::baseline().with_asn(*AMAZON_SIDE_ASN_32_BIT.start()),
        ),
        (
            "the 32-bit ASN range's ceiling",
            vec![param(
                "Options.AmazonSideAsn",
                &AMAZON_SIDE_ASN_32_BIT.end().to_string(),
            )],
            Reported::baseline().with_asn(*AMAZON_SIDE_ASN_32_BIT.end()),
        ),
        (
            "an IPv4 block added at the published minimum size",
            vec![param(
                "Options.AddTransitGatewayCidrBlocks.1",
                "192.168.7.0/24",
            )],
            Reported::baseline().with_cidr_blocks(&[
                BASELINE_IPV4_BLOCK,
                BASELINE_IPV6_BLOCK,
                "192.168.7.0/24",
            ]),
        ),
        (
            "an IPv6 block added larger than the published minimum",
            vec![param(
                "Options.AddTransitGatewayCidrBlocks.1",
                "2001:db8:9::/56",
            )],
            Reported::baseline().with_cidr_blocks(&[
                BASELINE_IPV4_BLOCK,
                BASELINE_IPV6_BLOCK,
                "2001:db8:9::/56",
            ]),
        ),
        (
            "an added block with its host bits set, stored canonically",
            vec![param(
                "Options.AddTransitGatewayCidrBlocks.1",
                "172.16.4.9/22",
            )],
            Reported::baseline().with_cidr_blocks(&[
                BASELINE_IPV4_BLOCK,
                BASELINE_IPV6_BLOCK,
                "172.16.4.0/22",
            ]),
        ),
        (
            "two blocks added at once",
            vec![
                param("Options.AddTransitGatewayCidrBlocks.1", "10.4.0.0/24"),
                param("Options.AddTransitGatewayCidrBlocks.2", "10.5.0.0/24"),
            ],
            Reported::baseline().with_cidr_blocks(&[
                BASELINE_IPV4_BLOCK,
                BASELINE_IPV6_BLOCK,
                "10.4.0.0/24",
                "10.5.0.0/24",
            ]),
        ),
        (
            "a block the gateway already holds, added again",
            vec![param(
                "Options.AddTransitGatewayCidrBlocks.1",
                BASELINE_IPV4_BLOCK,
            )],
            Reported::baseline(),
        ),
        (
            "one block removed",
            vec![param(
                "Options.RemoveTransitGatewayCidrBlocks.1",
                BASELINE_IPV4_BLOCK,
            )],
            Reported::baseline().with_cidr_blocks(&[BASELINE_IPV6_BLOCK]),
        ),
        (
            "every block removed",
            vec![
                param(
                    "Options.RemoveTransitGatewayCidrBlocks.1",
                    BASELINE_IPV4_BLOCK,
                ),
                param(
                    "Options.RemoveTransitGatewayCidrBlocks.2",
                    BASELINE_IPV6_BLOCK,
                ),
            ],
            Reported::baseline().with_cidr_blocks(&[]),
        ),
        (
            "a block the gateway does not hold, removed",
            vec![param(
                "Options.RemoveTransitGatewayCidrBlocks.1",
                "10.99.0.0/24",
            )],
            Reported::baseline(),
        ),
        (
            "one block added and another removed",
            vec![
                param("Options.AddTransitGatewayCidrBlocks.1", "10.6.0.0/24"),
                param(
                    "Options.RemoveTransitGatewayCidrBlocks.1",
                    BASELINE_IPV6_BLOCK,
                ),
            ],
            Reported::baseline().with_cidr_blocks(&[BASELINE_IPV4_BLOCK, "10.6.0.0/24"]),
        ),
        (
            "one block named in both lists, which the gateway is left holding",
            vec![
                param(
                    "Options.AddTransitGatewayCidrBlocks.1",
                    BASELINE_IPV4_BLOCK,
                ),
                param(
                    "Options.RemoveTransitGatewayCidrBlocks.1",
                    BASELINE_IPV4_BLOCK,
                ),
            ],
            Reported::baseline().with_cidr_blocks(&[BASELINE_IPV6_BLOCK, BASELINE_IPV4_BLOCK]),
        ),
    ];

    for (case, params, expected) in cases {
        // A gateway of its own per row, so what one modification leaves behind cannot be
        // read as another's work.
        let transit_gateway_id = create_ok(case, &router, &baseline_params()).await;
        let answered = modify_ok(case, &router, &request(&transit_gateway_id, &params)).await;
        let actual = reported(case, &answered);
        assert_eq!(
            actual, expected,
            "case {case:?}: ModifyTransitGateway answered {actual:?}, expected {expected:?} \
             in {answered}"
        );
        // The same record, read back through the reader every other caller sees it with.
        let read = describe_ok(case, &router, &transit_gateway_id).await;
        let described = reported(case, &read);
        assert_eq!(
            described, expected,
            "case {case:?}: DescribeTransitGateways reported {described:?}, expected \
             {expected:?} in {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// What a modification refuses
// ---------------------------------------------------------------------------

/// The rule: a value outside a published range, a value that is not a CIDR block of an
/// admitted size, and an id naming no resource of this gateway are all REFUSED, and a refused
/// modification writes nothing.
///
/// The rows walk one step past each published bound of each published ASN range, each family's
/// published minimum block size on both the add and the remove list, a value that is not a
/// block at all, a value no option enum has, and each of the two refusals a default route
/// table id gets.
#[tokio::test]
async fn modify_transit_gateway_refuses_a_value_outside_a_published_range_or_enum() {
    let (router, _engine) = emulator();
    let transit_gateway_id = create_ok("the gateway", &router, &baseline_params()).await;
    // A second gateway, so "a route table of another gateway" is a real route table rather
    // than an id nothing holds.
    let other_id = create_ok("another gateway", &router, &baseline_params()).await;
    let other_route_table = option(
        "another gateway",
        &describe_ok("another gateway", &router, &other_id).await,
        "associationDefaultRouteTableId",
    )
    .expect("the other gateway's default association route table");

    // (case, the modification's form fields, the EC2 error code)
    let cases: Vec<(&str, Vec<Param>, String)> = vec![
        (
            "one below the 16-bit ASN range",
            vec![param(
                "Options.AmazonSideAsn",
                &(AMAZON_SIDE_ASN_16_BIT.start() - 1).to_string(),
            )],
            ec2::state::INVALID_PARAMETER_VALUE.to_string(),
        ),
        (
            "one above the 16-bit ASN range",
            vec![param(
                "Options.AmazonSideAsn",
                &(AMAZON_SIDE_ASN_16_BIT.end() + 1).to_string(),
            )],
            ec2::state::INVALID_PARAMETER_VALUE.to_string(),
        ),
        (
            "one below the 32-bit ASN range",
            vec![param(
                "Options.AmazonSideAsn",
                &(AMAZON_SIDE_ASN_32_BIT.start() - 1).to_string(),
            )],
            ec2::state::INVALID_PARAMETER_VALUE.to_string(),
        ),
        (
            "one above the 32-bit ASN range",
            vec![param(
                "Options.AmazonSideAsn",
                &(AMAZON_SIDE_ASN_32_BIT.end() + 1).to_string(),
            )],
            ec2::state::INVALID_PARAMETER_VALUE.to_string(),
        ),
        (
            "an added IPv4 block one bit smaller than the minimum size",
            vec![param("Options.AddTransitGatewayCidrBlocks.1", "10.20.0.0/25")],
            ec2::state::INVALID_PARAMETER_VALUE.to_string(),
        ),
        (
            "an added IPv6 block one bit smaller than the minimum size",
            vec![param("Options.AddTransitGatewayCidrBlocks.1", "2001:db8::/65")],
            ec2::state::INVALID_PARAMETER_VALUE.to_string(),
        ),
        (
            "an added value that is not a CIDR block",
            vec![param(
                "Options.AddTransitGatewayCidrBlocks.1",
                "ten dot zero",
            )],
            ec2::state::INVALID_PARAMETER_VALUE.to_string(),
        ),
        (
            "an added IPv4 block with no prefix length",
            vec![param("Options.AddTransitGatewayCidrBlocks.1", "10.21.0.0")],
            ec2::state::INVALID_PARAMETER_VALUE.to_string(),
        ),
        (
            "a removed value that is not a CIDR block",
            vec![param(
                "Options.RemoveTransitGatewayCidrBlocks.1",
                "ten dot zero",
            )],
            ec2::state::INVALID_PARAMETER_VALUE.to_string(),
        ),
        (
            "a removed IPv4 block one bit smaller than the minimum size",
            vec![param(
                "Options.RemoveTransitGatewayCidrBlocks.1",
                "10.20.0.0/25",
            )],
            ec2::state::INVALID_PARAMETER_VALUE.to_string(),
        ),
        (
            // Refused by the boundary validator, which reads the model's own enum: the
            // member is a `DnsSupportValue`, so a value outside it never reaches the
            // handler, and the code is the boundary's rather than the handler's.
            "a value no option enum has",
            vec![param("Options.DnsSupport", "enabled")],
            "ValidationError".to_string(),
        ),
        (
            "an association default route table id that is not one at all",
            vec![param("Options.AssociationDefaultRouteTableId", "rtb-12345678")],
            ec2::state::INVALID_ROUTE_TABLE_ID_MALFORMED.to_string(),
        ),
        (
            "a propagation default route table id that is not one at all",
            vec![param(
                "Options.PropagationDefaultRouteTableId",
                "not-a-route-table",
            )],
            ec2::state::INVALID_ROUTE_TABLE_ID_MALFORMED.to_string(),
        ),
        (
            "a well-formed route table id no route table has",
            vec![param(
                "Options.AssociationDefaultRouteTableId",
                "tgw-rtb-0123456789abcdef0",
            )],
            ec2::state::INVALID_ROUTE_TABLE_ID_NOT_FOUND.to_string(),
        ),
        (
            "a route table of another transit gateway",
            vec![param(
                "Options.AssociationDefaultRouteTableId",
                &other_route_table,
            )],
            ec2::state::INVALID_ROUTE_TABLE_ID_NOT_FOUND.to_string(),
        ),
    ];

    for (case, params, expected) in cases {
        let (status, body) = modify(&router, &request(&transit_gateway_id, &params)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from ModifyTransitGateway, got {status} with \
             body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected,
            "case {case:?}: expected the error code {expected:?}, got {code:?} in {body}"
        );
        // Nothing was written: the gateway still reports what it was created with.
        let read = describe_ok(case, &router, &transit_gateway_id).await;
        let described = reported(case, &read);
        assert_eq!(
            described,
            Reported::baseline(),
            "case {case:?}: a refused modification must write nothing, got {described:?} \
             in {read}"
        );
    }

    // An id this account holds no transit gateway for has nothing to modify, whatever the
    // request carries beside it.
    for (case, params) in [
        ("an unknown gateway, nothing else", Vec::new()),
        (
            "an unknown gateway and a description",
            vec![param("Description", "nowhere")],
        ),
    ] {
        let mut fields = vec![id_param("tgw-0123456789abcdef0")];
        fields.extend(params);
        let (status, body) = modify(&router, &fields).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for an unknown gateway, got {status} with \
             body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code,
            ec2::state::INVALID_TRANSIT_GATEWAY_ID_NOT_FOUND,
            "case {case:?}: expected the NotFound code, got {code:?} in {body}"
        );
    }
}

/// The rule: "If you have the required permissions, the error response is DryRunOperation"
/// (`DryRun`) — a dry run that would have succeeded reports that outcome and writes nothing.
///
/// The rows differ in how much of the modification the dry run carries, so the refusal
/// cannot depend on one request's shape.
#[tokio::test]
async fn a_dry_run_modification_reports_the_outcome_and_stores_nothing() {
    let (router, _engine) = emulator();
    let transit_gateway_id = create_ok("the gateway", &router, &baseline_params()).await;

    let cases: Vec<(&str, Vec<Param>)> = vec![
        ("nothing but the gateway id", Vec::new()),
        (
            "a description",
            vec![param("Description", "never written")],
        ),
        (
            "an option and a CIDR block",
            vec![
                param("Options.DnsSupport", OPTION_DISABLE),
                param("Options.AddTransitGatewayCidrBlocks.1", "10.30.0.0/24"),
            ],
        ),
    ];

    for (case, params) in cases {
        let mut fields = request(&transit_gateway_id, &params);
        fields.push(param("DryRun", "true"));
        let (status, body) = modify(&router, &fields).await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS} from a dry run, got {status} \
             with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code,
            ec2::state::DRY_RUN_OPERATION,
            "case {case:?}: expected the DryRunOperation code, got {code:?} in {body}"
        );
        let read = describe_ok(case, &router, &transit_gateway_id).await;
        let described = reported(case, &read);
        assert_eq!(
            described,
            Reported::baseline(),
            "case {case:?}: a dry run must write nothing, got {described:?} in {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// The state the modification leaves the gateway in
// ---------------------------------------------------------------------------

/// The rule: a modification answers the gateway in the TRANSIENT `modifying` state and the
/// clock owes it `available` one transition later — the span is never zero, so a caller
/// acting straight after the call still sees the modification being carried out, and nothing
/// but time is needed to finish it.
///
/// The rows differ in what the modification carried, so the move is a property of the
/// modification rather than of one request's shape.
#[tokio::test]
async fn a_modified_transit_gateway_becomes_available_again_on_the_clock() {
    let (router, _engine) = emulator();
    let cases: Vec<(&str, Vec<Param>)> = vec![
        ("nothing but the gateway id", Vec::new()),
        (
            "a description",
            vec![param("Description", "the settled gateway")],
        ),
        (
            "an option",
            vec![param("Options.DnsSupport", OPTION_DISABLE)],
        ),
    ];

    let mut modified: Vec<(&str, String)> = Vec::new();
    for (case, params) in cases {
        let transit_gateway_id = create_ok(case, &router, &baseline_params()).await;
        // The gateway is `available` before the modification, so what the rows below show is
        // the modification's own move rather than the create's.
        tokio::time::sleep(past_the_transition()).await;
        let settled = describe_ok(case, &router, &transit_gateway_id).await;
        assert_eq!(
            member(case, &settled, "state").as_deref(),
            Some(TRANSIT_GATEWAY_STATE_AVAILABLE),
            "case {case:?}: the created gateway must be available before it is modified, \
             got {settled}"
        );

        let answered = modify_ok(case, &router, &request(&transit_gateway_id, &params)).await;
        assert_eq!(
            member(case, &answered, "state").as_deref(),
            Some(TRANSIT_GATEWAY_STATE_MODIFYING),
            "case {case:?}: expected the transient state of a modification, got {answered}"
        );
        let read = describe_ok(case, &router, &transit_gateway_id).await;
        assert_eq!(
            member(case, &read, "state").as_deref(),
            Some(TRANSIT_GATEWAY_STATE_MODIFYING),
            "case {case:?}: the store must hold what the modification reported, got {read}"
        );
        modified.push((case, transit_gateway_id));
    }

    // The move falls due on its own, with nobody asking for it.
    tokio::time::sleep(past_the_transition()).await;

    for (case, transit_gateway_id) in modified {
        let read = describe_ok(case, &router, &transit_gateway_id).await;
        assert_eq!(
            member(case, &read, "state").as_deref(),
            Some(TRANSIT_GATEWAY_STATE_AVAILABLE),
            "case {case:?}: the clock owed this gateway available, got {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// The default route table roles
// ---------------------------------------------------------------------------

/// The rule: a default route table role the gateway's options enable names a route table of
/// THAT gateway — the table the other role names, or one the store mints when the gateway has
/// none ("If `defaultRouteTablePropagation` is set to `enable`, Amazon Web Services Transit
/// Gateway will create the default transit gateway route table") — a role that is disabled
/// names none, and every route table of the gateway reports the roles its options name it
/// for.
#[tokio::test]
async fn enabling_a_default_route_table_role_names_a_route_table_of_the_gateway() {
    let (router, _engine) = emulator();
    // A gateway created with neither default role has no default route table at all.
    let transit_gateway_id = create_ok(
        "neither role",
        &router,
        &[
            param("Options.DefaultRouteTableAssociation", OPTION_DISABLE),
            param("Options.DefaultRouteTablePropagation", OPTION_DISABLE),
        ],
    )
    .await;
    let created = describe_ok("neither role", &router, &transit_gateway_id).await;
    assert_eq!(
        option("neither role", &created, "associationDefaultRouteTableId"),
        None,
        "a gateway created with neither default role must name no association table: \
         {created}"
    );
    assert_eq!(
        option("neither role", &created, "propagationDefaultRouteTableId"),
        None,
        "a gateway created with neither default role must name no propagation table: \
         {created}"
    );

    // Enabling propagation mints the gateway's default route table.
    let case = "propagation enabled";
    let answered = modify_ok(
        case,
        &router,
        &request(
            &transit_gateway_id,
            &[param("Options.DefaultRouteTablePropagation", OPTION_ENABLE)],
        ),
    )
    .await;
    let minted = option(case, &answered, "propagationDefaultRouteTableId")
        .unwrap_or_else(|| panic!("case {case:?}: a propagation table must be named: {answered}"));
    assert!(
        minted.starts_with("tgw-rtb-"),
        "case {case:?}: expected a transit gateway route table id, got {minted:?}"
    );
    assert_eq!(
        option(case, &answered, "associationDefaultRouteTableId"),
        None,
        "case {case:?}: a disabled role must name no table, got {answered}"
    );
    let table = describe_route_table_ok(case, &router, &minted).await;
    for (name, expected) in [
        ("transitGatewayId", transit_gateway_id.as_str()),
        ("defaultPropagationRouteTable", "true"),
        ("defaultAssociationRouteTable", "false"),
    ] {
        let actual = element(&table, name);
        assert_eq!(
            actual.as_deref(),
            Some(expected),
            "case {case:?}: expected <{name}> {expected:?} on the minted table, got \
             {actual:?} in {table}"
        );
    }

    // Enabling association as well puts BOTH roles on that one table rather than minting a
    // second one.
    let case = "association enabled too";
    let answered = modify_ok(
        case,
        &router,
        &request(
            &transit_gateway_id,
            &[param("Options.DefaultRouteTableAssociation", OPTION_ENABLE)],
        ),
    )
    .await;
    assert_eq!(
        option(case, &answered, "associationDefaultRouteTableId").as_deref(),
        Some(minted.as_str()),
        "case {case:?}: one table serves both default roles, got {answered}"
    );
    assert_eq!(
        option(case, &answered, "propagationDefaultRouteTableId").as_deref(),
        Some(minted.as_str()),
        "case {case:?}: the propagation role must still name its table, got {answered}"
    );
    let table = describe_route_table_ok(case, &router, &minted).await;
    assert_eq!(
        element(&table, "defaultAssociationRouteTable").as_deref(),
        Some("true"),
        "case {case:?}: the table must report the association role, got {table}"
    );

    // An ADDITIONAL route table of the gateway can be named as the default association
    // table, which moves that role off the minted one.
    let case = "an additional table named as the association default";
    let (status, body) = invoke(
        &router,
        "CreateTransitGatewayRouteTable",
        &[id_param(&transit_gateway_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateTransitGatewayRouteTable, got {status} \
         with body {body}"
    );
    let additional = element(&body, "transitGatewayRouteTableId")
        .unwrap_or_else(|| panic!("case {case:?}: the created table must carry an id: {body}"));
    let answered = modify_ok(
        case,
        &router,
        &request(
            &transit_gateway_id,
            &[param("Options.AssociationDefaultRouteTableId", &additional)],
        ),
    )
    .await;
    assert_eq!(
        option(case, &answered, "associationDefaultRouteTableId").as_deref(),
        Some(additional.as_str()),
        "case {case:?}: the named table must hold the association role, got {answered}"
    );
    let moved_to = describe_route_table_ok(case, &router, &additional).await;
    assert_eq!(
        element(&moved_to, "defaultAssociationRouteTable").as_deref(),
        Some("true"),
        "case {case:?}: the named table must report the association role, got {moved_to}"
    );
    let moved_from = describe_route_table_ok(case, &router, &minted).await;
    assert_eq!(
        element(&moved_from, "defaultAssociationRouteTable").as_deref(),
        Some("false"),
        "case {case:?}: the table the role moved off must report it no longer, got \
         {moved_from}"
    );

    // Disabling a role leaves the gateway naming no table for it, and the table reporting
    // the role no longer.
    let case = "propagation disabled again";
    let answered = modify_ok(
        case,
        &router,
        &request(
            &transit_gateway_id,
            &[param("Options.DefaultRouteTablePropagation", OPTION_DISABLE)],
        ),
    )
    .await;
    assert_eq!(
        option(case, &answered, "propagationDefaultRouteTableId"),
        None,
        "case {case:?}: a disabled role must name no table, got {answered}"
    );
    let table = describe_route_table_ok(case, &router, &minted).await;
    assert_eq!(
        element(&table, "defaultPropagationRouteTable").as_deref(),
        Some("false"),
        "case {case:?}: the table must report the propagation role no longer, got {table}"
    );
}

/// The rule's other half: ONE table serves both default roles, so a call that moves the
/// default role from one option to the other reuses the table the gateway already holds
/// rather than minting a second one and leaving the first holding neither role.
///
/// The rows are the two directions that move can be made in.
#[tokio::test]
async fn moving_a_default_role_between_the_options_keeps_one_default_table() {
    let (router, _engine) = emulator();
    // (case, the role the gateway starts with, the role it is moved to, the id each names)
    let cases = [
        (
            "association to propagation",
            "Options.DefaultRouteTableAssociation",
            "Options.DefaultRouteTablePropagation",
            "associationDefaultRouteTableId",
            "propagationDefaultRouteTableId",
        ),
        (
            "propagation to association",
            "Options.DefaultRouteTablePropagation",
            "Options.DefaultRouteTableAssociation",
            "propagationDefaultRouteTableId",
            "associationDefaultRouteTableId",
        ),
    ];

    for (case, from_option, to_option, from_member, to_member) in cases {
        let transit_gateway_id = create_ok(
            case,
            &router,
            &[param(from_option, OPTION_ENABLE), param(to_option, OPTION_DISABLE)],
        )
        .await;
        let created = describe_ok(case, &router, &transit_gateway_id).await;
        let held = option(case, &created, from_member).unwrap_or_else(|| {
            panic!("case {case:?}: the created gateway must hold a default table: {created}")
        });

        // One call that disables the role the table holds and enables the other.
        let answered = modify_ok(
            case,
            &router,
            &request(
                &transit_gateway_id,
                &[
                    param(from_option, OPTION_DISABLE),
                    param(to_option, OPTION_ENABLE),
                ],
            ),
        )
        .await;
        assert_eq!(
            option(case, &answered, from_member),
            None,
            "case {case:?}: the disabled role must name no table, got {answered}"
        );
        assert_eq!(
            option(case, &answered, to_member).as_deref(),
            Some(held.as_str()),
            "case {case:?}: the enabled role must take the table the gateway already \
             holds, got {answered}"
        );
        // The gateway holds that one table and nothing else: a second one would have been
        // minted with neither role.
        let tables = describe_route_tables_of(case, &router, &transit_gateway_id).await;
        assert_eq!(
            tables,
            vec![held.clone()],
            "case {case:?}: the gateway must hold exactly its one default route table, \
             got {tables:?}"
        );
    }
}

/// The same rule read as an INVARIANT over a COUNT: a gateway has at most ONE default route
/// table, so disabling both default roles and enabling one again gives that table the role
/// back instead of minting another — however many times the pair of calls is repeated, the
/// gateway holds exactly one route table.
#[tokio::test]
async fn cycling_the_default_roles_keeps_the_one_default_table() {
    // A bounded number of cycles, well above the one a single recorded call makes: each cycle
    // that minted a second table would leave a record nothing names behind.
    const CYCLES: usize = 8;

    let (router, _engine) = emulator();
    let transit_gateway_id = create_ok("the gateway", &router, &baseline_params()).await;
    let created = describe_ok("the gateway", &router, &transit_gateway_id).await;
    let held = option("the gateway", &created, "associationDefaultRouteTableId")
        .unwrap_or_else(|| panic!("the created gateway must hold a default table: {created}"));

    for cycle in 1..=CYCLES {
        let case = format!("cycle {cycle}, both roles disabled");
        let answered = modify_ok(
            &case,
            &router,
            &request(
                &transit_gateway_id,
                &[
                    param("Options.DefaultRouteTableAssociation", OPTION_DISABLE),
                    param("Options.DefaultRouteTablePropagation", OPTION_DISABLE),
                ],
            ),
        )
        .await;
        for name in ["associationDefaultRouteTableId", "propagationDefaultRouteTableId"] {
            let named = option(&case, &answered, name);
            assert_eq!(
                named, None,
                "case {case:?}: a disabled role must name no table, got {named:?} in {answered}"
            );
        }

        let case = format!("cycle {cycle}, association enabled again");
        let answered = modify_ok(
            &case,
            &router,
            &request(
                &transit_gateway_id,
                &[param("Options.DefaultRouteTableAssociation", OPTION_ENABLE)],
            ),
        )
        .await;
        let named = option(&case, &answered, "associationDefaultRouteTableId");
        assert_eq!(
            named.as_deref(),
            Some(held.as_str()),
            "case {case:?}: the role must take back the gateway's own default table, \
             got {named:?} in {answered}"
        );
        let tables = describe_route_tables_of(&case, &router, &transit_gateway_id).await;
        assert_eq!(
            tables,
            vec![held.clone()],
            "case {case:?}: the gateway must still hold exactly one route table, got {tables:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// VPC Encryption Control
// ---------------------------------------------------------------------------
/// The rule: "Enable or disable encryption support for VPC Encryption Control"
/// (`EncryptionSupport`) asks for the END state, and the gateway reports the state it is IN
/// (`EncryptionStateValue`) — so an enable answers `enabling` and reaches `enabled` on the
/// clock, a disable answers `disabling` and reaches `disabled`, and asking for the state the
/// gateway has already reached changes nothing.
#[tokio::test]
async fn encryption_support_walks_the_published_states() {
    let (router, _engine) = emulator();
    let transit_gateway_id = create_ok("the gateway", &router, &baseline_params()).await;

    // Nothing in a create asks for VPC Encryption Control, so the gateway reports no
    // encryption state at all until a modification asks for one.
    let created = describe_ok("the gateway", &router, &transit_gateway_id).await;
    assert_eq!(
        encryption_state("the gateway", &created),
        None,
        "a created gateway must report no encryption state: {created}"
    );

    // (case, the value asked for, the state answered, the state the clock then owes)
    let cases = [
        ("enabled", OPTION_ENABLE, ENCRYPTION_STATE_ENABLING, ENCRYPTION_STATE_ENABLED),
        (
            "enabled again once enabled",
            OPTION_ENABLE,
            ENCRYPTION_STATE_ENABLED,
            ENCRYPTION_STATE_ENABLED,
        ),
        (
            "disabled",
            OPTION_DISABLE,
            ENCRYPTION_STATE_DISABLING,
            ENCRYPTION_STATE_DISABLED,
        ),
        (
            "disabled again once disabled",
            OPTION_DISABLE,
            ENCRYPTION_STATE_DISABLED,
            ENCRYPTION_STATE_DISABLED,
        ),
        (
            "enabled once more",
            OPTION_ENABLE,
            ENCRYPTION_STATE_ENABLING,
            ENCRYPTION_STATE_ENABLED,
        ),
    ];

    for (case, asked, answered_state, settled_state) in cases {
        let answered = modify_ok(
            case,
            &router,
            &request(
                &transit_gateway_id,
                &[param("Options.EncryptionSupport", asked)],
            ),
        )
        .await;
        let actual = encryption_state(case, &answered);
        assert_eq!(
            actual.as_deref(),
            Some(answered_state),
            "case {case:?}: expected the encryption state {answered_state:?}, got \
             {actual:?} in {answered}"
        );

        tokio::time::sleep(past_the_transition()).await;
        let read = describe_ok(case, &router, &transit_gateway_id).await;
        let settled = encryption_state(case, &read);
        assert_eq!(
            settled.as_deref(),
            Some(settled_state),
            "case {case:?}: the clock owed the encryption state {settled_state:?}, got \
             {settled:?} in {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// The attachments that block an ASN change
// ---------------------------------------------------------------------------

/// A Connect attachment of `transit_gateway_id`, as it is stored.
fn connect_attachment(transit_gateway_id: &str, state: &str) -> TransitGatewayAttachmentView {
    TransitGatewayAttachmentView::Connect(TransitGatewayConnect {
        transit_gateway_id: Some(transit_gateway_id.to_string()),
        state: Some(state.to_string()),
        ..Default::default()
    })
}

/// A Client VPN attachment of `transit_gateway_id`, as it is stored.
fn client_vpn_attachment(transit_gateway_id: &str, state: &str) -> TransitGatewayAttachmentView {
    TransitGatewayAttachmentView::ClientVpn(TransitGatewayClientVpnAttachment {
        transit_gateway_id: Some(transit_gateway_id.to_string()),
        client_vpn_endpoint_id: Some("cvpn-endpoint-0123456789abcdef0".to_string()),
        state: Some(state.to_string()),
        ..Default::default()
    })
}

/// A VPC attachment of `transit_gateway_id`, as it is stored.
fn vpc_attachment(transit_gateway_id: &str, state: &str) -> TransitGatewayAttachmentView {
    TransitGatewayAttachmentView::Vpc(TransitGatewayVpcAttachment {
        transit_gateway_id: Some(transit_gateway_id.to_string()),
        vpc_id: Some("vpc-0123456789abcdef0".to_string()),
        state: Some(state.to_string()),
        ..Default::default()
    })
}

/// A transit gateway peering attachment whose requester is `transit_gateway_id`.
fn peering_attachment(transit_gateway_id: &str, state: &str) -> TransitGatewayAttachmentView {
    TransitGatewayAttachmentView::Peering(TransitGatewayPeeringAttachment {
        requester_tgw_info: Some(PeeringTgwInfo {
            transit_gateway_id: Some(transit_gateway_id.to_string()),
            owner_id: Some(engine::emulator_account_id().to_string()),
            region: Some(REGION.to_string()),
            ..Default::default()
        }),
        state: Some(state.to_string()),
        ..Default::default()
    })
}

/// One row of the table below: the attachment the gateway holds, and whether an attachment
/// of that kind in that state blocks a modification of the gateway's Amazon-side ASN.
struct AsnCase {
    /// What this row is called in an assertion.
    case: &'static str,
    /// The attachment to seed on the created gateway, or `None` for a gateway holding none.
    seed: Option<fn(&str, &str) -> TransitGatewayAttachmentView>,
    /// The state the seeded attachment is in.
    state: &'static str,
    /// Whether that attachment blocks the ASN change.
    blocks: bool,
}

/// The rule: "The modify ASN operation is not allowed on a transit gateway if it has the
/// following attachments: … Connect … Client VPN. You must first delete all transit gateway
/// attachments configured prior to modifying the ASN on the transit gateway"
/// (`AmazonSideAsn`) — the request is refused for the state the gateway is in and writes
/// nothing, while an attachment of a kind that sentence does not list, and a modification
/// that names no ASN at all, are served.
///
/// The rows are one attachment of each kind the store holds, a rejected attachment of a
/// blocking kind, and a gateway with no attachment at all.
#[tokio::test]
async fn modifying_the_amazon_side_asn_is_refused_while_a_blocking_attachment_exists() {
    let cases = vec![
        AsnCase {
            case: "no attachment at all",
            seed: None,
            state: "",
            blocks: false,
        },
        AsnCase {
            case: "a Connect attachment",
            seed: Some(connect_attachment),
            state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            blocks: true,
        },
        AsnCase {
            case: "a Client VPN attachment",
            seed: Some(client_vpn_attachment),
            state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            blocks: true,
        },
        AsnCase {
            case: "a VPC attachment, a kind the sentence does not list",
            seed: Some(vpc_attachment),
            state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            blocks: false,
        },
        AsnCase {
            case: "a peering attachment, a kind the sentence does not list",
            seed: Some(peering_attachment),
            state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            blocks: false,
        },
        AsnCase {
            case: "a rejected Connect attachment, configured on nothing",
            seed: Some(connect_attachment),
            state: TRANSIT_GATEWAY_ATTACHMENT_STATE_REJECTED,
            blocks: false,
        },
    ];

    for AsnCase {
        case,
        seed,
        state,
        blocks,
    } in cases
    {
        let (router, engine) = emulator();
        let transit_gateway_id = create_ok(case, &router, &baseline_params()).await;
        if let Some(seed) = seed {
            engine
                .state::<Ec2State>()
                .lock()
                .insert_transit_gateway_attachment(seed(&transit_gateway_id, state))
                .unwrap_or_else(|| panic!("case {case:?}: the store must hold the attachment"));
        }

        let asked = *AMAZON_SIDE_ASN_16_BIT.end();
        let params = request(
            &transit_gateway_id,
            &[param("Options.AmazonSideAsn", &asked.to_string())],
        );
        let (status, body) = modify(&router, &params).await;
        if blocks {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for a blocked ASN change, got {status} \
                 with body {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code,
                ec2::state::INCORRECT_STATE,
                "case {case:?}: expected IncorrectState, got {code:?} in {body}"
            );
            let message = error_message(&body);
            assert!(
                message.contains(&transit_gateway_id),
                "case {case:?}: the refusal must name the gateway, got {message:?}"
            );
            // Nothing was written: the ASN is the one the gateway was created with.
            let read = describe_ok(case, &router, &transit_gateway_id).await;
            let described = reported(case, &read);
            assert_eq!(
                described,
                Reported::baseline(),
                "case {case:?}: a refused modification must write nothing, got \
                 {described:?} in {read}"
            );
            // The same gateway takes a modification that names no ASN at all.
            let answered = modify_ok(
                case,
                &router,
                &request(&transit_gateway_id, &[param("Description", "still fine")]),
            )
            .await;
            assert_eq!(
                member(case, &answered, "description").as_deref(),
                Some("still fine"),
                "case {case:?}: a modification naming no ASN must be served, got {answered}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for an ASN change nothing blocks, got \
                 {status} with body {body}"
            );
            let actual = option(case, &body, "amazonSideAsn");
            assert_eq!(
                actual.as_deref(),
                Some(asked.to_string().as_str()),
                "case {case:?}: expected the ASN {asked}, got {actual:?} in {body}"
            );
        }
    }
}
