// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateTransitGatewayConnect` and the
//! `DescribeTransitGatewayConnects` it reads back through, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=CreateTransitGatewayConnect`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that a Connect attachment is COMPUTED FROM THE REQUEST AND THE
//! STORE, never echoed or guessed: its `TransitGatewayAttachmentId` comes from the store's id
//! mint, its `TransitGatewayId` from the TRANSPORT attachment's own record rather than from
//! the request, its tunnel protocol from the request's `Options`, its tags from the request's
//! `TagSpecifications`, and it is stored in the non-terminal `pending` state with the clock
//! owing it `available` — all of it readable back through `DescribeTransitGatewayConnects`.
//! Each table row is a distinct input along what that generalises over: which transport
//! attachment and so which transit gateway, and how many tags the request applies.
//!
//! Every published constraint of the request has a refusal row carrying EC2's own error code
//! and sentence: a transport id that is not a transit gateway attachment id at all, one that
//! names no attachment, one naming an attachment of each kind that cannot carry a Connect
//! attachment, one naming a transport attachment in each state that is not in service, and a
//! tag specification naming another resource type. Nothing is stored by any of them.
//!
//! `CreateTransitGatewayVpcAttachment` is still a scaffold, so the TRANSPORT attachments are
//! seeded straight into the store the way the Connect-peer and attachment-acceptance suites
//! seed the attachments their operations act on; the transit gateways themselves are created
//! over the wire, so the gateway id a created Connect attachment reports is one that
//! operation really stored.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ec2::ops::types::{
    PeeringTgwInfo, TransitGatewayClientVpnAttachment, TransitGatewayConnect,
    TransitGatewayConnectOptions, TransitGatewayPeeringAttachment, TransitGatewayVpcAttachment,
};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, INCORRECT_STATE, INVALID_PARAMETER_VALUE,
    INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MESSAGE, PROTOCOL_VALUE_GRE,
    RESOURCE_TYPE_TRANSIT_GATEWAY_ATTACHMENT, TRANSIT_GATEWAY_ATTACHMENT_ID_PREFIX,
    TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_CLIENT_VPN,
    TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_CONNECT,
    TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_PEERING, TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VPC,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE, TRANSIT_GATEWAY_ATTACHMENT_STATE_DELETING,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_FAILED, TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE, TRANSITION_DELAY,
    TransitGatewayAttachmentView,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, call, error_code, error_message, indexed};
