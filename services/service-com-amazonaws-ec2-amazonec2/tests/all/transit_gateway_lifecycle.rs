// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateTransitGateway` and the `DescribeTransitGateways` it reads
//! back through, driven over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries
//! `Action=CreateTransitGateway`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rule under test is that the operation CREATES THE RESOURCE THE REQUEST DESCRIBES:
//! the transit gateway it stores carries the requested description, the requested options
//! with every member the request left out at the default that member's own documentation
//! publishes, and the tags of a `transit-gateway` tag specification; it is answered in the
//! non-terminal `pending` state the clock then owes `available`; it gets the default
//! transit gateway route table exactly when an option enables one; and every member of it
//! reads back through `DescribeTransitGateways`. Every table row is a distinct input along
//! what that generalises over — the description, each option's value, the ASN at both
//! published bounds of both published ranges, the CIDR block family and size, the tag count
//! — so the one case a recording happens to contain is one row among many rather than the
//! specification.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed};
use super::wire::{child, child_text, document, is_long_id, param, Param};

/// The Region every call here is signed for, which the gateway's ARN must name.
const REGION: &str = "us-east-1";

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it. The store is
/// reached only through the wire here, so the engine handle is not needed.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `CreateTransitGateway` call carrying `params`, as its HTTP status and XML body.
async fn create(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "CreateTransitGateway", params).await
}

/// One `DescribeTransitGateways` call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeTransitGateways", params).await
}

/// `CreateTransitGateway` requiring success, and the body it answered.
async fn create_ok(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    let (status, body) = create(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateTransitGateway, got {status} with body {body}"
    );
    body
}

/// `DescribeTransitGateways` requiring success, and the body it answered.
async fn describe_ok(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeTransitGateways, got {status} with \
         body {body}"
    );
    body
}

/// The ids of every transit gateway a response reports, in document order.
fn reported_ids(body: &str) -> Vec<String> {
    elements(body, "transitGatewayId")
}

/// A scalar member of the one transit gateway a response carries, or `None` when the
/// gateway does not carry it.
fn member(case: &str, body: &str, name: &str) -> Option<String> {
    let parsed = document(case, body);
    let gateway = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "transitGatewayId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no transit gateway in {body}"));
    child_text(gateway, name)
}

/// A member of the `options` of the one transit gateway a response carries.
fn option(case: &str, body: &str, name: &str) -> Option<String> {
    let parsed = document(case, body);
    let gateway = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "transitGatewayId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no transit gateway in {body}"));
    child(gateway, "options").and_then(|options| child_text(options, name))
}

/// The `<item>` texts of a named set inside the one gateway's `options`.
fn option_items(case: &str, body: &str, name: &str) -> Vec<String> {
    let parsed = document(case, body);
    let Some(gateway) = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "transitGatewayId").is_some())
    else {
        panic!("case {case:?}: no transit gateway in {body}");
    };
    let Some(set) = child(gateway, "options").and_then(|options| child(options, name)) else {
        return Vec::new();
    };
    set.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(|item| item.text().unwrap_or_default().to_string())
        .collect()
}

/// The tags of the one transit gateway a response carries, in the order it reported them.
fn gateway_tags(case: &str, body: &str) -> Vec<(String, String)> {
    let parsed = document(case, body);
    let Some(gateway) = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "transitGatewayId").is_some())
    else {
        panic!("case {case:?}: no transit gateway in {body}");
    };
    let Some(set) = child(gateway, "tagSet") else {
        return Vec::new();
    };
    set.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(|item| {
            (
                child_text(item, "key").unwrap_or_default(),
                child_text(item, "value").unwrap_or_default(),
            )
        })
        .collect()
}

/// The form fields for `tags` as a `transit-gateway` tag specification.
fn tag_params(tags: &[(&str, &str)]) -> Vec<Param> {
    let mut params = vec![param(
        "TagSpecification.1.ResourceType",
        ec2::state::RESOURCE_TYPE_TRANSIT_GATEWAY,
    )];
    for (position, (key, value)) in tags.iter().enumerate() {
        let index = position + 1;
        params.push(param(&format!("TagSpecification.1.Tag.{index}.Key"), key));
        params.push(param(
            &format!("TagSpecification.1.Tag.{index}.Value"),
            value,
        ));
    }
    params
}

/// What a transit gateway must report: the request's own values, and every member the
/// request left out at the default its documentation publishes.
#[derive(Debug, Clone)]
struct Expected {
    description: Option<String>,
    amazon_side_asn: String,
    auto_accept_shared_attachments: String,
    default_route_table_association: String,
    default_route_table_propagation: String,
    vpn_ecmp_support: String,
    dns_support: String,
    security_group_referencing_support: String,
    multicast_support: String,
    cidr_blocks: Vec<String>,
    tags: Vec<(String, String)>,
}

