// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateTransitGatewayPeeringAttachment` and the
//! `DescribeTransitGatewayPeeringAttachments` it reads back through, driven over the wire the
//! service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=CreateTransitGatewayPeeringAttachment`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test is the one EC2 holds a parameter whose value must DIFFER from another
//! parameter's to: a request that repeats the value is refused as a value it cannot carry —
//! HTTP 400 `InvalidParameterValue`, naming the value, the parameter it was sent for and the
//! parameter whose value it repeats — and that is decided on the request's OWN members, before
//! either value is resolved against the store, so the same refusal is answered whatever the
//! repeated value names. The pair this create publishes is the two ends of the peering it
//! records: it "requests a transit gateway peering attachment between the specified transit
//! gateway (requester) and a peer transit gateway (accepter)", so a request naming one gateway
//! as both ends asks for no peering at all.
//!
//! Each row of the refusal table repeats a DIFFERENT value, along what that generalises over:
//! a gateway of this account in service, one the clock has not finished bringing up, a
//! well-formed gateway id this account holds none for, a value that is no transit gateway id
//! at all, and one longer than the characters a refusal echoes — each carried beside different
//! other members (tags, a dynamic-routing option, a peer Region this store holds no gateway
//! in), because none of them changes the answer. The contrast tests peer two DISTINCT
//! gateways, which is served and reads back identically through the describe, and name a
//! distinct peer the account holds no gateway for, which is refused for THAT peer rather than
//! for a repetition.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ec2::ops::types::{DYNAMIC_ROUTING_VALUE_VALUES, TRANSIT_GATEWAY_ATTACHMENT_STATE_VALUES};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, INVALID_PARAMETER_VALUE,
    INVALID_TRANSIT_GATEWAY_ID_NOT_FOUND, MAX_ECHOED_RESOURCE_ID_CHARS, OPTION_DISABLE,
    RESOURCE_TYPE_TRANSIT_GATEWAY_ATTACHMENT, TRANSIT_GATEWAY_ATTACHMENT_ID_PREFIX,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE, TRANSITION_DELAY,
    TransitGatewayAttachmentView,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, call, element, error_code, error_message, indexed};
use super::wire::{Param, child, child_text, document, is_long_id, param, set_items};

/// The Region every call here is signed for, and the Region the peer of a served peering is
/// in: this store holds the gateways of ONE Region.
const REGION: &str = "us-east-1";

/// A Region other than the one these calls are signed for, so a peer placed in it is one this
/// store holds no gateway in at all.
const OTHER_REGION: &str = "us-west-2";

/// A well-formed transit gateway id no gateway of this account holds: the mint counts from
/// one, so the all-zero id is never issued.
const UNISSUED_GATEWAY_ID: &str = "tgw-00000000000000000";

/// The ec2Query `Action` under test.
const ACTION: &str = "CreateTransitGatewayPeeringAttachment";

/// Long enough past [`TRANSITION_DELAY`] that a move the clock owes is certainly due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// A member of `DynamicRoutingValue`, taken from the modelled catalog
/// (`smithy_query(query="shape",shape="DynamicRoutingValue")`, re-exported as
/// `DYNAMIC_ROUTING_VALUE_VALUES`) rather than written out here.
fn dynamic_routing(name: &str) -> &'static str {
    DYNAMIC_ROUTING_VALUE_VALUES
        .iter()
        .find(|value| **value == name)
        .unwrap_or_else(|| panic!("{name:?} is no DynamicRoutingValue member"))
}

/// A member of `TransitGatewayAttachmentState`, taken from the modelled catalog
/// (`smithy_query(query="shape",shape="TransitGatewayAttachmentState")`, re-exported as
/// `TRANSIT_GATEWAY_ATTACHMENT_STATE_VALUES`) rather than written out here.
fn attachment_state(name: &str) -> &'static str {
    TRANSIT_GATEWAY_ATTACHMENT_STATE_VALUES
        .iter()
        .find(|value| **value == name)
        .unwrap_or_else(|| panic!("{name:?} is no TransitGatewayAttachmentState member"))
}

