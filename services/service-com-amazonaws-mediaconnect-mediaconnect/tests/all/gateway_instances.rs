// SPDX-License-Identifier: CC0-1.0

//! A gateway's instances, end to end over HTTP: DeregisterGatewayInstance and the reader it
//! is proved through, DescribeGatewayInstance.
//!
//! The rules asserted here are:
//!
//! * DeregisterGatewayInstance deregisters the instance its ARN addresses: the instance
//!   enters the non-terminal `DEREGISTERING` and the clock owes it the terminal
//!   `DEREGISTERED` one transition span later, so a caller reading it straight afterwards
//!   still sees the deregistration in flight;
//! * it refuses, with the modelled `ConflictException` (409), an instance that has bridges
//!   RUNNING on it unless the request sets `Force` ("before you deregister an instance, all
//!   bridges running on the instance must be stopped. If you want to deregister an instance
//!   without stopping the bridges, you must use the --force option"), and an instance whose
//!   registration is still in flight;
//! * a repeat of the `@idempotent` call on an instance already deregistering or
//!   deregistered answers that instance's current state and starts nothing;
//! * an ARN no instance has is the modelled `NotFoundException` (404), in this service's
//!   not-found envelope;
//! * DescribeGatewayInstance reports the STORED record — including `RunningBridgeCount`,
//!   which is the bridges placed on the instance's gateway that are not stopped — and keeps
//!   answering for a deregistered instance, because `DEREGISTERED` is a terminal value of
//!   the modelled `InstanceState` enum;
//! * DeleteGateway refuses a gateway that still has registered instances ("before you can
//!   delete a gateway, you must deregister its instances and delete its bridges").
//!
//! Each table's rows differ along what the rule generalises over: the `InstanceState` the
//! instance is in (every value of the modelled enum), how many bridges are running on its
//! gateway and in which `BridgeState`, and whether the request sets `Force` to true, to
//! false or not at all.
//!
//! An instance is never created by a client: the model declares no operation that registers
//! one, because on AWS the gateway software's SSM install on the on-premises appliance
//! registers it. That registration reaches the service out of band, through
//! `mc::ComAmazonawsMediaconnectMediaconnectState::register_gateway_instance`, which is the
//! channel the fixtures below use.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case drives the same request path a MediaConnect client reaches: routing, restJson1
//! deserialization, constraint validation, handler, serialization.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::bridge_lifecycle::create_bridge;
use crate::bridge_members_and_deletion::force_bridge_state;
use crate::flow_lifecycle::Emulator;

/// Long enough for a transition the clock owes (`mc::TRANSITION_SPAN`) to come due.
///
/// `pub(crate)` so `tests/all/describe_gateway_instance.rs` waits out the same span rather
/// than keeping a second copy of it.
pub(crate) const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// `GET /v1/gateway-instances/{GatewayInstanceArn}`.
///
/// `pub(crate)` because `tests/all/describe_gateway_instance.rs` drives the same request
/// through the same front door rather than spelling the binding a second time.
pub(crate) async fn describe_gateway_instance(
    emulator: &Emulator,
    arn: &str,
) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/gateway-instances/{arn}"), None)
        .await
}

/// `DELETE /v1/gateway-instances/{GatewayInstanceArn}`, with `Force` as the `force` query
/// parameter the model binds it to (`smithy_query(query="operation",
/// operation="DeregisterGatewayInstance")`: `Force` is an `@httpQuery("force")`), or no
/// query parameter at all when the row leaves it unset.
async fn deregister_gateway_instance(
    emulator: &Emulator,
    arn: &str,
    force: Option<bool>,
) -> (u16, Option<String>, Value) {
    let query = match force {
        Some(force) => format!("?force={force}"),
        None => String::new(),
    };
    emulator
        .send(
            "DELETE",
            &format!("/v1/gateway-instances/{arn}{query}"),
            None,
        )
        .await
}

async fn delete_gateway(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/gateways/{arn}"), None)
        .await
}

