// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateTransitGatewayVpcAttachment` and the
//! `DescribeTransitGatewayVpcAttachments` it reads back through, driven over the wire the
//! service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose
//! body carries `Action=CreateTransitGatewayVpcAttachment`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that a VPC attachment is COMPUTED FROM THE REQUEST AND THE STORE,
//! never echoed or guessed: its `TransitGatewayAttachmentId` comes from the store's id mint,
//! its `VpcOwnerId` from the stored VPC, its `SubnetIds` from the subnets the request names
//! (each resolved against that VPC, de-duplicated, in the order named), each of its four
//! options from the request where it carries one and otherwise from the transit gateway's own
//! option or the member's documented default, and it is stored in the non-terminal `pending`
//! state with the clock owing it `available` — all of it readable back through
//! `DescribeTransitGatewayVpcAttachments`.
//!
//! Every table row is a distinct input along what that rule generalises over: which transit
//! gateway (and so which inherited options), which VPC and so which subnets, how many subnets
//! the request names (one, two, three), the order it names them in, whether it names one
//! twice, whether it carries an options block, which value each option member holds, and how
//! many tags it applies.
//!
//! Every parent of the attachment has a refusal row carrying EC2's own error code and
//! sentence: a value that is not a subnet id at all, a well-formed subnet id naming no subnet,
//! a subnet of another VPC, two subnets of one Availability Zone, a transit gateway the store
//! holds none of, a second attachment of one VPC to one gateway, and a tag specification
//! naming another resource type. Nothing is stored by any of them.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ec2::ops::types::TransitGatewayVpcAttachment;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DUPLICATE_SUBNETS_IN_SAME_ZONE,
    INVALID_PARAMETER_VALUE, INVALID_SUBNET_ID_MALFORMED, INVALID_SUBNET_ID_NOT_FOUND,
    INVALID_TRANSIT_GATEWAY_ID_NOT_FOUND, OPTION_DISABLE, OPTION_ENABLE,
    TRANSIT_GATEWAY_ATTACHMENT_ID_PREFIX, TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING, TRANSIT_GATEWAY_ATTACHMENT_STATE_REJECTED,
    TRANSITION_DELAY, TransitGatewayAttachmentView,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, call, element, error_code, error_message, indexed};
use super::wire::{Param, child, child_text, document, is_long_id, param, set_items};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// The ec2Query `Action` under test.
const ACTION: &str = "CreateTransitGatewayVpcAttachment";

/// The code the request-boundary validator refuses a member the model requires and the request
/// leaves out with, before any handler of this service runs.
const BOUNDARY_MISSING_MEMBER: &str = "MalformedInput";

/// The code that same validator refuses a value outside a modelled enum with.
const BOUNDARY_ENUM_VIOLATION: &str = "ValidationError";

/// A well-formed EC2 id suffix — 17 lowercase hex digits — that nothing in this store mints,
/// so an id built from it is well formed and names no resource.
const UNKNOWN_SUFFIX: &str = "00000000000000000";

/// Long enough past the two spans a new attachment waits — its transit gateway's own move to
/// `available`, then its own — that both are certainly due.
fn past_the_attachment_transition() -> Duration {
    TRANSITION_DELAY * 2 + Duration::from_millis(250)
}