/// One `CreateTransitGateway` call, answering the id of the gateway it created. The gateway
/// comes up `pending`, with the clock owing it `available` one [`TRANSITION_DELAY`] later.
async fn create_transit_gateway(router: &MultiServiceRouter) -> String {
    let (status, body) = call(router, REGION, "CreateTransitGateway", &[]).await;
    assert_eq!(
        status, 200,
        "the fixture's transit gateway must be created, got {status} with body {body}"
    );
    element(&body, "transitGatewayId").unwrap_or_else(|| panic!("no transitGatewayId in {body}"))
}

/// One `CreateTransitGatewayPeeringAttachment` call carrying `params`.
async fn create(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    call(router, REGION, ACTION, params).await
}

/// The four `@required` members of the request, with both ends and the peer's Region as given
/// and the peer account this emulator's own.
fn ends(transit_gateway_id: &str, peer_transit_gateway_id: &str, peer_region: &str) -> Vec<Param> {
    vec![
        param("TransitGatewayId", transit_gateway_id),
        param("PeerTransitGatewayId", peer_transit_gateway_id),
        param("PeerAccountId", engine::emulator_account_id()),
        param("PeerRegion", peer_region),
    ]
}

/// The form fields for the one tag specification a `CreateTransitGatewayPeeringAttachment` may
/// carry: the attachment it creates is the only resource it tags.
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

/// How many peering attachments the store holds — what a refusal must leave unchanged.
fn stored_peerings(engine: &Arc<engine::InMemoryEngine>) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .transit_gateway_attachments
        .values()
        .filter(|record| matches!(record.view, TransitGatewayAttachmentView::Peering(_)))
        .count()
}

/// One reported `TransitGatewayPeeringAttachment`, read out of the XML by the document rather
/// than by string surgery: the two `PeeringTgwInfo` ends carry identically-named members, so
/// each is read inside its own element.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    attachment_id: String,
    requester: (String, String, String),
    accepter: (String, String, String),
    state: String,
    dynamic_routing: Option<String>,
    creation_time: String,
    tags: Vec<(String, String)>,
}

/// The one peering attachment a response carries — the element that has a `requesterTgwInfo`
/// child, whether the response is a create's single `transitGatewayPeeringAttachment` or a
/// describe's `transitGatewayPeeringAttachments` set.
fn reported(case: &str, body: &str) -> Reported {
    let parsed = document(case, body);
    let mut found: Vec<Reported> = parsed
        .descendants()
        .filter(|node| node.is_element() && child(*node, "requesterTgwInfo").is_some())
        .map(|item| {
            let end = |name: &str| {
                child(item, name)
                    .map(|info| {
                        (
                            child_text(info, "transitGatewayId").unwrap_or_default(),
                            child_text(info, "ownerId").unwrap_or_default(),
                            child_text(info, "region").unwrap_or_default(),
                        )
                    })
                    .unwrap_or_default()
            };
            Reported {
                attachment_id: child_text(item, "transitGatewayAttachmentId").unwrap_or_default(),
                requester: end("requesterTgwInfo"),
                accepter: end("accepterTgwInfo"),
                state: child_text(item, "state").unwrap_or_default(),
                dynamic_routing: child(item, "options")
                    .and_then(|options| child_text(options, "dynamicRouting")),
                creation_time: child_text(item, "creationTime").unwrap_or_default(),
                tags: set_items(item, "tagSet")
                    .into_iter()
                    .map(|tag| {
                        (
                            child_text(tag, "key").unwrap_or_default(),
                            child_text(tag, "value").unwrap_or_default(),
                        )
                    })
                    .collect(),
            }
        })
        .collect();
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one peering attachment, got {found:?} in {body}"
    );
    found.remove(0)
}