use super::wire::{Param, child, child_text, document, is_long_id, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// Long enough past [`TRANSITION_DELAY`] that a move the clock owes is certainly due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// The transport attachments the fixture seeds: a label, which of the two transit gateways the
/// attachment is on, and the state it is in.
///
/// Two attachments in service on DIFFERENT gateways, so a created Connect attachment's
/// `TransitGatewayId` can only come from the transport attachment it was made from; and one
/// in each of three states that are not in service.
struct TransportSpec {
    label: &'static str,
    gateway: usize,
    state: &'static str,
}

/// The transport attachments every test of this suite creates Connect attachments from, or is
/// refused by.
fn transports() -> Vec<TransportSpec> {
    vec![
        TransportSpec {
            label: "available on gateway A",
            gateway: 0,
            state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
        },
        TransportSpec {
            label: "available on gateway B",
            gateway: 1,
            state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
        },
        TransportSpec {
            label: "pendingAcceptance on gateway A",
            gateway: 0,
            state: TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE,
        },
        TransportSpec {
            label: "deleting on gateway A",
            gateway: 0,
            state: TRANSIT_GATEWAY_ATTACHMENT_STATE_DELETING,
        },
        TransportSpec {
            label: "failed on gateway B",
            gateway: 1,
            state: TRANSIT_GATEWAY_ATTACHMENT_STATE_FAILED,
        },
    ]
}

/// One seeded transport attachment: the id the store minted for it beside the gateway it is on
/// and the state it is in, so a report is compared against the record rather than a literal.
#[derive(Debug, Clone)]
struct Transport {
    label: &'static str,
    id: String,
    transit_gateway_id: String,
    state: String,
}

/// The emulator, two transit gateways created over the wire, the transport attachments
/// [`transports`] describes seeded on the first of them, and one attachment of each kind that
/// cannot carry a Connect attachment.
struct Fixture {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    transports: Vec<Transport>,
    /// The attachment kinds no Connect attachment can be made from: the kind's own
    /// `TransitGatewayAttachmentResourceType` beside the id the store minted for it.
    other_kinds: Vec<(&'static str, String)>,
}

impl Fixture {
    /// The fixture, with every seeded attachment's minted id resolved.
    async fn new() -> Self {
        let (router, engine) = super::ec2_query::emulator();
        let mut gateways = Vec::new();
        for index in 0..2 {
            let (status, body) = call(&router, REGION, "CreateTransitGateway", &[]).await;
            assert_eq!(
                status, 200,
                "the fixture's transit gateway {index} must be created, got {status} with \
                 body {body}"
            );
            gateways.push(
                super::ec2_query::element(&body, "transitGatewayId")
                    .unwrap_or_else(|| panic!("no transitGatewayId in {body}")),
            );
        }

        let mut seeded = Vec::new();
        for spec in transports() {
            let transit_gateway_id = gateways[spec.gateway].clone();
            let id = engine
                .state::<Ec2State>()
                .lock()
                .insert_transit_gateway_attachment(TransitGatewayAttachmentView::Vpc(
                    TransitGatewayVpcAttachment {
                        transit_gateway_id: Some(transit_gateway_id.clone()),
                        vpc_id: Some("vpc-0123456789abcdef0".to_string()),
                        state: Some(spec.state.to_string()),
                        ..Default::default()
                    },
                ))
                .unwrap_or_else(|| {
                    panic!("the attachment store has room for the seed {:?}", spec.label)
                });
            seeded.push(Transport {
                label: spec.label,
                id,
                transit_gateway_id,
                state: spec.state.to_string(),
            });
        }

        let mut other_kinds = Vec::new();
        let peering = engine
            .state::<Ec2State>()
            .lock()
            .insert_transit_gateway_attachment(TransitGatewayAttachmentView::Peering(
                TransitGatewayPeeringAttachment {
                    requester_tgw_info: Some(PeeringTgwInfo {
                        transit_gateway_id: Some(gateways[0].clone()),
                        ..Default::default()
                    }),
                    state: Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE.to_string()),
                    ..Default::default()
                },
            ))
            .expect("the attachment store has room for one peering attachment");
        other_kinds.push((TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_PEERING, peering));
        let client_vpn = engine
            .state::<Ec2State>()
            .lock()
            .insert_transit_gateway_attachment(TransitGatewayAttachmentView::ClientVpn(
                TransitGatewayClientVpnAttachment {
                    transit_gateway_id: Some(gateways[0].clone()),
                    client_vpn_endpoint_id: Some("cvpn-endpoint-0123456789abcdef0".to_string()),
                    state: Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE.to_string()),
                    ..Default::default()
                },
            ))
            .expect("the attachment store has room for one Client VPN attachment");
        other_kinds.push((
            TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_CLIENT_VPN,
            client_vpn,
        ));
        let connect = engine
            .state::<Ec2State>()
            .lock()
            .insert_transit_gateway_attachment(TransitGatewayAttachmentView::Connect(
                TransitGatewayConnect {
                    transit_gateway_id: Some(gateways[0].clone()),
                    transport_transit_gateway_attachment_id: Some(seeded[0].id.clone()),
                    state: Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE.to_string()),
                    options: Some(TransitGatewayConnectOptions {
                        protocol: Some(PROTOCOL_VALUE_GRE.to_string()),
                    }),
                    ..Default::default()
                },
            ))
            .expect("the attachment store has room for one Connect attachment");
        other_kinds.push((TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_CONNECT, connect));

        Self {
            router,
            engine,
            transports: seeded,
            other_kinds,
        }
    }

    /// The seeded transport attachment `label` names.
    fn transport(&self, label: &str) -> &Transport {
        self.transports
            .iter()
            .find(|transport| transport.label == label)
            .unwrap_or_else(|| panic!("no seeded transport attachment labelled {label:?}"))
    }

    /// How many Connect attachments the store holds — what a refusal must leave unchanged.
    fn stored_connects(&self) -> usize {
        self.engine
            .state::<Ec2State>()
            .lock()
            .transit_gateway_attachments
            .values()
            .filter(|record| {
                matches!(record.view, TransitGatewayAttachmentView::Connect(_))
            })
            .count()
    }

    /// The state the store holds for the attachment `attachment_id`, or `None` when it holds
    /// none — read straight out of the store, so a move the clock owes is read as the store
    /// has it rather than as a describe rendered it.
    fn stored_state(&self, attachment_id: &str) -> Option<String> {
        self.engine
            .state::<Ec2State>()
            .lock()
            .transit_gateway_attachments
            .get(attachment_id)
            .and_then(|record| record.view.state().map(str::to_string))
    }
}