impl Expected {
    /// The gateway a request that names nothing but the action gets: every option at the
    /// default `TransitGatewayRequestOptions`'s own member docs publish, which this crate
    /// holds in one place ([`ec2::state::transit_gateway_options`]'s constants).
    fn defaults() -> Self {
        Self {
            description: None,
            amazon_side_asn: ec2::state::DEFAULT_AMAZON_SIDE_ASN.to_string(),
            auto_accept_shared_attachments: ec2::state::OPTION_DISABLE.to_string(),
            default_route_table_association: ec2::state::OPTION_ENABLE.to_string(),
            default_route_table_propagation: ec2::state::OPTION_ENABLE.to_string(),
            vpn_ecmp_support: ec2::state::OPTION_ENABLE.to_string(),
            dns_support: ec2::state::OPTION_ENABLE.to_string(),
            security_group_referencing_support: ec2::state::OPTION_DISABLE.to_string(),
            multicast_support: ec2::state::OPTION_DISABLE.to_string(),
            cidr_blocks: Vec::new(),
            tags: Vec::new(),
        }
    }

    fn with_description(mut self, description: &str) -> Self {
        self.description = Some(description.to_string());
        self
    }

    fn with_asn(mut self, asn: i64) -> Self {
        self.amazon_side_asn = asn.to_string();
        self
    }

    fn with_cidr_blocks(mut self, blocks: &[&str]) -> Self {
        self.cidr_blocks = blocks.iter().map(|block| (*block).to_string()).collect();
        self
    }

    fn with_tags(mut self, tags: &[(&str, &str)]) -> Self {
        self.tags = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        self
    }
}

/// Assert every member of `expected` on the one transit gateway `body` carries.
fn assert_gateway(case: &str, body: &str, expected: &Expected) {
    let actual_description = member(case, body, "description");
    assert_eq!(
        actual_description, expected.description,
        "case {case:?}: expected description {:?}, got {actual_description:?}",
        expected.description
    );
    let scalars = [
        ("amazonSideAsn", &expected.amazon_side_asn),
        (
            "autoAcceptSharedAttachments",
            &expected.auto_accept_shared_attachments,
        ),
        (
            "defaultRouteTableAssociation",
            &expected.default_route_table_association,
        ),
        (
            "defaultRouteTablePropagation",
            &expected.default_route_table_propagation,
        ),
        ("vpnEcmpSupport", &expected.vpn_ecmp_support),
        ("dnsSupport", &expected.dns_support),
        (
            "securityGroupReferencingSupport",
            &expected.security_group_referencing_support,
        ),
        ("multicastSupport", &expected.multicast_support),
    ];
    for (name, wanted) in scalars {
        let actual = option(case, body, name);
        assert_eq!(
            actual.as_deref(),
            Some(wanted.as_str()),
            "case {case:?}: expected option {name} {wanted:?}, got {actual:?}"
        );
    }
    let actual_blocks = option_items(case, body, "transitGatewayCidrBlocks");
    assert_eq!(
        actual_blocks, expected.cidr_blocks,
        "case {case:?}: expected CIDR blocks {:?}, got {actual_blocks:?}",
        expected.cidr_blocks
    );
    let actual_tags = gateway_tags(case, body);
    assert_eq!(
        actual_tags, expected.tags,
        "case {case:?}: expected tags {:?}, got {actual_tags:?}",
        expected.tags
    );
}

// ---------------------------------------------------------------------------
// What a create stores, and reading it back
// ---------------------------------------------------------------------------