/// Create one gateway over the wire and answer its ARN.
///
/// The create is `gateway_lifecycle`'s subject; here it is only the fixture every instance
/// below is registered with. `pub(crate)` for the same reason as
/// [`describe_gateway_instance`].
pub(crate) async fn gateway_with(emulator: &Emulator, name: &str) -> String {
    let (status, code, created) = emulator
        .send(
            "POST",
            "/v1/gateways",
            Some(json!({
                "name": name,
                "egressCidrBlocks": ["10.0.0.0/16"],
                "networks": [{ "name": "net-1", "cidrBlock": "10.0.0.0/24" }],
            })),
        )
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "fixture {name:?}: create expected 201, got {status} {code:?}: {created}"
    );
    created["gateway"]["gatewayArn"]
        .as_str()
        .unwrap_or_else(|| panic!("fixture {name:?}: the create answers an ARN: {created}"))
        .to_string()
}

/// Register one instance with `gateway_arn` through the out-of-band channel, and answer its
/// ARN.
///
/// This is the registration AWS performs when the gateway software is installed on the
/// appliance: the model declares no operation for it (see this module's header), so the
/// fixture goes through the store's own seam rather than over the wire.
pub(crate) fn register_instance(emulator: &Emulator, gateway_arn: &str) -> String {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let mut store = state.lock();
    store
        .register_gateway_instance(gateway_arn, std::time::SystemTime::now())
        .gateway_instance_arn
}

/// Register one instance with `gateway_arn` and put it in `instance_state`.
///
/// The registration itself passes through `REGISTERING` and then `ACTIVE`; every other value
/// of the modelled `InstanceState` enum is one the appliance reports out of band too (a
/// registration or deregistration error) or one an operation moves the instance into, so the
/// tables below force the state the way the reporter of it would rather than leaving most of
/// the enum unasserted.
///
/// `pub(crate)` because `tests/all/describe_gateway_instance.rs` forces the same seam.
pub(crate) fn instance_in_state(
    emulator: &Emulator,
    gateway_arn: &str,
    instance_state: &str,
) -> String {
    let instance_arn = register_instance(emulator, gateway_arn);
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let mut store = state.lock();
    let instance = store
        .gateway_instances
        .get_mut(&instance_arn)
        .unwrap_or_else(|| panic!("the fixture instance {instance_arn} is stored"));
    instance.instance_state = instance_state.to_string();
    // The state is forced, so the transition the registration left pending is not owed any
    // more.
    instance.pending = None;
    instance_arn
}

/// Place one bridge on `gateway_arn` in `bridge_state`, over the wire, and answer its ARN.
fn running_bridge_body(name: &str, placement_arn: &str) -> Value {
    json!({
        "name": name,
        "placementArn": placement_arn,
        "ingressGatewayBridge": { "maxBitrate": 20_000_000, "maxOutputs": 8 },
        "sources": [{ "networkSource": {
            "multicastIp": "224.0.1.1",
            "name": "src",
            "networkName": "net-1",
            "port": 5000,
            "protocol": "rtp",
        }}],
    })
}

/// Place `bridge_states.len()` bridges on `gateway_arn`, each in the state the row names.
async fn bridges_on(emulator: &Emulator, gateway_arn: &str, case: &str, bridge_states: &[&str]) {
    for (index, bridge_state) in bridge_states.iter().enumerate() {
        let name = format!("bridge-{}-{index}", case.len());
        let (status, code, created) =
            create_bridge(emulator, running_bridge_body(&name, gateway_arn)).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: the fixture bridge create expected 201, got {status} {code:?}: \
             {created}"
        );
        let arn = created["bridge"]["bridgeArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create answers an ARN: {created}"))
            .to_string();
        force_bridge_state(emulator, &arn, bridge_state);
    }
}