/// One `CreateTransitGatewayConnect` call carrying `params`.
async fn create(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    call(router, REGION, "CreateTransitGatewayConnect", params).await
}

/// One reported `TransitGatewayConnect`, read out of the XML by the document rather than by
/// string surgery — `state` and `protocol` appear both on the attachment and inside its
/// options.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    attachment_id: String,
    transport_id: String,
    transit_gateway_id: String,
    state: String,
    creation_time: String,
    protocol: String,
    tags: Vec<(String, String)>,
}

/// Every Connect attachment a response carries, in document order: the elements that have a
/// `transportTransitGatewayAttachmentId` child, whether the response is a create's single
/// `transitGatewayConnect` or a describe's `transitGatewayConnectSet`.
fn reported_all(case: &str, body: &str) -> Vec<Reported> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| {
            node.is_element() && child(*node, "transportTransitGatewayAttachmentId").is_some()
        })
        .map(|item| Reported {
            attachment_id: child_text(item, "transitGatewayAttachmentId").unwrap_or_default(),
            transport_id: child_text(item, "transportTransitGatewayAttachmentId")
                .unwrap_or_default(),
            transit_gateway_id: child_text(item, "transitGatewayId").unwrap_or_default(),
            state: child_text(item, "state").unwrap_or_default(),
            creation_time: child_text(item, "creationTime").unwrap_or_default(),
            protocol: child(item, "options")
                .and_then(|options| child_text(options, "protocol"))
                .unwrap_or_default(),
            tags: child(item, "tagSet")
                .map(|tag_set| {
                    tag_set
                        .children()
                        .filter(|node| node.is_element() && node.tag_name().name() == "item")
                        .map(|tag| {
                            (
                                child_text(tag, "key").unwrap_or_default(),
                                child_text(tag, "value").unwrap_or_default(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect()
}

/// The one Connect attachment a response carries, or a panic naming the case.
fn reported(case: &str, body: &str) -> Reported {
    let mut all = reported_all(case, body);
    assert_eq!(
        all.len(),
        1,
        "case {case:?}: expected exactly one Connect attachment, got {all:?} in {body}"
    );
    all.remove(0)
}

/// The one Connect attachment `DescribeTransitGatewayConnects` reports for `attachment_id`.
async fn described(case: &str, router: &MultiServiceRouter, attachment_id: &str) -> Reported {
    let (status, body) = call(
        router,
        REGION,
        "DescribeTransitGatewayConnects",
        &indexed("TransitGatewayAttachmentIds", &[attachment_id]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the describe of {attachment_id} must succeed, got {status} with body \
         {body}"
    );
    reported(case, &body)
}

/// The form fields for one tag at position `index` (counting from 1) of the one tag
/// specification a `CreateTransitGatewayConnect` may carry.
fn tag_params(tags: &[(&str, &str)]) -> Vec<Param> {
    let mut params = vec![param(
        "TagSpecification.1.ResourceType",
        RESOURCE_TYPE_TRANSIT_GATEWAY_ATTACHMENT,
    )];
    for (index, (key, value)) in tags.iter().enumerate() {
        params.push(param(&format!("TagSpecification.1.Tag.{}.Key", index + 1), key));
        params.push(param(
            &format!("TagSpecification.1.Tag.{}.Value", index + 1),
            value,
        ));
    }
    params
}

/// One create to drive: which seeded transport attachment it is made from, and the tags it
/// applies.
struct Case {
    label: &'static str,
    transport: &'static str,
    tags: &'static [(&'static str, &'static str)],
}

/// A Connect attachment is created from its transport attachment: a freshly minted id, the
/// transport attachment it names, the gateway THAT attachment is on, the requested tunnel
/// protocol and tags, and the non-terminal `pending` state — and every member reads back
/// identically through `DescribeTransitGatewayConnects`.
#[tokio::test]
async fn a_connect_attachment_is_created_from_its_transport_attachment() {
    let cases = [
        Case {
            label: "transport on gateway A, no tags",
            transport: "available on gateway A",
            tags: &[],
        },
        Case {
            label: "transport on gateway B, one tag",
            transport: "available on gateway B",
            tags: &[("Name", "appliance-b")],
        },
        Case {
            label: "transport on gateway A, three tags",
            transport: "available on gateway A",
            tags: &[("Name", "appliance-a"), ("Stage", "test"), ("Owner", "net")],
        },
    ];

    let fixture = Fixture::new().await;
    let mut minted: Vec<String> = Vec::new();
    for case in &cases {
        let label = case.label;
        let transport = fixture.transport(case.transport).clone();
        let before = SystemTime::now();
        let mut params = vec![
            param("TransportTransitGatewayAttachmentId", &transport.id),
            param("Options.Protocol", PROTOCOL_VALUE_GRE),
        ];
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
            !minted.contains(&got.attachment_id) && got.attachment_id != transport.id,
            "case {label:?}: expected an id no attachment already holds, got {:?} beside \
             {minted:?} and the transport {:?}",
            got.attachment_id,
            transport.id
        );
        assert_eq!(
            got.transport_id, transport.id,
            "case {label:?}: expected the transport attachment {:?}, got {:?}",
            transport.id, got.transport_id
        );
        assert_eq!(
            got.transit_gateway_id, transport.transit_gateway_id,
            "case {label:?}: expected the gateway of the transport attachment {:?}, got {:?}",
            transport.transit_gateway_id, got.transit_gateway_id
        );
        assert_eq!(
            got.state, TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING,
            "case {label:?}: expected the non-terminal \
             {TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING:?}, got {:?}",
            got.state
        );
        assert_eq!(
            got.protocol, PROTOCOL_VALUE_GRE,
            "case {label:?}: expected the requested tunnel protocol {PROTOCOL_VALUE_GRE:?}, \
             got {:?}",
            got.protocol
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
            "case {label:?}: expected a creationTime inside the call, got {created_at} \
             outside [{floor}, {ceiling}]"
        );

        let read_back = described(label, &fixture.router, &got.attachment_id).await;
        assert_eq!(
            read_back, got,
            "case {label:?}: expected the describe to report the created attachment, got \
             {read_back:?}"
        );
        minted.push(got.attachment_id);
    }

    // The transport attachments are READ by this create, so nothing about them moved.
    for transport in &fixture.transports {
        let state = fixture.stored_state(&transport.id);
        assert_eq!(
            state.as_deref(),
            Some(transport.state.as_str()),
            "case {:?}: the transport attachment must still be in {:?}, got {state:?}",
            transport.label,
            transport.state
        );
    }
}

/// The clock owes the new Connect attachment `available`: it is `pending` straight after the
/// create and in service once the span has passed, with nothing slept for in between.
#[tokio::test]
async fn the_clock_owes_the_new_attachment_available() {
    let case = "pending -> available";
    let fixture = Fixture::new().await;
    let transport = fixture.transport("available on gateway A").clone();

    let (status, body) = create(
        &fixture.router,
        &[
            param("TransportTransitGatewayAttachmentId", &transport.id),
            param("Options.Protocol", PROTOCOL_VALUE_GRE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let attachment_id = reported(case, &body).attachment_id;
    let straight_after = described(case, &fixture.router, &attachment_id)
        .await
        .state;
    assert_eq!(
        straight_after, TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING,
        "case {case:?}: expected {TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING:?} straight after \
         the create, got {straight_after:?}"
    );

    tokio::time::sleep(past_the_transition()).await;
    let later = described(case, &fixture.router, &attachment_id).await.state;
    assert_eq!(
        later, TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
        "case {case:?}: expected {TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE:?} once the \
         owed move is due, got {later:?}"
    );
}

/// One refusal to drive: the parameters, and the code and sentence EC2 answers them with.
struct Refusal {
    label: String,
    params: Vec<Param>,
    code: &'static str,
    message: String,
}

/// Every refusal of this create carries EC2's own code and sentence, and stores nothing.
#[tokio::test]
async fn the_create_refuses_a_transport_attachment_it_cannot_build_on() {
    let fixture = Fixture::new().await;
    let options = param("Options.Protocol", PROTOCOL_VALUE_GRE);
    let mut refusals: Vec<Refusal> = Vec::new();

    // Not a transit gateway attachment id at all: the wrong resource's prefix, no prefix, a
    // digit count outside the two the id form allows, upper-case hex, and an empty value.
    for malformed in [
        "vpc-0123456789abcdef0",
        "not-an-attachment-id",
        "tgw-attach-0123456789ab",
        "tgw-attach-0123456789ABCDEF0",
        "tgw-attach-",
        "",
    ] {
        refusals.push(Refusal {
            label: format!("malformed transport id {malformed:?}"),
            params: vec![
                param("TransportTransitGatewayAttachmentId", malformed),
                options.clone(),
            ],
            code: INVALID_PARAMETER_VALUE,
            message: INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MESSAGE.to_string(),
        });
    }

    // Well formed, but no attachment of the account holds it. The transport attachment is a
    // PARAMETER VALUE of this operation rather than the resource it acts on, so an id naming
    // no attachment of the account is refused as a value it cannot act on — the code this
    // operation is recorded answering such a transport with, where the attachment's own
    // `NotFound` belongs to the operations whose subject IS the attachment.
    let unknown = "tgw-attach-00000000000000000";
    refusals.push(Refusal {
        label: format!("unknown transport id {unknown:?}"),
        params: vec![
            param("TransportTransitGatewayAttachmentId", unknown),
            options.clone(),
        ],
        code: INVALID_PARAMETER_VALUE,
        message: format!(
            "Invalid value '{unknown}' for transportTransitGatewayAttachmentId. The transit \
             gateway attachment to create the Connect attachment from could not be found"
        ),
    });

    // An attachment of a kind a Connect attachment cannot be made from, each sharing the one
    // id space this create resolves in.
    for (kind, attachment_id) in &fixture.other_kinds {
        refusals.push(Refusal {
            label: format!("transport of kind {kind:?}"),
            params: vec![
                param("TransportTransitGatewayAttachmentId", attachment_id),
                options.clone(),
            ],
            code: INVALID_PARAMETER_VALUE,
            message: format!(
                "The transit gateway attachment '{attachment_id}' is a '{kind}' attachment; a \
                 Connect attachment can only be created from a \
                 '{TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VPC}' attachment or an Amazon Web \
                 Services Direct Connect attachment"
            ),
        });
    }

    // A transport attachment that is not in service, in each state the fixture seeded.
    for transport in &fixture.transports {
        if transport.state == TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE {
            continue;
        }
        refusals.push(Refusal {
            label: format!("transport {:?}", transport.label),
            params: vec![
                param("TransportTransitGatewayAttachmentId", &transport.id),
                options.clone(),
            ],
            code: INCORRECT_STATE,
            message: format!(
                "The transit gateway attachment '{id}' is in state '{state}'; a Connect \
                 attachment can only be created from an attachment in state \
                 '{TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE}'",
                id = transport.id,
                state = transport.state,
            ),
        });
    }

    // A tag specification naming a resource type this action does not tag.
    let transport = fixture.transport("available on gateway A").clone();
    refusals.push(Refusal {
        label: "tag specification of another resource type".to_string(),
        params: vec![
            param("TransportTransitGatewayAttachmentId", &transport.id),
            options.clone(),
            param("TagSpecification.1.ResourceType", "vpc"),
            param("TagSpecification.1.Tag.1.Key", "Name"),
            param("TagSpecification.1.Tag.1.Value", "appliance"),
        ],
        code: INVALID_PARAMETER_VALUE,
        message: "'vpc' is not a valid resource type for tagging on \
                  CreateTransitGatewayConnect"
            .to_string(),
    });

    let before = fixture.stored_connects();
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
        let now = fixture.stored_connects();
        assert_eq!(
            now, before,
            "case {label:?}: a refusal must store nothing, got {now} Connect attachments \
             where {before} were held"
        );
    }
}

/// The code the request-boundary validator refuses a member the model requires and the
/// request leaves out with, before any handler of this service runs.
const BOUNDARY_MISSING_MEMBER: &str = "MalformedInput";

/// The code that same validator refuses a value outside a modelled enum with.
const BOUNDARY_ENUM_VIOLATION: &str = "ValidationError";

/// A request that does not name the tunnel protocol the `Options` require is refused before
/// any state is read, and stores nothing: the structure left out entirely, and the protocol
/// left empty or outside the one member its enum publishes.
#[tokio::test]
async fn the_create_requires_the_tunnel_protocol() {
    let fixture = Fixture::new().await;
    let transport = fixture.transport("available on gateway A").clone();
    let before = fixture.stored_connects();
    let id = param("TransportTransitGatewayAttachmentId", &transport.id);
    let cases = [
        ("no Options at all", vec![id.clone()], BOUNDARY_MISSING_MEMBER),
        (
            "an empty Options.Protocol",
            vec![id.clone(), param("Options.Protocol", "")],
            BOUNDARY_ENUM_VIOLATION,
        ),
        (
            "a protocol outside the enum",
            vec![id.clone(), param("Options.Protocol", "ipsec")],
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
        let now = fixture.stored_connects();
        assert_eq!(
            now, before,
            "case {case:?}: a refusal must store nothing, got {now} Connect attachments where \
             {before} were held"
        );
    }
}

/// A dry run of a request that would have succeeded is refused with `DryRunOperation` and
/// stores nothing.
#[tokio::test]
async fn a_dry_run_stores_nothing() {
    let case = "DryRun";
    let fixture = Fixture::new().await;
    let transport = fixture.transport("available on gateway A").clone();
    let before = fixture.stored_connects();

    let (status, body) = create(
        &fixture.router,
        &[
            param("TransportTransitGatewayAttachmentId", &transport.id),
            param("Options.Protocol", PROTOCOL_VALUE_GRE),
            param("DryRun", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, DRY_RUN_STATUS,
        "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
    );
    let now = fixture.stored_connects();
    assert_eq!(
        now, before,
        "case {case:?}: a dry run must store nothing, got {now} Connect attachments where \
         {before} were held"
    );

    // A dry run of a request that would FAIL is refused for the request, not for the dry run:
    // the id's own form is read before the permission check.
    let (status, body) = create(
        &fixture.router,
        &[
            param("TransportTransitGatewayAttachmentId", "not-an-attachment-id"),
            param("Options.Protocol", PROTOCOL_VALUE_GRE),
            param("DryRun", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: a malformed id must be refused before the dry run, got {status} with \
         body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, INVALID_PARAMETER_VALUE,
        "case {case:?}: expected the code {INVALID_PARAMETER_VALUE:?}, got {code:?}"
    );
}

/// Two creates from one transport attachment are two attachments: the id is minted per call,
/// and both read back through the describe.
#[tokio::test]
async fn each_create_mints_its_own_attachment() {
    let case = "two creates, one transport";
    let fixture = Fixture::new().await;
    let transport = fixture.transport("available on gateway B").clone();
    let params = [
        param("TransportTransitGatewayAttachmentId", &transport.id),
        param("Options.Protocol", PROTOCOL_VALUE_GRE),
    ];

    let mut ids = Vec::new();
    for attempt in 0..2 {
        let (status, body) = create(&fixture.router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: create {attempt} must succeed, got {status} with body {body}"
        );
        ids.push(reported(case, &body).attachment_id);
    }
    assert_ne!(
        ids[0], ids[1],
        "case {case:?}: expected two distinct attachment ids, got {ids:?}"
    );
    for id in &ids {
        let got = described(case, &fixture.router, id).await;
        assert_eq!(
            got.transport_id, transport.id,
            "case {case:?}: expected {id} to name the transport {:?}, got {:?}",
            transport.id, got.transport_id
        );
    }
}

/// The ids list of a `DescribeTransitGatewayConnects` reports only the attachments created
/// here, so the id space the create mints in is the one the describe resolves in.
#[tokio::test]
async fn the_describe_resolves_the_minted_ids() {
    let case = "ids of the created attachments";
    let fixture = Fixture::new().await;
    let transport = fixture.transport("available on gateway A").clone();
    let (status, body) = create(
        &fixture.router,
        &[
            param("TransportTransitGatewayAttachmentId", &transport.id),
            param("Options.Protocol", PROTOCOL_VALUE_GRE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let created = reported(case, &body).attachment_id;

    let (status, body) = call(
        &fixture.router,
        REGION,
        "DescribeTransitGatewayConnects",
        &indexed("TransitGatewayAttachmentIds", &[&created]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the describe must succeed, got {status} with body {body}"
    );
    let got = reported(case, &body);
    assert_eq!(
        got.attachment_id, created,
        "case {case:?}: expected the describe to resolve {created:?}, got {:?}",
        got.attachment_id
    );
}