/// The one peering attachment `DescribeTransitGatewayPeeringAttachments` reports for
/// `attachment_id`.
async fn described(case: &str, router: &MultiServiceRouter, attachment_id: &str) -> Reported {
    let (status, body) = call(
        router,
        REGION,
        "DescribeTransitGatewayPeeringAttachments",
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

/// Which value a refusal row names as BOTH ends of the peering.
#[derive(Debug, Clone, Copy)]
enum Repeated {
    /// A transit gateway of this account that is in service.
    GatewayInService,
    /// A transit gateway of this account the clock has not finished bringing up.
    GatewayComingUp,
    /// A well-formed transit gateway id this account holds no gateway for.
    UnissuedId,
    /// A value that is no transit gateway id at all.
    NotAGatewayId,
    /// A value longer than the characters a refusal echoes.
    Overlong,
}

/// One refusal row: the value it repeats, the peer Region it sends, and the members it carries
/// beside the four required ones.
struct Case {
    label: &'static str,
    repeated: Repeated,
    peer_region: &'static str,
    extra: Vec<Param>,
}

/// The two ends of a peering have to be different gateways: a request that names one value as
/// both is refused `InvalidParameterValue`, with EC2's own sentence naming the value and both
/// parameters, before either end is resolved — and nothing is stored.
#[tokio::test]
async fn the_two_ends_of_a_peering_must_be_different_gateways() {
    let (router, engine) = super::ec2_query::emulator();
    let in_service = create_transit_gateway(&router).await;
    tokio::time::sleep(past_the_transition()).await;

    let cases = vec![
        Case {
            label: "a gateway of this account in service",
            repeated: Repeated::GatewayInService,
            peer_region: REGION,
            extra: Vec::new(),
        },
        Case {
            label: "a gateway still coming up, with tags",
            repeated: Repeated::GatewayComingUp,
            peer_region: REGION,
            extra: tag_params(&[("Name", "peering"), ("Stage", "test")]),
        },
        Case {
            label: "an id this account holds no gateway for, with dynamic routing asked for",
            repeated: Repeated::UnissuedId,
            peer_region: REGION,
            extra: vec![param("Options.DynamicRouting", dynamic_routing("enable"))],
        },
        Case {
            label: "a value that is no transit gateway id, in another Region",
            repeated: Repeated::NotAGatewayId,
            peer_region: OTHER_REGION,
            extra: Vec::new(),
        },
        Case {
            label: "a value longer than a refusal echoes",
            repeated: Repeated::Overlong,
            peer_region: REGION,
            extra: Vec::new(),
        },
    ];

    for case in &cases {
        let label = case.label;
        let repeated = match case.repeated {
            Repeated::GatewayInService => in_service.clone(),
            // Created HERE rather than with the fixture, so the clock has certainly not
            // brought it up yet by the time this row calls.
            Repeated::GatewayComingUp => create_transit_gateway(&router).await,
            Repeated::UnissuedId => UNISSUED_GATEWAY_ID.to_string(),
            Repeated::NotAGatewayId => "not-a-transit-gateway-id".to_string(),
            Repeated::Overlong => format!("tgw-{}", "0".repeat(MAX_ECHOED_RESOURCE_ID_CHARS * 2)),
        };
        let mut params = ends(&repeated, &repeated, case.peer_region);
        params.extend(case.extra.iter().cloned());

        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, INVALID_PARAMETER_VALUE,
            "case {label:?}: expected the code {INVALID_PARAMETER_VALUE:?}, got {code:?} with \
             body {body}"
        );
        // Only the characters a refusal echoes are named, so a caller cannot grow the answer
        // by sending a longer value.
        let echoed: String = repeated.chars().take(MAX_ECHOED_RESOURCE_ID_CHARS).collect();
        let expected = format!(
            "Invalid value {echoed} for PeerTransitGatewayId. The value of \
             PeerTransitGatewayId needs to be different to TransitGatewayId"
        );
        let message = error_message(&body);
        assert_eq!(
            message, expected,
            "case {label:?}: expected the sentence {expected:?}, got {message:?}"
        );
    }

    let held = stored_peerings(&engine);
    assert_eq!(
        held, 0,
        "a refused request stores nothing, so the store must hold no peering attachment, got \
         {held}"
    );
}

/// One served peering to drive: the tags it applies and the dynamic routing it asks for.
struct Served {
    label: &'static str,
    tags: &'static [(&'static str, &'static str)],
    requested_routing: Option<&'static str>,
}

/// Two DISTINCT gateways are peered: the attachment is computed from the request and the store
/// — a freshly minted id, the requester and accepter ends with this account and Region, the
/// dynamic routing the request asked for or the member's documented default, the requested tags
/// and the state a peering REQUEST starts in — and every member reads back identically through
/// `DescribeTransitGatewayPeeringAttachments`.
#[tokio::test]
async fn two_distinct_gateways_are_peered_and_read_back() {
    let (router, engine) = super::ec2_query::emulator();
    let requester = create_transit_gateway(&router).await;
    let mut accepters = Vec::new();
    for _ in 0..2 {
        accepters.push(create_transit_gateway(&router).await);
    }
    tokio::time::sleep(past_the_transition()).await;

    let cases = [
        Served {
            label: "dynamic routing asked for, two tags",
            tags: &[("Name", "peering-a"), ("Stage", "test")],
            requested_routing: Some("enable"),
        },
        Served {
            label: "no options and no tags",
            tags: &[],
            requested_routing: None,
        },
    ];

    let account = engine::emulator_account_id().to_string();
    let mut minted: Vec<String> = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        let accepter = accepters[index].clone();
        let mut params = ends(&requester, &accepter, REGION);
        if !case.tags.is_empty() {
            params.extend(tag_params(case.tags));
        }
        if let Some(routing) = case.requested_routing {
            params.push(param("Options.DynamicRouting", dynamic_routing(routing)));
        }

        let before = SystemTime::now();
        let (status, body) = create(&router, &params).await;
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
            got.requester,
            (requester.clone(), account.clone(), REGION.to_string()),
            "case {label:?}: expected the requester end {:?}, got {:?}",
            (&requester, &account, REGION),
            got.requester
        );
        assert_eq!(
            got.accepter,
            (accepter.clone(), account.clone(), REGION.to_string()),
            "case {label:?}: expected the accepter end {:?}, got {:?}",
            (&accepter, &account, REGION),
            got.accepter
        );
        // "A request that names no dynamic routing takes the member's documented default,
        // which is that it is off."
        let expected_routing = case.requested_routing.unwrap_or(OPTION_DISABLE);
        assert_eq!(
            got.dynamic_routing.as_deref(),
            Some(expected_routing),
            "case {label:?}: expected the dynamic routing {expected_routing:?}, got {:?}",
            got.dynamic_routing
        );
        let starts_in = attachment_state("initiatingRequest");
        assert_eq!(
            got.state, starts_in,
            "case {label:?}: expected the state a peering request starts in {starts_in:?}, got \
             {:?}",
            got.state
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

        let read_back = described(label, &router, &got.attachment_id).await;
        assert_eq!(
            read_back, got,
            "case {label:?}: expected the describe to report the created attachment, got \
             {read_back:?}"
        );
        minted.push(got.attachment_id);
    }

    let held = stored_peerings(&engine);
    assert_eq!(
        held,
        cases.len(),
        "expected one stored peering attachment per served request, got {held}"
    );

    // "After you create the peering attachment, the owner of the accepter transit gateway must
    // accept the attachment request": the clock owes the request that waiting state, and
    // nothing is slept for in between.
    tokio::time::sleep(past_the_transition()).await;
    for attachment_id in &minted {
        let later = described("the clock owes pendingAcceptance", &router, attachment_id)
            .await
            .state;
        assert_eq!(
            later, TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE,
            "case {attachment_id:?}: expected \
             {TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE:?} once the owed move is due, \
             got {later:?}"
        );
    }
}