/// One successful call of `action`, as its body.
async fn ok_body(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// The transit gateway `CreateTransitGateway` makes with `options`.
async fn create_transit_gateway(router: &MultiServiceRouter, options: &[Param]) -> String {
    let body = ok_body("the gateway", router, "CreateTransitGateway", options).await;
    element(&body, "transitGatewayId")
        .unwrap_or_else(|| panic!("CreateTransitGateway answered no transitGatewayId: {body}"))
}

/// The VPC `CreateVpc` makes for `cidr_block`.
async fn create_vpc(router: &MultiServiceRouter, cidr_block: &str) -> String {
    let body = ok_body(
        "the network",
        router,
        "CreateVpc",
        &[param("CidrBlock", cidr_block)],
    )
    .await;
    element(&body, "vpcId").unwrap_or_else(|| panic!("CreateVpc answered no vpcId: {body}"))
}

/// The subnet `CreateSubnet` makes in `vpc_id`, in `zone`.
async fn create_subnet(
    router: &MultiServiceRouter,
    vpc_id: &str,
    cidr_block: &str,
    zone: &str,
) -> String {
    let body = ok_body(
        "the network",
        router,
        "CreateSubnet",
        &[
            param("VpcId", vpc_id),
            param("CidrBlock", cidr_block),
            param("AvailabilityZone", zone),
        ],
    )
    .await;
    element(&body, "subnetId").unwrap_or_else(|| panic!("CreateSubnet answered no subnetId: {body}"))
}

/// One VPC of the fixture: its id and the subnets created in it, one per Availability Zone of
/// [`ZONES`], in the order they were created.
#[derive(Debug, Clone)]
struct Network {
    vpc_id: String,
    subnets: Vec<String>,
}

impl Network {
    /// The subnet of this network in the zone at `index`.
    fn subnet(&self, index: usize) -> &str {
        &self.subnets[index]
    }
}

/// The Availability Zones of [`REGION`] the fixture puts its subnets in — one subnet per zone,
/// which is what an attachment admits.
const ZONES: [&str; 3] = ["us-east-1a", "us-east-1b", "us-east-1c"];

/// The emulator, two transit gateways created over the wire with DIFFERENT options, and two
/// VPCs with their subnets.
struct Fixture {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    /// A gateway created with no options at all, so each of its options is its own default.
    default_gateway: String,
    /// A gateway whose `DnsSupport` is `disable` and whose `SecurityGroupReferencingSupport`
    /// is `enable` — the opposite of both defaults, so an attachment that inherits them cannot
    /// be reporting a default by accident.
    reversed_gateway: String,
    /// The VPC with three subnets, one per zone of [`ZONES`].
    primary: Network,
    /// A second VPC with one subnet, in the first zone.
    secondary: Network,
}

impl Fixture {
    async fn new() -> Self {
        let (router, engine) = super::ec2_query::emulator();
        let default_gateway = create_transit_gateway(&router, &[]).await;
        let reversed_gateway = create_transit_gateway(
            &router,
            &[
                param("Options.DnsSupport", OPTION_DISABLE),
                param("Options.SecurityGroupReferencingSupport", OPTION_ENABLE),
            ],
        )
        .await;

        let primary_vpc = create_vpc(&router, "10.90.0.0/16").await;
        let mut primary_subnets = Vec::new();
        for (index, zone) in ZONES.into_iter().enumerate() {
            primary_subnets
                .push(create_subnet(&router, &primary_vpc, &format!("10.90.{index}.0/24"), zone).await);
        }
        let secondary_vpc = create_vpc(&router, "10.91.0.0/16").await;
        let secondary_subnets =
            vec![create_subnet(&router, &secondary_vpc, "10.91.0.0/24", ZONES[0]).await];

        Self {
            router,
            engine,
            default_gateway,
            reversed_gateway,
            primary: Network {
                vpc_id: primary_vpc,
                subnets: primary_subnets,
            },
            secondary: Network {
                vpc_id: secondary_vpc,
                subnets: secondary_subnets,
            },
        }
    }

    /// How many VPC attachments the store holds — what a refusal must leave unchanged.
    fn stored_attachments(&self) -> usize {
        self.engine
            .state::<Ec2State>()
            .lock()
            .transit_gateway_attachments
            .values()
            .filter(|record| matches!(record.view, TransitGatewayAttachmentView::Vpc(_)))
            .count()
    }

    /// Seed one VPC attachment of `vpc_id` to `transit_gateway_id` in `attachment_state`
    /// straight through the store's own writer, for the rows that need an attachment this
    /// create did not make.
    fn seed_attachment(
        &self,
        transit_gateway_id: &str,
        vpc_id: &str,
        attachment_state: &str,
    ) -> String {
        self.engine
            .state::<Ec2State>()
            .lock()
            .insert_transit_gateway_attachment(TransitGatewayAttachmentView::Vpc(
                TransitGatewayVpcAttachment {
                    transit_gateway_id: Some(transit_gateway_id.to_string()),
                    vpc_id: Some(vpc_id.to_string()),
                    vpc_owner_id: Some(engine::emulator_account_id().to_string()),
                    state: Some(attachment_state.to_string()),
                    ..Default::default()
                },
            ))
            .expect("the attachment store has room for one seeded attachment")
    }
}

/// One `CreateTransitGatewayVpcAttachment` call carrying `params`.
async fn create(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    call(router, REGION, ACTION, params).await
}

/// One reported `TransitGatewayVpcAttachment`, read out of the XML by the document rather than
/// by string surgery — `state` appears on the attachment and the option members appear inside
/// its `options`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    attachment_id: String,
    transit_gateway_id: String,
    vpc_id: String,
    vpc_owner_id: String,
    state: String,
    subnet_ids: Vec<String>,
    creation_time: String,
    options: Vec<(String, String)>,
    tags: Vec<(String, String)>,
}

/// The text of every `item` of the `name` set of `node`, in document order.
fn item_texts(node: roxmltree::Node<'_, '_>, name: &str) -> Vec<String> {
    set_items(node, name)
        .into_iter()
        .map(|entry| entry.text().unwrap_or_default().to_string())
        .collect()
}