/// The `GatewayInstance` DescribeGatewayInstance reports for `arn`, asserted to be reported
/// at all.
async fn instance_of(emulator: &Emulator, case: &str, arn: &str) -> Value {
    let (status, code, described) = describe_gateway_instance(emulator, arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: describe expected 200, got {status} {code:?}: {described}"
    );
    described["gatewayInstance"].clone()
}

/// What a deregistration of an instance in a given state, with given bridges running on its
/// gateway, must answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// 202 with `DEREGISTERING`, settling into the terminal `DEREGISTERED`.
    Accepted,
    /// 202 with the state the instance already has, and no new transition: the repeat of an
    /// `@idempotent` call.
    AlreadyUnderWay,
    /// 409 `ConflictException`, with the instance left in the state it was in.
    Conflict,
}

/// One deregistration case.
#[derive(Debug)]
struct Case {
    case: &'static str,
    /// The state the instance is in when the deregistration arrives.
    instance_state: &'static str,
    /// The `BridgeState` of each bridge placed on the instance's gateway.
    bridge_states: &'static [&'static str],
    /// `Force`, as the request carries it: `None` leaves the query parameter off.
    force: Option<bool>,
    expected: Expected,
}

/// The deregistration table. The rows vary every input the rule generalises over: the
/// instance's state (every value of the modelled `InstanceState` enum —
/// `smithy_query(query="shape",shape="InstanceState")`), how many bridges are running on its
/// gateway and in which `BridgeState`, and whether `Force` is set to true, to false, or left
/// off the request entirely.
const CASES: &[Case] = &[
    Case {
        case: "an active instance with no bridges on its gateway",
        instance_state: mc::INSTANCE_STATE_ACTIVE,
        bridge_states: &[],
        force: None,
        expected: Expected::Accepted,
    },
    Case {
        case: "an active instance whose gateway's only bridge is stopped",
        instance_state: mc::INSTANCE_STATE_ACTIVE,
        bridge_states: &[mc::BRIDGE_STATE_STANDBY],
        force: None,
        expected: Expected::Accepted,
    },
    Case {
        case: "an active instance whose gateway's bridges are all stopped, three of them",
        instance_state: mc::INSTANCE_STATE_ACTIVE,
        bridge_states: &[
            mc::BRIDGE_STATE_STANDBY,
            mc::BRIDGE_STATE_CREATING,
            mc::BRIDGE_STATE_START_FAILED,
        ],
        force: None,
        expected: Expected::Accepted,
    },
    Case {
        case: "one running bridge, no Force on the request",
        instance_state: mc::INSTANCE_STATE_ACTIVE,
        bridge_states: &[mc::BRIDGE_STATE_ACTIVE],
        force: None,
        expected: Expected::Conflict,
    },
    Case {
        case: "one running bridge, Force explicitly false",
        instance_state: mc::INSTANCE_STATE_ACTIVE,
        bridge_states: &[mc::BRIDGE_STATE_ACTIVE],
        force: Some(false),
        expected: Expected::Conflict,
    },
    Case {
        case: "one running bridge, Force true",
        instance_state: mc::INSTANCE_STATE_ACTIVE,
        bridge_states: &[mc::BRIDGE_STATE_ACTIVE],
        force: Some(true),
        expected: Expected::Accepted,
    },
    Case {
        case: "a bridge that is only on its way to running, no Force",
        instance_state: mc::INSTANCE_STATE_ACTIVE,
        bridge_states: &[mc::BRIDGE_STATE_STARTING],
        force: None,
        expected: Expected::Conflict,
    },
    Case {
        case: "a mix of running and stopped bridges, no Force",
        instance_state: mc::INSTANCE_STATE_ACTIVE,
        bridge_states: &[
            mc::BRIDGE_STATE_STANDBY,
            mc::BRIDGE_STATE_DEPLOYING,
            mc::BRIDGE_STATE_UPDATING,
        ],
        force: None,
        expected: Expected::Conflict,
    },
    Case {
        case: "a mix of running and stopped bridges, Force true",
        instance_state: mc::INSTANCE_STATE_ACTIVE,
        bridge_states: &[
            mc::BRIDGE_STATE_STANDBY,
            mc::BRIDGE_STATE_DEPLOYING,
            mc::BRIDGE_STATE_UPDATING,
        ],
        force: Some(true),
        expected: Expected::Accepted,
    },
    Case {
        case: "an instance whose registration is still in flight",
        instance_state: mc::INSTANCE_STATE_REGISTERING,
        bridge_states: &[],
        force: None,
        expected: Expected::Conflict,
    },
    Case {
        case: "an instance whose registration is still in flight, Force true",
        instance_state: mc::INSTANCE_STATE_REGISTERING,
        bridge_states: &[],
        force: Some(true),
        expected: Expected::Conflict,
    },
    Case {
        case: "an instance whose registration failed",
        instance_state: mc::INSTANCE_STATE_REGISTRATION_ERROR,
        bridge_states: &[],
        force: None,
        expected: Expected::Accepted,
    },
    Case {
        case: "an instance whose previous deregistration failed",
        instance_state: mc::INSTANCE_STATE_DEREGISTRATION_ERROR,
        bridge_states: &[mc::BRIDGE_STATE_ACTIVE],
        force: Some(true),
        expected: Expected::Accepted,
    },
    Case {
        case: "an instance already deregistering",
        instance_state: mc::INSTANCE_STATE_DEREGISTERING,
        bridge_states: &[mc::BRIDGE_STATE_ACTIVE],
        force: None,
        expected: Expected::AlreadyUnderWay,
    },
    Case {
        case: "an instance already deregistered",
        instance_state: mc::INSTANCE_STATE_DEREGISTERED,
        bridge_states: &[],
        force: None,
        expected: Expected::AlreadyUnderWay,
    },
];