/// The rule: `CreateTransitGateway` stores the gateway the request describes — its
/// description, its options with every omitted member at its published default, and its
/// tags — answers it with a server-minted id, ARN, account and creation time in `pending`,
/// and every member of it reads back through `DescribeTransitGateways`.
///
/// The rows differ in the description, each option's value, the ASN at both published
/// bounds of both published ranges, the CIDR block family and size, and the tag count.
#[tokio::test]
async fn create_transit_gateway_stores_the_request_and_round_trips_through_describe() {
    let router = emulator();
    // (case, the request's form fields, what the gateway must report)
    let cases: Vec<(&str, Vec<Param>, Expected)> = vec![
        ("nothing but the action", Vec::new(), Expected::defaults()),
        (
            "a description",
            vec![param("Description", "the hub gateway")],
            Expected::defaults().with_description("the hub gateway"),
        ),
        (
            "the 16-bit ASN range's floor",
            vec![param("Options.AmazonSideAsn", "64512")],
            Expected::defaults().with_asn(64_512),
        ),
        (
            "the 16-bit ASN range's ceiling",
            vec![param("Options.AmazonSideAsn", "65534")],
            Expected::defaults().with_asn(65_534),
        ),
        (
            "the 32-bit ASN range's floor",
            vec![param("Options.AmazonSideAsn", "4200000000")],
            Expected::defaults().with_asn(4_200_000_000),
        ),
        (
            "the 32-bit ASN range's ceiling",
            vec![param("Options.AmazonSideAsn", "4294967294")],
            Expected::defaults().with_asn(4_294_967_294),
        ),
        (
            "every option set the other way round",
            vec![
                param("Options.AutoAcceptSharedAttachments", "enable"),
                param("Options.VpnEcmpSupport", "disable"),
                param("Options.DnsSupport", "disable"),
                param("Options.SecurityGroupReferencingSupport", "enable"),
                param("Options.MulticastSupport", "enable"),
            ],
            Expected {
                auto_accept_shared_attachments: "enable".to_string(),
                vpn_ecmp_support: "disable".to_string(),
                dns_support: "disable".to_string(),
                security_group_referencing_support: "enable".to_string(),
                multicast_support: "enable".to_string(),
                ..Expected::defaults()
            },
        ),
        (
            "an IPv4 CIDR block at the published minimum size",
            vec![param("Options.TransitGatewayCidrBlocks.1", "10.7.0.0/24")],
            Expected::defaults().with_cidr_blocks(&["10.7.0.0/24"]),
        ),
        (
            "an IPv4 block larger than the minimum, with host bits set",
            vec![param("Options.TransitGatewayCidrBlocks.1", "10.8.1.9/16")],
            Expected::defaults().with_cidr_blocks(&["10.8.0.0/16"]),
        ),
        (
            "an IPv6 block at the published minimum size",
            vec![param(
                "Options.TransitGatewayCidrBlocks.1",
                "2001:db8:1234:1a00::/64",
            )],
            Expected::defaults().with_cidr_blocks(&["2001:db8:1234:1a00::/64"]),
        ),
        (
            "both families at once, each larger than its minimum",
            vec![
                param("Options.TransitGatewayCidrBlocks.1", "10.9.0.0/20"),
                param("Options.TransitGatewayCidrBlocks.2", "2001:db8:5::/56"),
            ],
            Expected::defaults().with_cidr_blocks(&["10.9.0.0/20", "2001:db8:5::/56"]),
        ),
        (
            "one tag",
            tag_params(&[("Name", "alpha")]),
            Expected::defaults().with_tags(&[("Name", "alpha")]),
        ),
        (
            "three tags, a description and an option",
            {
                let mut params = vec![
                    param("Description", "tagged and configured"),
                    param("Options.DnsSupport", "disable"),
                ];
                params.extend(tag_params(&[
                    ("Name", "beta"),
                    ("Tier", "web"),
                    ("Owner", "TeamA"),
                ]));
                params
            },
            Expected {
                dns_support: "disable".to_string(),
                ..Expected::defaults()
                    .with_description("tagged and configured")
                    .with_tags(&[("Name", "beta"), ("Tier", "web"), ("Owner", "TeamA")])
            },
        ),
    ];

    for (case, params, expected) in cases {
        let created = create_ok(case, &router, &params).await;
        assert_gateway(case, &created, &expected);

        // The server-minted members: the id in EC2's long-id form, the ARN naming the
        // Region the request was signed for and the emulator's account, the owning
        // account, a creation time, and the non-terminal state a create answers with.
        let id = member(case, &created, "transitGatewayId")
            .unwrap_or_else(|| panic!("case {case:?}: no transitGatewayId in {created}"));
        assert!(
            is_long_id("tgw", &id),
            "case {case:?}: expected a tgw- long id, got {id:?}"
        );
        let arn = member(case, &created, "transitGatewayArn");
        let wanted_arn = format!(
            "arn:aws:ec2:{REGION}:{}:transit-gateway/{id}",
            engine::emulator_account_id()
        );
        assert_eq!(
            arn.as_deref(),
            Some(wanted_arn.as_str()),
            "case {case:?}: expected ARN {wanted_arn:?}, got {arn:?}"
        );
        let owner = member(case, &created, "ownerId");
        assert_eq!(
            owner.as_deref(),
            Some(engine::emulator_account_id()),
            "case {case:?}: expected the emulator account as ownerId, got {owner:?}"
        );
        let state = member(case, &created, "state");
        assert_eq!(
            state.as_deref(),
            Some(ec2::state::TRANSIT_GATEWAY_STATE_PENDING),
            "case {case:?}: expected a create to answer the pending state, got {state:?}"
        );
        let creation_time = member(case, &created, "creationTime");
        assert!(
            creation_time
                .as_deref()
                .is_some_and(|time| time.contains('T')),
            "case {case:?}: expected an ISO-8601 creationTime, got {creation_time:?}"
        );

        // The round trip: every member reads back through the describe, and the describe
        // narrowed to this id reports exactly it.
        let read = describe_ok(case, &router, &[param("TransitGatewayIds.1", &id)]).await;
        assert_eq!(
            reported_ids(&read),
            vec![id.clone()],
            "case {case:?}: the describe narrowed to one id must report exactly it: {read}"
        );
        assert_gateway(case, &read, &expected);
        assert_eq!(
            member(case, &read, "transitGatewayArn"),
            arn,
            "case {case:?}: the ARN must round-trip through the describe: {read}"
        );
        let read_state = member(case, &read, "state").unwrap_or_default();
        assert!(
            read_state == ec2::state::TRANSIT_GATEWAY_STATE_PENDING
                || read_state == ec2::state::TRANSIT_GATEWAY_STATE_AVAILABLE,
            "case {case:?}: expected pending or available from the describe, \
             got {read_state:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The default route table a create owes the gateway
// ---------------------------------------------------------------------------

/// The rule: a create makes the default transit gateway route table exactly when at least
/// one of `DefaultRouteTableAssociation` and `DefaultRouteTablePropagation` is `enable`
/// ("Either `defaultRouteTableAssociation` or `defaultRouteTablePropagation` must be set to
/// `enable` for … Transit Gateway to create the default transit gateway route table" —
/// `TransitGatewayOptions$DefaultRouteTableAssociation`), it is ONE table used for both
/// purposes, and the id of each role is reported only for the option that is enabled.
///
/// The rows are every combination of the two options' two published values.
#[tokio::test]
async fn create_transit_gateway_creates_the_default_route_table_only_when_an_option_enables_it() {
    let router = emulator();
    // (case, association, propagation, an association id is reported, a propagation id is)
    let cases = [
        ("both enabled, the documented default", "enable", "enable", true, true),
        ("association only", "enable", "disable", true, false),
        ("propagation only", "disable", "enable", false, true),
        ("neither", "disable", "disable", false, false),
    ];

    for (case, association, propagation, reports_association, reports_propagation) in cases {
        let body = create_ok(
            case,
            &router,
            &[
                param("Options.DefaultRouteTableAssociation", association),
                param("Options.DefaultRouteTablePropagation", propagation),
            ],
        )
        .await;
        let association_id = option(case, &body, "associationDefaultRouteTableId");
        let propagation_id = option(case, &body, "propagationDefaultRouteTableId");
        assert_eq!(
            association_id.is_some(),
            reports_association,
            "case {case:?}: expected an associationDefaultRouteTableId: \
             {reports_association}, got {association_id:?}"
        );
        assert_eq!(
            propagation_id.is_some(),
            reports_propagation,
            "case {case:?}: expected a propagationDefaultRouteTableId: \
             {reports_propagation}, got {propagation_id:?}"
        );
        for id in [&association_id, &propagation_id].into_iter().flatten() {
            assert!(
                is_long_id("tgw-rtb", id),
                "case {case:?}: expected a tgw-rtb- long id, got {id:?}"
            );
        }
        if reports_association && reports_propagation {
            assert_eq!(
                association_id, propagation_id,
                "case {case:?}: one default route table serves both roles, \
                 got {association_id:?} and {propagation_id:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// What a create refuses
// ---------------------------------------------------------------------------

/// The rule: a value outside a published range, outside a published enum, or naming a
/// resource type this action does not tag is REFUSED with EC2's own code for it, and
/// nothing is stored.
///
/// The rows walk one step past each published bound of each published ASN range, each
/// family's published minimum CIDR block size, a value that is not a block at all, a value
/// that is not a member of an option's enum, and both tag-specification refusals.
#[tokio::test]
async fn create_transit_gateway_refuses_a_value_outside_a_published_range_or_enum() {
    let router = emulator();
    let before = reported_ids(&describe_ok("before", &router, &[]).await).len();

    // (case, the request's form fields, the EC2 error code)
    let cases: Vec<(&str, Vec<Param>, &str)> = vec![
        (
            "one below the 16-bit ASN range",
            vec![param("Options.AmazonSideAsn", "64511")],
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "one above the 16-bit ASN range",
            vec![param("Options.AmazonSideAsn", "65535")],
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "one below the 32-bit ASN range",
            vec![param("Options.AmazonSideAsn", "4199999999")],
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "one above the 32-bit ASN range",
            vec![param("Options.AmazonSideAsn", "4294967295")],
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "an IPv4 block one bit smaller than the minimum size",
            vec![param("Options.TransitGatewayCidrBlocks.1", "10.20.0.0/25")],
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "an IPv6 block one bit smaller than the minimum size",
            vec![param("Options.TransitGatewayCidrBlocks.1", "2001:db8::/65")],
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a CIDR block that is not one",
            vec![param("Options.TransitGatewayCidrBlocks.1", "ten dot zero")],
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "an IPv4 block with no prefix length",
            vec![param("Options.TransitGatewayCidrBlocks.1", "10.21.0.0")],
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a value no option enum has",
            vec![param("Options.DnsSupport", "enabled")],
            // Refused by the boundary validator, which reads the model's own enum: the
            // member is a `DnsSupportValue`, so a value outside it never reaches the
            // handler. Its code is EC2's `ValidationError`, not `InvalidParameterValue`.
            "ValidationError",
        ),
        (
            "a value no option enum has, on another member",
            vec![param("Options.MulticastSupport", "yes")],
            "ValidationError",
        ),
        (
            "a tag specification for another resource type",
            vec![
                param("TagSpecification.1.ResourceType", ec2::state::RESOURCE_TYPE_VPC),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "nope"),
            ],
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a tag specification with no resource type",
            vec![
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "nope"),
            ],
            ec2::state::MISSING_PARAMETER,
        ),
    ];

    for (case, params, expected) in cases {
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from CreateTransitGateway, got {status} with \
             body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected,
            "case {case:?}: expected the error code {expected:?}, got {code:?} in {body}"
        );
    }

    let after = reported_ids(&describe_ok("after", &router, &[]).await).len();
    assert_eq!(
        after, before,
        "a refused create must store nothing: expected {before} transit gateways, got {after}"
    );
}

/// The rule: a `DryRun` request that would have succeeded reports that outcome and writes
/// nothing — "If you have the required permissions, the error response is
/// DryRunOperation" (`DryRun`).
///
/// The rows differ in how much of the request the dry run carries, so the refusal cannot
/// depend on one request's shape.
#[tokio::test]
async fn a_dry_run_create_reports_the_outcome_and_stores_nothing() {
    let router = emulator();
    let before = reported_ids(&describe_ok("before", &router, &[]).await).len();

    let cases: Vec<(&str, Vec<Param>)> = vec![
        ("nothing but the flag", vec![param("DryRun", "true")]),
        (
            "a description and options",
            vec![
                param("DryRun", "true"),
                param("Description", "never stored"),
                param("Options.AmazonSideAsn", "64513"),
            ],
        ),
        ("a tag specification", {
            let mut params = vec![param("DryRun", "true")];
            params.extend(tag_params(&[("Name", "never stored")]));
            params
        }),
    ];
    for (case, params) in cases {
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412 from a dry run, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, ec2::state::DRY_RUN_OPERATION,
            "case {case:?}: expected the DryRunOperation code, got {code:?} in {body}"
        );
    }

    let after = reported_ids(&describe_ok("after", &router, &[]).await).len();
    assert_eq!(
        after, before,
        "a dry run must store nothing: expected {before} transit gateways, got {after}"
    );
}

// ---------------------------------------------------------------------------
// The clock-owed transition
// ---------------------------------------------------------------------------

/// The rule: a create answers the gateway in the non-terminal `pending` state, and the
/// clock — not a later call — owes it `available`; a read before the move is due must not
/// bring it forward.
///
/// The rows differ in the options the gateway was created with, so the transition is a
/// property of the resource rather than of one request's shape.
#[tokio::test]
async fn a_created_transit_gateway_becomes_available_on_the_clock() {
    let router = emulator();
    let cases: Vec<(&str, Vec<Param>)> = vec![
        ("the documented defaults", Vec::new()),
        (
            "no default route table",
            vec![
                param("Options.DefaultRouteTableAssociation", "disable"),
                param("Options.DefaultRouteTablePropagation", "disable"),
            ],
        ),
    ];

    let mut created: Vec<(&str, String)> = Vec::new();
    for (case, params) in cases {
        let body = create_ok(case, &router, &params).await;
        let id = member(case, &body, "transitGatewayId")
            .unwrap_or_else(|| panic!("case {case:?}: no transitGatewayId in {body}"));
        let read = describe_ok(case, &router, &[param("TransitGatewayIds.1", &id)]).await;
        let state = member(case, &read, "state").unwrap_or_default();
        assert_eq!(
            state,
            ec2::state::TRANSIT_GATEWAY_STATE_PENDING,
            "case {case:?}: a read before the transition is due must not bring it forward, \
             got {state:?}"
        );
        created.push((case, id));
    }

    tokio::time::sleep(Duration::from_millis(900)).await;

    for (case, id) in created {
        let read = describe_ok(case, &router, &[param("TransitGatewayIds.1", &id)]).await;
        let state = member(case, &read, "state").unwrap_or_default();
        assert_eq!(
            state,
            ec2::state::TRANSIT_GATEWAY_STATE_AVAILABLE,
            "case {case:?}: expected available once the transition fell due, got {state:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// What the describe selects
// ---------------------------------------------------------------------------

/// One row of the filter table: the case, the filters the request carries (each a name and
/// its values), and the gateway ids they must select.
type FilterCase<'a> = (&'a str, Vec<(&'a str, Vec<&'a str>)>, Vec<String>);

/// The rule: `DescribeTransitGateways` reports every gateway the store holds, narrowed by
/// `TransitGatewayIds` and by every filter its own documentation lists, each filter
/// matching the attribute the documentation names; an id that names no gateway is an error
/// and an undocumented filter name is refused rather than read as "no results".
///
/// The filter rows cover each documented filter name — on an option, on an id, on the
/// owning account, on the state, and on a tag by key and by key/value — so the set is the
/// documented one rather than the one a recording happened to use.
#[tokio::test]
async fn describe_transit_gateways_selects_by_id_and_by_every_documented_filter() {
    let router = emulator();

    // Three gateways differing in every attribute a filter can name.
    let mut first_params = vec![
        param("Description", "first"),
        param("Options.AmazonSideAsn", "64520"),
        param("Options.DnsSupport", "disable"),
    ];
    first_params.extend(tag_params(&[("Tier", "web")]));
    let first = create_ok("first", &router, &first_params).await;
    let first_id = member("first", &first, "transitGatewayId").expect("an id");

    let mut second_params = vec![
        param("Description", "second"),
        param("Options.AmazonSideAsn", "64521"),
        param("Options.AutoAcceptSharedAttachments", "enable"),
    ];
    second_params.extend(tag_params(&[("Tier", "data")]));
    let second = create_ok("second", &router, &second_params).await;
    let second_id = member("second", &second, "transitGatewayId").expect("an id");

    let third = create_ok(
        "third",
        &router,
        &[
            param("Description", "third"),
            param("Options.DefaultRouteTablePropagation", "disable"),
            param("Options.VpnEcmpSupport", "disable"),
        ],
    )
    .await;
    let third_id = member("third", &third, "transitGatewayId").expect("an id");
    let third_association =
        option("third", &third, "associationDefaultRouteTableId").expect("an association table");

    // Every gateway is `available` once the clock has passed their due instant, so the
    // `state` rows below are about a settled store rather than a race.
    tokio::time::sleep(Duration::from_millis(900)).await;

    let all = vec![first_id.clone(), second_id.clone(), third_id.clone()];
    // (case, the filter, the ids it must select)
    let cases: Vec<FilterCase> = vec![
        ("no filter at all", Vec::new(), all.clone()),
        (
            "transit-gateway-id",
            vec![("transit-gateway-id", vec![second_id.as_str()])],
            vec![second_id.clone()],
        ),
        (
            "transit-gateway-id with two values, joined with OR",
            vec![(
                "transit-gateway-id",
                vec![first_id.as_str(), third_id.as_str()],
            )],
            vec![first_id.clone(), third_id.clone()],
        ),
        (
            "options.amazon-side-asn",
            vec![("options.amazon-side-asn", vec!["64521"])],
            vec![second_id.clone()],
        ),
        (
            "options.dns-support",
            vec![("options.dns-support", vec!["disable"])],
            vec![first_id.clone()],
        ),
        (
            "options.auto-accept-shared-attachments",
            vec![("options.auto-accept-shared-attachments", vec!["enable"])],
            vec![second_id.clone()],
        ),
        (
            "options.vpn-ecmp-support",
            vec![("options.vpn-ecmp-support", vec!["disable"])],
            vec![third_id.clone()],
        ),
        (
            "options.default-route-table-propagation",
            vec![("options.default-route-table-propagation", vec!["disable"])],
            vec![third_id.clone()],
        ),
        (
            "options.default-route-table-association",
            vec![("options.default-route-table-association", vec!["enable"])],
            all.clone(),
        ),
        (
            "options.association-default-route-table-id",
            vec![(
                "options.association-default-route-table-id",
                vec![third_association.as_str()],
            )],
            vec![third_id.clone()],
        ),
        (
            "options.propagation-default-route-table-id, absent on a gateway that disabled it",
            vec![(
                "options.propagation-default-route-table-id",
                vec![third_association.as_str()],
            )],
            Vec::new(),
        ),
        (
            "owner-id",
            vec![("owner-id", vec![engine::emulator_account_id()])],
            all.clone(),
        ),
        (
            "state, on the state every gateway has settled into",
            vec![("state", vec![ec2::state::TRANSIT_GATEWAY_STATE_AVAILABLE])],
            all.clone(),
        ),
        (
            "state, on the one no gateway is in any more",
            vec![("state", vec![ec2::state::TRANSIT_GATEWAY_STATE_PENDING])],
            Vec::new(),
        ),
        (
            "tag:Tier",
            vec![("tag:Tier", vec!["data"])],
            vec![second_id.clone()],
        ),
        (
            "tag-key",
            vec![("tag-key", vec!["Tier"])],
            vec![first_id.clone(), second_id.clone()],
        ),
        (
            "two filters, joined with AND",
            vec![
                ("tag-key", vec!["Tier"]),
                ("options.dns-support", vec!["disable"]),
            ],
            vec![first_id.clone()],
        ),
        (
            "a wildcard filter value",
            vec![("tag:Tier", vec!["*at*"])],
            vec![second_id.clone()],
        ),
    ];

    for (case, entries, wanted) in cases {
        let entries: Vec<(&str, &[&str])> = entries
            .iter()
            .map(|(name, values)| (*name, values.as_slice()))
            .collect();
        let body = describe_ok(case, &router, &filters(&entries)).await;
        let mut reported = reported_ids(&body);
        reported.sort();
        let mut wanted = wanted;
        wanted.sort();
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected the ids {wanted:?}, got {reported:?} in {body}"
        );
    }

    // An id that names no gateway of the account is an error, not an absent row.
    let (status, body) = describe(
        &router,
        &[param("TransitGatewayIds.1", "tgw-0fedcba9876543210")],
    )
    .await;
    assert_eq!(
        status, 400,
        "an unknown transit gateway id must be refused, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code,
        ec2::state::INVALID_TRANSIT_GATEWAY_ID_NOT_FOUND,
        "expected the NotFound code for an unknown id, got {code:?} in {body}"
    );

    // An undocumented filter name is refused rather than read as "no results".
    for name in ["transit-gateway-arn", "options.multicast-support", "tag:"] {
        let (status, body) = describe(&router, &filters(&[(name, &["anything"])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: an undocumented filter must be refused, got {status} with \
             body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code,
            ec2::state::INVALID_PARAMETER_VALUE,
            "case {name:?}: expected InvalidParameterValue, got {code:?} in {body}"
        );
    }

    // A pagination token the service did not mint names no position in the report.
    let (status, body) = describe(&router, &[param("NextToken", "not-a-token")]).await;
    assert_eq!(
        status, 400,
        "a bogus pagination token must be refused, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code,
        ec2::state::INVALID_PAGINATION_TOKEN,
        "expected InvalidPaginationToken, got {code:?} in {body}"
    );
}

/// The rule: the report is paged in `TransitGatewayId` order, a `NextToken` is returned
/// exactly when a gateway follows the page, and following it reports the rest once each.
///
/// The rows differ in the page size, including the smallest the model's `MaxResults` range
/// publishes and one larger than the whole report.
#[tokio::test]
async fn describe_transit_gateways_pages_in_id_order() {
    let router = emulator();
    let mut created: Vec<String> = Vec::new();
    for index in 0..7 {
        let case = format!("gateway {index}");
        let body = create_ok(&case, &router, &[param("Description", &case)]).await;
        created.push(member(&case, &body, "transitGatewayId").expect("an id"));
    }
    created.sort();

    for page_size in [5usize, 6, 7, 1000] {
        let case = format!("a page size of {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut params = vec![param("MaxResults", &page_size.to_string())];
            if let Some(next) = token.as_deref() {
                params.push(param("NextToken", next));
            }
            let body = describe_ok(&case, &router, &params).await;
            let page = reported_ids(&body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} gateways, got {}",
                page.len()
            );
            seen.extend(page);
            token = element(&body, "nextToken");
            if token.is_none() {
                break;
            }
            assert!(
                seen.len() < created.len() + 1,
                "case {case:?}: a token must advance, but the report has already \
                 reported {} of {} gateways",
                seen.len(),
                created.len()
            );
        }
        assert_eq!(
            seen, created,
            "case {case:?}: expected every gateway once in id order, got {seen:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The ceilings on the filtering work one describe will do
// ---------------------------------------------------------------------------

/// The rule: matching cost is the PRODUCT of how many filters a request carries, how many
/// values each carries, how long a value is, how many ids it names and how many gateways
/// the store holds, and the model bounds none of them (`FilterList` and `Filter$Values`
/// carry no `@length`, `TransitGatewayIdStringList` no length either). So the handler
/// applies this crate's own ceilings — the same ones `DescribeSecurityGroups` and
/// `DescribeInstances` enforce — and a request AT each ceiling is answered.
///
/// Each row is a distinct factor of that product, named by the constant it comes from
/// rather than by a literal.
#[tokio::test]
async fn a_describe_at_each_filtering_ceiling_is_answered() {
    let router = emulator();
    let mut stored: Vec<String> = Vec::new();
    for index in 0..3 {
        let case = format!("gateway {index}");
        let body = create_ok(&case, &router, &[param("Description", &case)]).await;
        stored.push(member(&case, &body, "transitGatewayId").expect("an id"));
    }
    stored.sort();
    let known = stored[0].clone();

    // `MAX_FILTERS` filters, each selecting everything, so the AND of them all is the
    // whole store: the ceiling itself is accepted and nothing is lost.
    let at_filter_ceiling: Vec<(&str, &[&str])> = (0..ec2::state::MAX_FILTERS)
        .map(|_| ("transit-gateway-id", &["*"][..]))
        .collect();
    let body = describe_ok(
        "MAX_FILTERS filters",
        &router,
        &filters(&at_filter_ceiling),
    )
    .await;
    let mut reported = reported_ids(&body);
    reported.sort();
    assert_eq!(
        reported, stored,
        "case \"MAX_FILTERS filters\": expected every stored gateway, got {reported:?}"
    );

    // One filter carrying `MAX_FILTER_VALUES` values, a value of exactly
    // `MAX_FILTER_VALUE_BYTES` bytes, and `MAX_RESOURCE_IDS` ids: one row per remaining
    // factor of the product.
    let many_values: Vec<String> = (0..ec2::state::MAX_FILTER_VALUES)
        .map(|n| format!("tgw-{n:017x}"))
        .collect();
    let many_values: Vec<&str> = many_values.iter().map(String::as_str).collect();
    let long_value = "a".repeat(ec2::state::MAX_FILTER_VALUE_BYTES);
    let many_ids = vec![known.as_str(); ec2::state::MAX_RESOURCE_IDS];
    let cases: Vec<(&str, Vec<Param>)> = vec![
        (
            "MAX_FILTER_VALUES values in one filter",
            filters(&[("transit-gateway-id", &many_values)]),
        ),
        (
            "a value of MAX_FILTER_VALUE_BYTES bytes",
            filters(&[("transit-gateway-id", &[long_value.as_str()])]),
        ),
        (
            "MAX_RESOURCE_IDS ids, all naming one gateway",
            indexed("TransitGatewayIds", &many_ids),
        ),
    ];
    for (case, params) in cases {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 at the ceiling, got {status} with body {body}"
        );
    }
}

/// The rule's other side: one step past any ceiling is refused with
/// `InvalidParameterValue` and reports no gateway — the request asked for more work than
/// one call will do, and is told so rather than served a partial answer.
#[tokio::test]
async fn a_describe_one_step_past_a_filtering_ceiling_is_refused() {
    let router = emulator();
    let created = create_ok("one gateway", &router, &[]).await;
    let known = member("one gateway", &created, "transitGatewayId").expect("an id");

    let past_filter_ceiling: Vec<(&str, &[&str])> = (0..=ec2::state::MAX_FILTERS)
        .map(|_| ("transit-gateway-id", &["*"][..]))
        .collect();
    let past_value_ceiling: Vec<String> = (0..=ec2::state::MAX_FILTER_VALUES)
        .map(|n| format!("tgw-{n:017x}"))
        .collect();
    let past_value_ceiling: Vec<&str> = past_value_ceiling.iter().map(String::as_str).collect();
    let past_byte_ceiling = "a".repeat(ec2::state::MAX_FILTER_VALUE_BYTES + 1);
    let past_id_ceiling = vec![known.as_str(); ec2::state::MAX_RESOURCE_IDS + 1];

    let cases: Vec<(&str, Vec<Param>)> = vec![
        (
            "one filter past MAX_FILTERS",
            filters(&past_filter_ceiling),
        ),
        (
            "one value past MAX_FILTER_VALUES",
            filters(&[("transit-gateway-id", &past_value_ceiling)]),
        ),
        (
            "one byte past MAX_FILTER_VALUE_BYTES",
            filters(&[("transit-gateway-id", &[past_byte_ceiling.as_str()])]),
        ),
        (
            "one id past MAX_RESOURCE_IDS",
            indexed("TransitGatewayIds", &past_id_ceiling),
        ),
    ];
    for (case, params) in cases {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code,
            ec2::state::INVALID_PARAMETER_VALUE,
            "case {case:?}: expected InvalidParameterValue, got {code:?} in {body}"
        );
        assert!(
            reported_ids(&body).is_empty(),
            "case {case:?}: a refused request reports no gateway, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The ceiling on how many gateways the store holds
// ---------------------------------------------------------------------------

/// The rule: the store is finite, so a create past [`ec2::state::MAX_TRANSIT_GATEWAYS`]
/// is refused with EC2's `TransitGatewayLimitExceeded` instead of growing memory without
/// bound — and the refusal is the count ceiling, not the shape of the request.
///
/// The store is filled through the one writer every create goes through, so the test
/// exercises the handler's check rather than a copy of it; the rows differ in the request
/// that is then refused.
#[tokio::test]
async fn a_create_past_the_store_ceiling_is_refused() {
    let (router, engine) = super::ec2_query::emulator();

    // One short of the ceiling: a create is still served, and it is the one that fills it.
    {
        let store = engine.state::<ec2::state::ComAmazonawsEc2Amazonec2State>();
        let mut guard = store.lock();
        for index in 0..(ec2::state::MAX_TRANSIT_GATEWAYS - 1) {
            guard.insert_transit_gateway(ec2::state::TransitGatewaySpec {
                description: Some(format!("seeded {index}")),
                region: REGION.to_string(),
                created_at: std::time::SystemTime::now(),
                options: ec2::state::transit_gateway_options::<()>(None)
                    .expect("the documented defaults resolve"),
                tags: Vec::new(),
            });
        }
        assert!(
            guard.has_transit_gateway_capacity(),
            "one short of the ceiling the store must still have capacity, got {} gateways",
            guard.transit_gateways.len()
        );
    }
    let filling = create_ok("the create that fills the ceiling", &router, &[]).await;
    assert!(
        member("the create that fills the ceiling", &filling, "transitGatewayId").is_some(),
        "the create that fills the ceiling must still be served, got {filling}"
    );

    // At the ceiling, every create is refused, whatever it asks for.
    let cases: Vec<(&str, Vec<Param>)> = vec![
        ("nothing but the action", Vec::new()),
        ("a description", vec![param("Description", "one too many")]),
        (
            "an option",
            vec![param("Options.AmazonSideAsn", "64515")],
        ),
        ("a tag specification", tag_params(&[("Name", "rejected")])),
    ];
    for (case, params) in cases {
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 at the store ceiling, got {status} with \
             body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code,
            ec2::state::TRANSIT_GATEWAY_LIMIT_EXCEEDED,
            "case {case:?}: expected TransitGatewayLimitExceeded, got {code:?} in {body}"
        );
    }

    // The refusals stored nothing: the store is still exactly at the ceiling.
    let store = engine.state::<ec2::state::ComAmazonawsEc2Amazonec2State>();
    let held = store.lock().transit_gateways.len();
    assert_eq!(
        held,
        ec2::state::MAX_TRANSIT_GATEWAYS,
        "a refused create must store nothing: expected {} gateways, got {held}",
        ec2::state::MAX_TRANSIT_GATEWAYS
    );
}