/// A peer that is DISTINCT from the requester but names no transit gateway this service can
/// peer with is refused for that peer — the transit gateway's own `NotFound` code, not the
/// refusal a repeated value earns — and stores nothing.
#[tokio::test]
async fn a_distinct_peer_naming_no_gateway_is_refused_for_the_peer() {
    let (router, engine) = super::ec2_query::emulator();
    let requester = create_transit_gateway(&router).await;
    tokio::time::sleep(past_the_transition()).await;

    for (label, peer_region) in [
        ("an unissued peer id in this Region", REGION),
        ("a gateway placed in another Region", OTHER_REGION),
    ] {
        let (status, body) = create(
            &router,
            &ends(&requester, UNISSUED_GATEWAY_ID, peer_region),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, INVALID_TRANSIT_GATEWAY_ID_NOT_FOUND,
            "case {label:?}: expected the code {INVALID_TRANSIT_GATEWAY_ID_NOT_FOUND:?}, got \
             {code:?} with body {body}"
        );
        let message = error_message(&body);
        assert!(
            !message.contains("needs to be different to"),
            "case {label:?}: a distinct peer is not refused for repeating the requester, got \
             {message:?}"
        );
    }

    let held = stored_peerings(&engine);
    assert_eq!(
        held, 0,
        "a refused request stores nothing, so the store must hold no peering attachment, got \
         {held}"
    );
}