/// The rule, over the whole table: the bridges running on the instance and the state it is
/// in decide the answer, and a refused deregistration leaves the instance exactly as it was.
#[tokio::test]
async fn deregistering_an_instance_follows_its_state_and_its_running_bridges() {
    for (index, row) in CASES.iter().enumerate() {
        // One emulator per row: the running-bridge count is a property of the GATEWAY's
        // bridges, so each row needs its own gateway with only its own bridges on it.
        let emulator = Emulator::new();
        let case = row.case;
        let gateway_arn = gateway_with(&emulator, &format!("gw-deregister-{index}")).await;
        bridges_on(&emulator, &gateway_arn, case, row.bridge_states).await;
        let instance_arn = instance_in_state(&emulator, &gateway_arn, row.instance_state);

        let before = instance_of(&emulator, case, &instance_arn).await;
        let (status, code, answered) =
            deregister_gateway_instance(&emulator, &instance_arn, row.force).await;

        match row.expected {
            Expected::Accepted => {
                assert_eq!(
                    (status, code.as_deref()),
                    (202, None),
                    "case {case:?}: an accepted deregistration is 202, got {status} {code:?}: \
                     {answered}"
                );
                assert_eq!(
                    (
                        answered["gatewayInstanceArn"].as_str(),
                        answered["instanceState"].as_str()
                    ),
                    (
                        Some(instance_arn.as_str()),
                        Some(mc::INSTANCE_STATE_DEREGISTERING)
                    ),
                    "case {case:?}: the answer is the instance's ARN and its transient \
                     DEREGISTERING state, got {answered}"
                );

                // A caller reading straight after the call still sees the transient state,
                // because the transition span is never zero.
                let described = instance_of(&emulator, case, &instance_arn).await;
                assert_eq!(
                    described["instanceState"].as_str(),
                    Some(mc::INSTANCE_STATE_DEREGISTERING),
                    "case {case:?}: an instance read straight after the deregistration is \
                     still DEREGISTERING, got {described}"
                );

                tokio::time::sleep(PAST_THE_TRANSITION).await;
                let described = instance_of(&emulator, case, &instance_arn).await;
                assert_eq!(
                    (
                        described["instanceState"].as_str(),
                        described["connectionStatus"].as_str(),
                        described["runningBridgeCount"].as_i64()
                    ),
                    (
                        Some(mc::INSTANCE_STATE_DEREGISTERED),
                        Some(mc::CONNECTION_STATUS_DISCONNECTED),
                        Some(0)
                    ),
                    "case {case:?}: the clock settles the instance into the terminal \
                     DEREGISTERED, disconnected and hosting nothing, got {described}"
                );
            }
            Expected::AlreadyUnderWay => {
                assert_eq!(
                    (status, code.as_deref()),
                    (202, None),
                    "case {case:?}: a repeated deregistration is 202, got {status} {code:?}: \
                     {answered}"
                );
                assert_eq!(
                    (
                        answered["gatewayInstanceArn"].as_str(),
                        answered["instanceState"].as_str()
                    ),
                    (Some(instance_arn.as_str()), Some(row.instance_state)),
                    "case {case:?}: the repeat answers the state the instance already has, \
                     got {answered}"
                );
                let described = instance_of(&emulator, case, &instance_arn).await;
                assert_eq!(
                    described, before,
                    "case {case:?}: a repeated deregistration starts nothing, so the \
                     instance is unchanged; expected {before}, got {described}"
                );
            }
            Expected::Conflict => {
                assert_eq!(
                    (status, code.as_deref()),
                    (409, Some("ConflictException")),
                    "case {case:?}: a refused deregistration is a 409 ConflictException, got \
                     {status} {code:?}: {answered}"
                );
                let described = instance_of(&emulator, case, &instance_arn).await;
                assert_eq!(
                    described, before,
                    "case {case:?}: a refused deregistration changes nothing; expected \
                     {before}, got {described}"
                );
            }
        }
    }
}