/// Every VPC attachment a response carries, in document order: the elements that have both a
/// `transitGatewayAttachmentId` and a `vpcOwnerId` child, whether the response is a create's
/// single `transitGatewayVpcAttachment` or a describe's `transitGatewayVpcAttachments` set.
fn reported_all(case: &str, body: &str) -> Vec<Reported> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| {
            node.is_element()
                && child(*node, "transitGatewayAttachmentId").is_some()
                && child(*node, "vpcOwnerId").is_some()
        })
        .map(|item| Reported {
            attachment_id: child_text(item, "transitGatewayAttachmentId").unwrap_or_default(),
            transit_gateway_id: child_text(item, "transitGatewayId").unwrap_or_default(),
            vpc_id: child_text(item, "vpcId").unwrap_or_default(),
            vpc_owner_id: child_text(item, "vpcOwnerId").unwrap_or_default(),
            state: child_text(item, "state").unwrap_or_default(),
            subnet_ids: item_texts(item, "subnetIds"),
            creation_time: child_text(item, "creationTime").unwrap_or_default(),
            options: child(item, "options")
                .map(|options| {
                    options
                        .children()
                        .filter(roxmltree::Node::is_element)
                        .map(|member| {
                            (
                                member.tag_name().name().to_string(),
                                member.text().unwrap_or_default().to_string(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
            tags: set_items(item, "tagSet")
                .into_iter()
                .map(|tag| {
                    (
                        child_text(tag, "key").unwrap_or_default(),
                        child_text(tag, "value").unwrap_or_default(),
                    )
                })
                .collect(),
        })
        .collect()
}

/// The one VPC attachment a response carries, or a panic naming the case.
fn reported(case: &str, body: &str) -> Reported {
    let mut all = reported_all(case, body);
    assert_eq!(
        all.len(),
        1,
        "case {case:?}: expected exactly one VPC attachment, got {all:?} in {body}"
    );
    all.remove(0)
}

/// The one VPC attachment `DescribeTransitGatewayVpcAttachments` reports for `attachment_id`.
async fn described(case: &str, router: &MultiServiceRouter, attachment_id: &str) -> Reported {
    let body = ok_body(
        case,
        router,
        "DescribeTransitGatewayVpcAttachments",
        &indexed("TransitGatewayAttachmentIds", &[attachment_id]),
    )
    .await;
    reported(case, &body)
}

/// The form fields for one tag specification of the attachment this action creates.
///
/// `CreateTransitGatewayVpcAttachmentRequest$TagSpecifications` carries no `xmlName`, so the
/// wire field is the member's own name
/// (`smithy_query(query="shape",shape="CreateTransitGatewayVpcAttachmentRequest")`).
fn tag_params(tags: &[(&str, &str)]) -> Vec<Param> {
    let mut params = vec![param(
        "TagSpecifications.1.ResourceType",
        ec2::state::RESOURCE_TYPE_TRANSIT_GATEWAY_ATTACHMENT,
    )];
    for (index, (key, value)) in tags.iter().enumerate() {
        params.push(param(
            &format!("TagSpecifications.1.Tag.{}.Key", index + 1),
            key,
        ));
        params.push(param(
            &format!("TagSpecifications.1.Tag.{}.Value", index + 1),
            value,
        ));
    }
    params
}

/// The four option members of a request, each as the value it asks for — `None` leaves the
/// member out, so the attachment takes what it inherits.
#[derive(Debug, Clone, Copy, Default)]
struct RequestedOptions {
    dns_support: Option<&'static str>,
    security_group_referencing_support: Option<&'static str>,
    ipv6_support: Option<&'static str>,
    appliance_mode_support: Option<&'static str>,
}

impl RequestedOptions {
    /// The form fields these options are sent as.
    fn params(self) -> Vec<Param> {
        let mut params = Vec::new();
        for (name, value) in [
            ("Options.DnsSupport", self.dns_support),
            (
                "Options.SecurityGroupReferencingSupport",
                self.security_group_referencing_support,
            ),
            ("Options.Ipv6Support", self.ipv6_support),
            ("Options.ApplianceModeSupport", self.appliance_mode_support),
        ] {
            if let Some(value) = value {
                params.push(param(name, value));
            }
        }
        params
    }
}

/// Which gateway a row attaches to, which decides the options its attachment inherits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gateway {
    /// The gateway created with no options: `DnsSupport` `enable`,
    /// `SecurityGroupReferencingSupport` `disable`.
    Default,
    /// The gateway created with both of those reversed.
    Reversed,
}

/// Which VPC a row attaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    Primary,
    Secondary,
}

/// One create to drive: the gateway, the VPC, the zone indexes of the subnets it names (in the
/// order it names them), the options it asks for and the tags it applies.
struct Case {
    label: &'static str,
    gateway: Gateway,
    which: Which,
    subnets: &'static [usize],
    options: RequestedOptions,
    tags: &'static [(&'static str, &'static str)],
}

/// The expected value of one option member: what the request asked for, else the gateway's own
/// option where the gateway has one, else the member's documented default.
fn expected_option(
    requested: Option<&'static str>,
    inherited: Option<&'static str>,
    default: &'static str,
) -> String {
    requested.or(inherited).unwrap_or(default).to_string()
}

/// A VPC attachment is computed from the request and the store: a freshly minted id, the
/// gateway and VPC the request names, the VPC's owner from the stored VPC, the subnets it
/// names de-duplicated in order, the options it asked for or inherited, its tags, and the
/// non-terminal `pending` state — and every member reads back identically through
/// `DescribeTransitGatewayVpcAttachments`.
#[tokio::test]
async fn a_vpc_attachment_is_computed_from_the_request_and_the_store() {
    let cases = [
        Case {
            label: "one subnet, no options, no tags",
            gateway: Gateway::Default,
            which: Which::Primary,
            subnets: &[0],
            options: RequestedOptions::default(),
            tags: &[],
        },
        Case {
            label: "two subnets in reverse zone order, one tag",
            gateway: Gateway::Default,
            which: Which::Primary,
            subnets: &[2, 0],
            options: RequestedOptions::default(),
            tags: &[("Name", "attachment-a")],
        },
        Case {
            label: "three subnets, every option asked for, three tags",
            gateway: Gateway::Default,
            which: Which::Primary,
            subnets: &[0, 1, 2],
            options: RequestedOptions {
                dns_support: Some(OPTION_DISABLE),
                security_group_referencing_support: Some(OPTION_ENABLE),
                ipv6_support: Some(OPTION_ENABLE),
                appliance_mode_support: Some(OPTION_ENABLE),
            },
            tags: &[("Name", "attachment-b"), ("Stage", "test"), ("Owner", "net")],
        },
        Case {
            label: "one subnet named twice, no options",
            gateway: Gateway::Default,
            which: Which::Primary,
            subnets: &[1, 1],
            options: RequestedOptions::default(),
            tags: &[],
        },
        Case {
            label: "the other VPC on the gateway whose options are reversed",
            gateway: Gateway::Reversed,
            which: Which::Secondary,
            subnets: &[0],
            options: RequestedOptions::default(),
            tags: &[("Name", "attachment-c")],
        },
        Case {
            label: "the reversed gateway with two options asked for",
            gateway: Gateway::Reversed,
            which: Which::Primary,
            subnets: &[1, 2],
            options: RequestedOptions {
                dns_support: Some(OPTION_ENABLE),
                ipv6_support: Some(OPTION_DISABLE),
                ..RequestedOptions::default()
            },
            tags: &[],
        },
    ];

    let fixture = Fixture::new().await;
    let mut minted: Vec<String> = Vec::new();
    for case in &cases {
        let label = case.label;
        // Each row attaches to a transit gateway of its own — created here with the options
        // the row's gateway kind states — so no row is refused as a second attachment of one
        // VPC to one gateway, and what an attachment inherits is read off the gateway it was
        // made on. The gateway created with no options at all carries `DnsSupport` "Enabled by
        // default" (`TransitGatewayRequestOptions`); the reversed one carries it the other way
        // round. `SecurityGroupReferencingSupport` is NOT inherited — the attachment's own
        // default is `enable`, as the options an attachment created without the member carries
        // show — so each row's gateway setting for it is not read.
        let (transit_gateway_id, inherited_dns, _gateway_group_referencing) = match case.gateway {
            Gateway::Default => (
                create_transit_gateway(&fixture.router, &[]).await,
                OPTION_ENABLE,
                OPTION_DISABLE,
            ),
            Gateway::Reversed => (
                create_transit_gateway(
                    &fixture.router,
                    &[
                        param("Options.DnsSupport", OPTION_DISABLE),
                        param("Options.SecurityGroupReferencingSupport", OPTION_ENABLE),
                    ],
                )
                .await,
                OPTION_DISABLE,
                OPTION_ENABLE,
            ),
        };
        let network = match case.which {
            Which::Primary => &fixture.primary,
            Which::Secondary => &fixture.secondary,
        };
        let named: Vec<&str> = case
            .subnets
            .iter()
            .map(|index| network.subnet(*index))
            .collect();
        let mut expected_subnets: Vec<String> = Vec::new();
        for subnet_id in &named {
            if !expected_subnets.iter().any(|held| held == *subnet_id) {
                expected_subnets.push((*subnet_id).to_string());
            }
        }

        let before = SystemTime::now();
        let mut params = vec![
            param("TransitGatewayId", &transit_gateway_id),
            param("VpcId", &network.vpc_id),
        ];
        params.extend(indexed("SubnetIds", &named));
        params.extend(case.options.params());
        params.extend(tag_params(case.tags));
        let (status, body) = create(&fixture.router, &params).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200, got {status} with body {body}"
        );
        let got = reported(label, &body);

        assert!(
            is_long_id(TRANSIT_GATEWAY_ATTACHMENT_ID_PREFIX, &got.attachment_id),
            "case {label:?}: expected a minted {TRANSIT_GATEWAY_ATTACHMENT_ID_PREFIX} id, got \
             {:?}",
            got.attachment_id
        );
        assert!(
            !minted.contains(&got.attachment_id),
            "case {label:?}: expected an id no attachment already holds, got {:?} beside \
             {minted:?}",
            got.attachment_id
        );
        assert_eq!(
            got.transit_gateway_id, transit_gateway_id,
            "case {label:?}: expected the gateway {transit_gateway_id:?}, got {:?}",
            got.transit_gateway_id
        );
        assert_eq!(
            got.vpc_id, network.vpc_id,
            "case {label:?}: expected the VPC {:?}, got {:?}",
            network.vpc_id, got.vpc_id
        );
        assert_eq!(
            got.vpc_owner_id,
            engine::emulator_account_id(),
            "case {label:?}: expected the account that owns the VPC {:?}, got {:?}",
            engine::emulator_account_id(),
            got.vpc_owner_id
        );
        assert_eq!(
            got.state, TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING,
            "case {label:?}: expected the non-terminal \
             {TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING:?}, got {:?}",
            got.state
        );
        assert_eq!(
            got.subnet_ids, expected_subnets,
            "case {label:?}: expected the subnets {expected_subnets:?} in the order named, got \
             {:?}",
            got.subnet_ids
        );
        let expected_options: Vec<(String, String)> = vec![
            (
                "dnsSupport".to_string(),
                expected_option(case.options.dns_support, Some(inherited_dns), OPTION_ENABLE),
            ),
            (
                // The ATTACHMENT's own default is `enable`, which a recorded attachment
                // created without the member carries: the gateway-level default the transit
                // gateway documentation names is the GATEWAY's own setting and is not what a
                // new attachment inherits, so the gateway's value is not read here.
                "securityGroupReferencingSupport".to_string(),
                expected_option(
                    case.options.security_group_referencing_support,
                    None,
                    OPTION_ENABLE,
                ),
            ),
            (
                "ipv6Support".to_string(),
                expected_option(case.options.ipv6_support, None, OPTION_DISABLE),
            ),
            (
                "applianceModeSupport".to_string(),
                expected_option(case.options.appliance_mode_support, None, OPTION_DISABLE),
            ),
        ];
        let mut sorted_options = got.options.clone();
        sorted_options.sort();
        let mut sorted_expected = expected_options.clone();
        sorted_expected.sort();
        assert_eq!(
            sorted_options, sorted_expected,
            "case {label:?}: expected the options {sorted_expected:?}, got {sorted_options:?}"
        );
        let expected_tags: Vec<(String, String)> = case
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            got.tags, expected_tags,
            "case {label:?}: expected the requested tags {expected_tags:?}, got {:?}",
            got.tags
        );
        let created_at = as_timestamp(&got.creation_time).unwrap_or_else(|| {
            panic!(
                "case {label:?}: expected an ISO 8601 creationTime, got {:?}",
                got.creation_time
            )
        });
        let floor = before
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("the test clock is after the epoch")
            .as_secs_f64();
        let ceiling = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("the test clock is after the epoch")
            .as_secs_f64();
        assert!(
            created_at >= floor - 1.0 && created_at <= ceiling + 1.0,
            "case {label:?}: expected a creationTime inside the call, got {created_at} outside \
             [{floor}, {ceiling}]"
        );

        let read_back = described(label, &fixture.router, &got.attachment_id).await;
        assert_eq!(
            read_back, got,
            "case {label:?}: expected the describe to report the created attachment, got \
             {read_back:?}"
        );
        minted.push(got.attachment_id);
    }
}

/// The clock owes the new attachment `available`: it is `pending` straight after the create and
/// in service once the spans have passed — its gateway's and then its own — with nothing slept
/// for in between.
#[tokio::test]
async fn the_clock_owes_the_new_attachment_available() {
    let case = "pending -> available";
    let fixture = Fixture::new().await;

    let (status, body) = create(
        &fixture.router,
        &[
            param("TransitGatewayId", &fixture.default_gateway),
            param("VpcId", &fixture.primary.vpc_id),
            param("SubnetIds.1", fixture.primary.subnet(0)),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let attachment_id = reported(case, &body).attachment_id;
    let straight_after = described(case, &fixture.router, &attachment_id).await.state;
    assert_eq!(
        straight_after, TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING,
        "case {case:?}: expected {TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING:?} straight after the \
         create, got {straight_after:?}"
    );

    tokio::time::sleep(past_the_attachment_transition()).await;
    let later = described(case, &fixture.router, &attachment_id).await.state;
    assert_eq!(
        later, TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
        "case {case:?}: expected {TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE:?} once the owed \
         moves are due, got {later:?}"
    );
}

/// One refusal to drive: the parameters, and the code and sentence EC2 answers them with.
struct Refusal {
    label: String,
    params: Vec<Param>,
    code: String,
    message: String,
}

/// Every refusal of this create carries EC2's own code and sentence, and stores nothing.
#[tokio::test]
async fn the_create_refuses_a_parent_it_cannot_attach() {
    let fixture = Fixture::new().await;
    let gateway = param("TransitGatewayId", &fixture.default_gateway);
    let vpc = param("VpcId", &fixture.primary.vpc_id);
    let mut refusals: Vec<Refusal> = Vec::new();

    // Not a subnet id at all: the wrong resource's prefix, no prefix, a digit count outside the
    // two the id form allows, upper-case hex, and an empty value.
    for malformed in [
        "vpc-0123456789abcdef0",
        "not-a-subnet-id",
        "subnet-0123456789ab",
        "subnet-0123456789ABCDEF0",
        "subnet-",
    ] {
        refusals.push(Refusal {
            label: format!("malformed subnet id {malformed:?}"),
            params: vec![gateway.clone(), vpc.clone(), param("SubnetIds.1", malformed)],
            code: INVALID_SUBNET_ID_MALFORMED.to_string(),
            message: format!("The subnet ID '{malformed}' is malformed"),
        });
    }

    // Well formed, but no subnet of the account holds it — the first unresolvable id of the
    // request's own list, whichever position it is in.
    let unknown_subnet = format!("subnet-{UNKNOWN_SUFFIX}");
    for (label, named) in [
        ("an unknown subnet alone", vec![unknown_subnet.as_str()]),
        (
            "an unknown subnet after a known one",
            vec![fixture.primary.subnet(0), unknown_subnet.as_str()],
        ),
    ] {
        let mut params = vec![gateway.clone(), vpc.clone()];
        params.extend(indexed("SubnetIds", &named));
        refusals.push(Refusal {
            label: label.to_string(),
            params,
            code: INVALID_SUBNET_ID_NOT_FOUND.to_string(),
            message: format!("The subnet ID '{unknown_subnet}' does not exist"),
        });
    }

    // A subnet of ANOTHER VPC: it is no subnet of the VPC being attached, so it is no subnet
    // this attachment can place an address in. The transit gateway family words THAT refusal
    // its own way — "Subnet <id> was deleted or does not exist." — where an id the store holds
    // no subnet under at all gets the VPC surface's sentence above.
    let unreachable = fixture.secondary.subnet(0).to_string();
    refusals.push(Refusal {
        label: "a subnet of another VPC".to_string(),
        params: vec![
            gateway.clone(),
            vpc.clone(),
            param("SubnetIds.1", &unreachable),
        ],
        code: INVALID_SUBNET_ID_NOT_FOUND.to_string(),
        message: format!("Subnet {unreachable} was deleted or does not exist."),
    });

    // A VPC this account does not hold is refused for the subnets of it that are not there
    // either, which is the refusal AWS answers such a request with. The subnet named DOES
    // exist, but not in the VPC the request asks to attach, so the sentence is the transit
    // gateway family's own for a subnet this attachment cannot reach.
    let unknown_vpc = format!("vpc-{UNKNOWN_SUFFIX}");
    refusals.push(Refusal {
        label: "a VPC no VPC of the account has".to_string(),
        params: vec![
            gateway.clone(),
            param("VpcId", &unknown_vpc),
            param("SubnetIds.1", fixture.primary.subnet(0)),
        ],
        code: INVALID_SUBNET_ID_NOT_FOUND.to_string(),
        message: format!(
            "Subnet {subnet_id} was deleted or does not exist.",
            subnet_id = fixture.primary.subnet(0)
        ),
    });

    // Two subnets of one Availability Zone, which the attachment admits only one of. The
    // fixture's subnets are one per zone, so the two named here are two distinct subnets of two
    // VPCs in one zone — seeded as a third subnet of the primary VPC in the first zone.
    let same_zone_subnet = create_subnet(
        &fixture.router,
        &fixture.primary.vpc_id,
        "10.90.200.0/24",
        ZONES[0],
    )
    .await;
    refusals.push(Refusal {
        label: "two subnets of one Availability Zone".to_string(),
        params: vec![
            gateway.clone(),
            vpc.clone(),
            param("SubnetIds.1", fixture.primary.subnet(0)),
            param("SubnetIds.2", &same_zone_subnet),
        ],
        code: DUPLICATE_SUBNETS_IN_SAME_ZONE.to_string(),
        message: format!(
            "The subnets {held} and {same_zone_subnet} are both in the Availability Zone \
             {zone}; you can specify only one subnet per Availability Zone",
            held = fixture.primary.subnet(0),
            zone = ZONES[0],
        ),
    });

    // A transit gateway this account does not hold: there is nothing to attach the VPC to.
    let unknown_gateway = format!("tgw-{UNKNOWN_SUFFIX}");
    refusals.push(Refusal {
        label: "a transit gateway no gateway of the account has".to_string(),
        params: vec![
            param("TransitGatewayId", &unknown_gateway),
            vpc.clone(),
            param("SubnetIds.1", fixture.primary.subnet(0)),
        ],
        code: INVALID_TRANSIT_GATEWAY_ID_NOT_FOUND.to_string(),
        message: format!("Transit Gateway {unknown_gateway} was deleted or does not exist."),
    });

    // A tag specification naming a resource type this action does not tag.
    refusals.push(Refusal {
        label: "tag specification of another resource type".to_string(),
        params: vec![
            gateway.clone(),
            vpc.clone(),
            param("SubnetIds.1", fixture.primary.subnet(0)),
            param("TagSpecifications.1.ResourceType", "vpc"),
            param("TagSpecifications.1.Tag.1.Key", "Name"),
            param("TagSpecifications.1.Tag.1.Value", "attachment"),
        ],
        code: INVALID_PARAMETER_VALUE.to_string(),
        message: format!("'vpc' is not a valid resource type for tagging on {ACTION}"),
    });

    let before = fixture.stored_attachments();
    for refusal in &refusals {
        let label = &refusal.label;
        let (status, body) = create(&fixture.router, &refusal.params).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, refusal.code,
            "case {label:?}: expected the code {:?}, got {code:?} with body {body}",
            refusal.code
        );
        let message = error_message(&body);
        assert_eq!(
            message, refusal.message,
            "case {label:?}: expected the message {:?}, got {message:?}",
            refusal.message
        );
        let now = fixture.stored_attachments();
        assert_eq!(
            now, before,
            "case {label:?}: a refusal must store nothing, got {now} VPC attachments where \
             {before} were held"
        );
    }
}

/// One VPC is attached to one transit gateway once: the second create is refused naming the
/// attachment already held, the same VPC on ANOTHER gateway is created, and an attachment the
/// lifecycle has finished with does not stand in the way of a new one.
#[tokio::test]
async fn one_vpc_is_attached_to_one_gateway_once() {
    let case = "a second attachment of one VPC";
    let fixture = Fixture::new().await;
    let params = [
        param("TransitGatewayId", &fixture.default_gateway),
        param("VpcId", &fixture.primary.vpc_id),
        param("SubnetIds.1", fixture.primary.subnet(0)),
    ];

    let (status, body) = create(&fixture.router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the first create must succeed, got {status} with body {body}"
    );
    let held = reported(case, &body).attachment_id;

    let before = fixture.stored_attachments();
    let (status, body) = create(&fixture.router, &params).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "DuplicateTransitGatewayAttachment",
        "case {case:?}: expected the duplicate-attachment code, got {code:?} with body {body}"
    );
    let message = error_message(&body);
    let expected = format!(
        "{gateway} has non-deleted Transit Gateway Attachment {held} with VPC {vpc}",
        gateway = fixture.default_gateway,
        vpc = fixture.primary.vpc_id,
    );
    assert_eq!(
        message, expected,
        "case {case:?}: expected the message {expected:?}, got {message:?}"
    );
    let now = fixture.stored_attachments();
    assert_eq!(
        now, before,
        "case {case:?}: a refusal must store nothing, got {now} VPC attachments where {before} \
         were held"
    );

    // The same VPC on another transit gateway is another attachment, not a duplicate.
    let other = "the same VPC on another gateway";
    let (status, body) = create(
        &fixture.router,
        &[
            param("TransitGatewayId", &fixture.reversed_gateway),
            param("VpcId", &fixture.primary.vpc_id),
            param("SubnetIds.1", fixture.primary.subnet(0)),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {other:?}: expected HTTP 200, got {status} with body {body}"
    );
    assert_ne!(
        reported(other, &body).attachment_id, held,
        "case {other:?}: expected an attachment of its own, got the one already held"
    );

    // An attachment the owner rejected attaches nothing any more, so the VPC it named can be
    // attached again — here on a third gateway, whose only attachment of that VPC is rejected.
    let finished = "a rejected attachment does not block a new one";
    let third_gateway = create_transit_gateway(&fixture.router, &[]).await;
    let rejected = fixture.seed_attachment(
        &third_gateway,
        &fixture.primary.vpc_id,
        TRANSIT_GATEWAY_ATTACHMENT_STATE_REJECTED,
    );
    let (status, body) = create(
        &fixture.router,
        &[
            param("TransitGatewayId", &third_gateway),
            param("VpcId", &fixture.primary.vpc_id),
            param("SubnetIds.1", fixture.primary.subnet(1)),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {finished:?}: expected HTTP 200, got {status} with body {body}"
    );
    assert_ne!(
        reported(finished, &body).attachment_id, rejected,
        "case {finished:?}: expected a freshly minted attachment, got the rejected one"
    );
}

/// A request the model's own constraints refuse never reaches this handler, and stores nothing:
/// the required `SubnetIds`, `VpcId` and `TransitGatewayId` left out, and an option value
/// outside the enum its member targets.
#[tokio::test]
async fn the_boundary_refuses_what_the_model_constrains() {
    let fixture = Fixture::new().await;
    let gateway = param("TransitGatewayId", &fixture.default_gateway);
    let vpc = param("VpcId", &fixture.primary.vpc_id);
    let subnet = param("SubnetIds.1", fixture.primary.subnet(0));
    let before = fixture.stored_attachments();
    let cases = [
        (
            "no SubnetIds at all",
            vec![gateway.clone(), vpc.clone()],
            BOUNDARY_MISSING_MEMBER,
        ),
        (
            "no VpcId",
            vec![gateway.clone(), subnet.clone()],
            BOUNDARY_MISSING_MEMBER,
        ),
        (
            "no TransitGatewayId",
            vec![vpc.clone(), subnet.clone()],
            BOUNDARY_MISSING_MEMBER,
        ),
        (
            "a DnsSupport outside the enum",
            vec![
                gateway.clone(),
                vpc.clone(),
                subnet.clone(),
                param("Options.DnsSupport", "enabled"),
            ],
            BOUNDARY_ENUM_VIOLATION,
        ),
        (
            "an empty Options.ApplianceModeSupport",
            vec![
                gateway.clone(),
                vpc.clone(),
                subnet.clone(),
                param("Options.ApplianceModeSupport", ""),
            ],
            BOUNDARY_ENUM_VIOLATION,
        ),
    ];

    for (case, params, expected) in &cases {
        let (status, body) = create(&fixture.router, params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            &code, expected,
            "case {case:?}: expected the code {expected:?}, got {code:?} with body {body}"
        );
        let now = fixture.stored_attachments();
        assert_eq!(
            now, before,
            "case {case:?}: a refusal must store nothing, got {now} VPC attachments where \
             {before} were held"
        );
    }
}

/// A dry run of a request that would have succeeded is refused with `DryRunOperation` and
/// stores nothing; a dry run of one that would fail is refused for the request instead.
#[tokio::test]
async fn a_dry_run_stores_nothing() {
    let case = "DryRun";
    let fixture = Fixture::new().await;
    let before = fixture.stored_attachments();

    let (status, body) = create(
        &fixture.router,
        &[
            param("TransitGatewayId", &fixture.default_gateway),
            param("VpcId", &fixture.primary.vpc_id),
            param("SubnetIds.1", fixture.primary.subnet(0)),
            param("DryRun", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, DRY_RUN_STATUS,
        "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
    );
    let now = fixture.stored_attachments();
    assert_eq!(
        now, before,
        "case {case:?}: a dry run must store nothing, got {now} VPC attachments where {before} \
         were held"
    );

    // The subnet id's own form is read before the permission check, so a dry run of a request
    // that could not have succeeded is refused for the request.
    let (status, body) = create(
        &fixture.router,
        &[
            param("TransitGatewayId", &fixture.default_gateway),
            param("VpcId", &fixture.primary.vpc_id),
            param("SubnetIds.1", "not-a-subnet-id"),
            param("DryRun", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: a malformed subnet id must be refused before the dry run, got {status} \
         with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, INVALID_SUBNET_ID_MALFORMED,
        "case {case:?}: expected the code {INVALID_SUBNET_ID_MALFORMED:?}, got {code:?}"
    );
}

/// The attachment this create stores is reported by the whole-family describe too, so one
/// record answers `DescribeTransitGatewayVpcAttachments` and
/// `DescribeTransitGatewayAttachments` alike.
#[tokio::test]
async fn the_family_describe_reports_the_created_attachment() {
    let case = "the family describe";
    let fixture = Fixture::new().await;
    let (status, body) = create(
        &fixture.router,
        &[
            param("TransitGatewayId", &fixture.default_gateway),
            param("VpcId", &fixture.primary.vpc_id),
            param("SubnetIds.1", fixture.primary.subnet(0)),
            param("SubnetIds.2", fixture.primary.subnet(1)),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let created = reported(case, &body);

    let body = ok_body(
        case,
        &fixture.router,
        "DescribeTransitGatewayAttachments",
        &indexed("TransitGatewayAttachmentIds", &[&created.attachment_id]),
    )
    .await;
    let parsed = document(case, &body);
    let reported_ids: Vec<String> = parsed
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "transitGatewayAttachmentId")
        .map(|node| node.text().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reported_ids,
        vec![created.attachment_id.clone()],
        "case {case:?}: expected the family describe to report only the created attachment, got \
         {reported_ids:?} in {body}"
    );
    let resource_id = element(&body, "resourceId");
    assert_eq!(
        resource_id.as_deref(),
        Some(fixture.primary.vpc_id.as_str()),
        "case {case:?}: expected the family describe to name the attached VPC, got \
         {resource_id:?} in {body}"
    );
    let state = element(&body, "state");
    assert_eq!(
        state.as_deref(),
        Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING),
        "case {case:?}: expected the family describe to report the stored state, got {state:?}"
    );

    // Unused in the assertions above, but part of what the create answered: the options block
    // is reported by the VPC-attachment describe, which the first test compares in full.
    assert!(
        !created.options.is_empty(),
        "case {case:?}: the create must answer with the attachment's options, got {created:?}"
    );
}