/// `RunningBridgeCount` is computed from the bridges the store holds for the instance's
/// gateway: the ones that are not stopped. The rows vary the states and the count, and one
/// row places a running bridge on ANOTHER gateway to pin that the count is per gateway.
#[tokio::test]
async fn an_instance_reports_the_bridges_running_on_its_gateway() {
    // (the bridge states placed on the instance's gateway, the count it must report)
    let rows: [(&[&str], i64); 5] = [
        (&[], 0),
        (&[mc::BRIDGE_STATE_STANDBY, mc::BRIDGE_STATE_CREATING], 0),
        (&[mc::BRIDGE_STATE_ACTIVE], 1),
        (
            &[
                mc::BRIDGE_STATE_ACTIVE,
                mc::BRIDGE_STATE_STARTING,
                mc::BRIDGE_STATE_STANDBY,
                mc::BRIDGE_STATE_DEPLOYING,
            ],
            3,
        ),
        (
            &[
                mc::BRIDGE_STATE_STOPPING,
                mc::BRIDGE_STATE_STOP_FAILED,
                mc::BRIDGE_STATE_START_PENDING,
                mc::BRIDGE_STATE_UPDATING,
                mc::BRIDGE_STATE_START_FAILED,
            ],
            4,
        ),
    ];

    for (index, (bridge_states, expected)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let case = format!("{bridge_states:?} -> {expected}");
        let gateway_arn = gateway_with(&emulator, &format!("gw-count-{index}")).await;
        // A running bridge on a DIFFERENT gateway is none of this instance's business.
        let other_gateway = gateway_with(&emulator, &format!("gw-count-other-{index}")).await;
        bridges_on(&emulator, &other_gateway, &case, &[mc::BRIDGE_STATE_ACTIVE]).await;
        bridges_on(&emulator, &gateway_arn, &case, bridge_states).await;
        let instance_arn = instance_in_state(&emulator, &gateway_arn, mc::INSTANCE_STATE_ACTIVE);

        let described = instance_of(&emulator, &case, &instance_arn).await;
        assert_eq!(
            described["runningBridgeCount"].as_i64(),
            Some(expected),
            "case {case:?}: the instance reports the bridges running on its gateway, got \
             {described}"
        );
    }
}

/// The record DescribeGatewayInstance reports is the one the registration stored, and the
/// ARN it is addressed by is of the form the modelled `GatewayInstanceArn` fixes: the
/// gateway's own ARN with the instance under it.
#[tokio::test]
async fn a_registered_instance_is_reported_from_the_stored_record() {
    let emulator = Emulator::new();
    let gateway_arn = gateway_with(&emulator, "gw-registration").await;
    let instance_arn = register_instance(&emulator, &gateway_arn);

    assert!(
        mc::is_mintable_gateway_instance_arn(&instance_arn),
        "the registered instance's ARN must be one this service could mint, got \
         {instance_arn:?}"
    );
    assert_eq!(
        mc::gateway_arn_of_instance(&instance_arn),
        Some(gateway_arn.as_str()),
        "an instance ARN nests the ARN of the gateway it is registered with, got \
         {instance_arn:?}"
    );

    // A caller reading straight after the registration still sees it in flight.
    let described = instance_of(&emulator, "straight after the registration", &instance_arn).await;
    assert_eq!(
        (
            described["gatewayInstanceArn"].as_str(),
            described["gatewayArn"].as_str(),
            described["instanceState"].as_str(),
            described["bridgePlacement"].as_str(),
            described["connectionStatus"].as_str(),
            described["runningBridgeCount"].as_i64(),
        ),
        (
            Some(instance_arn.as_str()),
            Some(gateway_arn.as_str()),
            Some(mc::INSTANCE_STATE_REGISTERING),
            Some(mc::BRIDGE_PLACEMENT_AVAILABLE),
            Some(mc::CONNECTION_STATUS_CONNECTED),
            Some(0),
        ),
        "a freshly registered instance is REGISTERING, available and connected, got \
         {described}"
    );
    let instance_id = described["instanceId"].as_str().unwrap_or_default();
    assert!(
        instance_id.starts_with("mi-") && mc::is_valid_gateway_instance_id(instance_id),
        "`InstanceId` is the id the SSM install generates and begins with \"mi-\", got \
         {instance_id:?}"
    );
    assert!(
        described["instanceMessages"].is_null(),
        "an instance this emulator serves reports no messages, got {described}"
    );

    // Deregistering is refused while the registration is in flight…
    let (status, code, answered) =
        deregister_gateway_instance(&emulator, &instance_arn, None).await;
    assert_eq!(
        (status, code.as_deref()),
        (409, Some("ConflictException")),
        "deregistering an instance that is still REGISTERING is a 409, got {status} \
         {code:?}: {answered}"
    );

    // …and accepted once the clock has settled the registration.
    tokio::time::sleep(PAST_THE_TRANSITION).await;
    let described = instance_of(&emulator, "once the registration settled", &instance_arn).await;
    assert_eq!(
        described["instanceState"].as_str(),
        Some(mc::INSTANCE_STATE_ACTIVE),
        "the clock settles a registered instance into ACTIVE, got {described}"
    );
    let (status, code, answered) =
        deregister_gateway_instance(&emulator, &instance_arn, None).await;
    assert_eq!(
        (status, code.as_deref(), answered["instanceState"].as_str()),
        (202, None, Some(mc::INSTANCE_STATE_DEREGISTERING)),
        "an ACTIVE instance with no running bridges is deregistered, got {status} {code:?}: \
         {answered}"
    );
}

/// An ARN no instance has is the modelled `NotFoundException`, on both operations, in this
/// service's not-found envelope — never a 501, never a success, and never storing anything.
/// The rows are the ways an instance ARN can miss.
#[tokio::test]
async fn an_arn_no_instance_has_is_not_found() {
    let emulator = Emulator::new();
    let gateway_arn = gateway_with(&emulator, "gw-not-found").await;
    let registered = instance_in_state(&emulator, &gateway_arn, mc::INSTANCE_STATE_ACTIVE);

    let unknown_gateway = format!(
        "{}:instance:mi-0000000000000beef",
        engine::emulator_arn(
            mc::SERVICE_NAMESPACE,
            "us-east-1",
            &mc::gateway_resource_fragment(4_242, "never-created"),
        )
    );
    let rows = [
        (
            "an instance id no instance has, on a gateway that exists",
            mc::gateway_instance_arn(&gateway_arn, "mi-0000000000000dead"),
        ),
        (
            "an instance of a gateway that was never created",
            unknown_gateway,
        ),
        (
            "an instance id of a form this service never mints",
            format!("{gateway_arn}:instance:-"),
        ),
    ];

    for (case, arn) in rows {
        let (status, code, answered) = describe_gateway_instance(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: describe expected a 404 NotFoundException, got {status} \
             {code:?}: {answered}"
        );
        assert_eq!(
            answered["message"].as_str(),
            Some(mc::not_found_message(&arn).as_str()),
            "case {case:?}: the message is this service's not-found envelope around the \
             request's own label, got {answered}"
        );

        let (status, code, answered) = deregister_gateway_instance(&emulator, &arn, None).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: deregister expected a 404 NotFoundException, got {status} \
             {code:?}: {answered}"
        );
        assert_eq!(
            answered["message"].as_str(),
            Some(mc::not_found_message(&arn).as_str()),
            "case {case:?}: the message is this service's not-found envelope around the \
             request's own label, got {answered}"
        );
    }

    // The instance that does exist is untouched by every miss above.
    let described = instance_of(&emulator, "the registered instance", &registered).await;
    assert_eq!(
        described["instanceState"].as_str(),
        Some(mc::INSTANCE_STATE_ACTIVE),
        "a not-found on another ARN leaves the registered instance alone, got {described}"
    );
}

/// "Before you can delete a gateway, you must deregister its instances and delete its
/// bridges": the delete is refused while an instance is still registered, and accepted once
/// the deregistration has settled.
#[tokio::test]
async fn a_gateway_with_registered_instances_cannot_be_deleted() {
    let emulator = Emulator::new();
    let gateway_arn = gateway_with(&emulator, "gw-with-instances").await;
    let instance_arn = instance_in_state(&emulator, &gateway_arn, mc::INSTANCE_STATE_ACTIVE);

    let (status, code, answered) = delete_gateway(&emulator, &gateway_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "deleting a gateway that still has a registered instance is refused, got {status} \
         {code:?}: {answered}"
    );
    let message = answered["message"].as_str().unwrap_or_default();
    assert!(
        message.starts_with(mc::BAD_REQUEST_PREFIX) && message.contains(&gateway_arn),
        "the refusal is this service's state-refusal envelope naming the gateway, got \
         {message:?}"
    );

    let (status, code, answered) =
        deregister_gateway_instance(&emulator, &instance_arn, None).await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "the instance is deregistered, got {status} {code:?}: {answered}"
    );

    // The instance is on its way out but not out yet, so the precondition still holds.
    let (status, code, answered) = delete_gateway(&emulator, &gateway_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "a deregistration still in flight leaves the gateway undeletable, got {status} \
         {code:?}: {answered}"
    );

    tokio::time::sleep(PAST_THE_TRANSITION).await;
    let (status, code, answered) = delete_gateway(&emulator, &gateway_arn).await;
    assert_eq!(
        (status, code.as_deref(), answered["gatewayArn"].as_str()),
        (200, None, Some(gateway_arn.as_str())),
        "once every instance is deregistered the gateway is deleted, got {status} {code:?}: \
         {answered}"
    );

    // The deregistered instance stays readable, as the terminal `DEREGISTERED` state says.
    let described = instance_of(&emulator, "after the gateway was deleted", &instance_arn).await;
    assert_eq!(
        described["instanceState"].as_str(),
        Some(mc::INSTANCE_STATE_DEREGISTERED),
        "a deregistered instance is still reported, got {described}"
    );
}
