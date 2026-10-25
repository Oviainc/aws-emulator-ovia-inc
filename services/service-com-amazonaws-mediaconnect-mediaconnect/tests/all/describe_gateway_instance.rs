// SPDX-License-Identifier: CC0-1.0

//! DescribeGatewayInstance, end to end over HTTP: the reporting operation on a gateway
//! instance ("displays the details of an instance").
//!
//! The rule asserted here is the one the operation answers by: it reports the STORED
//! gateway-instance record the ARN in its path addresses, whatever value of the modelled
//! `InstanceState` that record carries (`smithy_query(query="shape",shape="InstanceState")`:
//! REGISTERING | ACTIVE | DEREGISTERING | DEREGISTERED | REGISTRATION_ERROR |
//! DEREGISTRATION_ERROR), and — being `@readonly`
//! (`smithy_query(query="operation",operation="DescribeGatewayInstance")`) — it changes
//! nothing a later caller sees, including a state change the clock has not made due yet.
//!
//! `tests/all/gateway_instances.rs` asserts the deregistration this reader is proved
//! through, its `RunningBridgeCount` and its not-found envelope; the tables here vary what
//! this operation's own rule generalises over: the `InstanceState` of the record (every
//! value of the enum), its `BridgePlacement` and `ConnectionStatus` (both values of each),
//! how many instances the store holds and across how many gateways, and how many times one
//! instance is read.
//!
//! An instance is never created by a client: the model declares no operation that registers
//! one, so the fixtures register through the store's own out-of-band channel
//! (`mc::ComAmazonawsMediaconnectMediaconnectState::register_gateway_instance`), exactly as
//! `tests/all/gateway_instances.rs` does.

use std::collections::BTreeSet;
use std::time::Instant;

use serde_json::Value;

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::gateway_instances::{
    describe_gateway_instance, gateway_with, instance_in_state, register_instance,
    PAST_THE_TRANSITION,
};

/// One stored record to report.
#[derive(Debug)]
struct Row {
    case: &'static str,
    /// `GatewayInstance$InstanceState`, as the record holds it.
    instance_state: &'static str,
    /// `GatewayInstance$BridgePlacement`, as the record holds it.
    bridge_placement: &'static str,
    /// `GatewayInstance$ConnectionStatus`, as the record holds it.
    connection_status: &'static str,
    /// The `ConnectionStatus` the response must carry: the stored one while the instance is
    /// still registered with its gateway, and DISCONNECTED once it has reached the terminal
    /// `DEREGISTERED` ("the connection state of the instance" — an instance that is no
    /// longer registered is no longer talking to the service).
    ///
    /// The model states only the two values of the enum, so the second half of that is this
    /// emulator's own derivation of the member rather than a modelled requirement
    /// (`mc::StoredGatewayInstance::connection_status`); it is asserted here so the two
    /// gateway-instance readers cannot drift apart over it.
    reported_connection_status: &'static str,
}

/// Every value of the modelled `InstanceState`, each paired with a `BridgePlacement` and a
/// `ConnectionStatus` so both values of both enums are reported too.
const ROWS: &[Row] = &[
    Row {
        case: "a registration still in flight, available and connected",
        instance_state: mc::INSTANCE_STATE_REGISTERING,
        bridge_placement: mc::BRIDGE_PLACEMENT_AVAILABLE,
        connection_status: mc::CONNECTION_STATUS_CONNECTED,
        reported_connection_status: mc::CONNECTION_STATUS_CONNECTED,
    },
    Row {
        case: "an active instance that can host new bridges",
        instance_state: mc::INSTANCE_STATE_ACTIVE,
        bridge_placement: mc::BRIDGE_PLACEMENT_AVAILABLE,
        connection_status: mc::CONNECTION_STATUS_CONNECTED,
        reported_connection_status: mc::CONNECTION_STATUS_CONNECTED,
    },
    Row {
        case: "an active instance locked against new bridges, appliance offline",
        instance_state: mc::INSTANCE_STATE_ACTIVE,
        bridge_placement: mc::BRIDGE_PLACEMENT_LOCKED,
        connection_status: mc::CONNECTION_STATUS_DISCONNECTED,
        reported_connection_status: mc::CONNECTION_STATUS_DISCONNECTED,
    },
    Row {
        case: "a deregistration still in flight",
        instance_state: mc::INSTANCE_STATE_DEREGISTERING,
        bridge_placement: mc::BRIDGE_PLACEMENT_LOCKED,
        connection_status: mc::CONNECTION_STATUS_CONNECTED,
        reported_connection_status: mc::CONNECTION_STATUS_CONNECTED,
    },
    Row {
        case: "an instance in the terminal DEREGISTERED state",
        instance_state: mc::INSTANCE_STATE_DEREGISTERED,
        bridge_placement: mc::BRIDGE_PLACEMENT_AVAILABLE,
        connection_status: mc::CONNECTION_STATUS_CONNECTED,
        reported_connection_status: mc::CONNECTION_STATUS_DISCONNECTED,
    },
    Row {
        case: "a deregistered instance whose appliance was already offline",
        instance_state: mc::INSTANCE_STATE_DEREGISTERED,
        bridge_placement: mc::BRIDGE_PLACEMENT_LOCKED,
        connection_status: mc::CONNECTION_STATUS_DISCONNECTED,
        reported_connection_status: mc::CONNECTION_STATUS_DISCONNECTED,
    },
    Row {
        case: "an instance whose registration failed",
        instance_state: mc::INSTANCE_STATE_REGISTRATION_ERROR,
        bridge_placement: mc::BRIDGE_PLACEMENT_AVAILABLE,
        connection_status: mc::CONNECTION_STATUS_DISCONNECTED,
        reported_connection_status: mc::CONNECTION_STATUS_DISCONNECTED,
    },
    Row {
        case: "an instance whose deregistration failed",
        instance_state: mc::INSTANCE_STATE_DEREGISTRATION_ERROR,
        bridge_placement: mc::BRIDGE_PLACEMENT_LOCKED,
        connection_status: mc::CONNECTION_STATUS_CONNECTED,
        reported_connection_status: mc::CONNECTION_STATUS_CONNECTED,
    },
];

/// Register one instance with `gateway_arn` and put its record in the state the row names.
///
/// The `InstanceState` is forced through the seam `gateway_instances` owns
/// ([`instance_in_state`], which registers, forces the state and clears the transition the
/// registration left pending); the other two members this table varies are reported out of
/// band too, so they are forced the same way on top of it.
fn instance_with(emulator: &Emulator, gateway_arn: &str, row: &Row) -> String {
    let instance_arn = instance_in_state(emulator, gateway_arn, row.instance_state);
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let mut store = state.lock();
    let instance = store
        .gateway_instances
        .get_mut(&instance_arn)
        .unwrap_or_else(|| panic!("case {:?}: the fixture instance is stored", row.case));
    instance.bridge_placement = row.bridge_placement.to_string();
    instance.connection_status = row.connection_status.to_string();
    instance_arn
}

/// The whole gateway-instance map as the store holds it, for comparing a before and an after:
/// a read must leave every record — the one it reported and every other — exactly as it was.
fn stored_instances(emulator: &Emulator) -> String {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = state.lock();
    format!("{:?}", store.gateway_instances)
}

/// The `InstanceId` the registration generated for `arn`, read from the store, so the
/// response is compared against the PERSISTED value rather than against a literal.
fn stored_instance_id(emulator: &Emulator, arn: &str) -> String {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = state.lock();
    store
        .gateway_instances
        .get(arn)
        .unwrap_or_else(|| panic!("the fixture instance {arn} is stored"))
        .instance_id
        .clone()
}

/// The `GatewayInstance` of a 200 response, asserted to be answered at all.
fn reported(case: &str, status: u16, code: Option<&str>, body: &Value) -> Value {
    assert_eq!(
        (status, code),
        (200, None),
        "case {case:?}: describe expected 200, got {status} {code:?}: {body}"
    );
    body["gatewayInstance"].clone()
}

/// The rule over the whole table: whatever state the stored record is in, the response is
/// that record — and the read leaves the store untouched.
#[tokio::test]
async fn an_instance_is_reported_from_its_stored_record_whatever_state_it_is_in() {
    let emulator = Emulator::new();
    let gateway_arn = gateway_with(&emulator, "gw-describe-instance").await;

    // One store holding every row at once, so a row that reported another row's record
    // would fail rather than pass unnoticed.
    let arns: Vec<String> = ROWS
        .iter()
        .map(|row| instance_with(&emulator, &gateway_arn, row))
        .collect();
    let before = stored_instances(&emulator);

    for (row, arn) in ROWS.iter().zip(&arns) {
        let case = row.case;
        let (status, code, body) = describe_gateway_instance(&emulator, arn).await;
        let described = reported(case, status, code.as_deref(), &body);
        let instance_id = stored_instance_id(&emulator, arn);
        assert_eq!(
            (
                described["gatewayInstanceArn"].as_str(),
                described["gatewayArn"].as_str(),
                described["instanceId"].as_str(),
                described["instanceState"].as_str(),
                described["bridgePlacement"].as_str(),
                described["connectionStatus"].as_str(),
                described["runningBridgeCount"].as_i64(),
            ),
            (
                Some(arn.as_str()),
                Some(gateway_arn.as_str()),
                Some(instance_id.as_str()),
                Some(row.instance_state),
                Some(row.bridge_placement),
                Some(row.reported_connection_status),
                Some(0),
            ),
            "case {case:?}: the response is the stored record, got {described}"
        );

        // A read is repeatable: a second and a third call answer the same document.
        for repeat in 2..=3 {
            let (status, code, body) = describe_gateway_instance(&emulator, arn).await;
            let again = reported(case, status, code.as_deref(), &body);
            assert_eq!(
                again, described,
                "case {case:?}: read {repeat} must answer what read 1 did; expected \
                 {described}, got {again}"
            );
        }
    }

    let after = stored_instances(&emulator);
    assert_eq!(
        after, before,
        "describing instances must store nothing; expected {before}, got {after}"
    );
}

/// Each ARN addresses its own record: the store holds instances of several gateways at once
/// and every describe answers the one its path names. The rows vary the number of instances
/// per gateway.
#[tokio::test]
async fn every_instance_is_addressed_by_its_own_arn() {
    for instances_per_gateway in [1_usize, 2, 5] {
        let emulator = Emulator::new();
        let case = format!("{instances_per_gateway} instances on each of two gateways");

        // (the instance's ARN, the ARN of the gateway it is registered with)
        let mut registered: Vec<(String, String)> = Vec::new();
        for gateway in 0..2 {
            let gateway_arn =
                gateway_with(&emulator, &format!("gw-{instances_per_gateway}-{gateway}")).await;
            for _ in 0..instances_per_gateway {
                let instance_arn = register_instance(&emulator, &gateway_arn);
                registered.push((instance_arn, gateway_arn.clone()));
            }
        }

        let mut reported_ids = BTreeSet::new();
        for (instance_arn, gateway_arn) in &registered {
            let (status, code, body) = describe_gateway_instance(&emulator, instance_arn).await;
            let described = reported(&case, status, code.as_deref(), &body);
            assert_eq!(
                (
                    described["gatewayInstanceArn"].as_str(),
                    described["gatewayArn"].as_str(),
                ),
                (Some(instance_arn.as_str()), Some(gateway_arn.as_str())),
                "case {case:?}: the response is the record {instance_arn} addresses, got \
                 {described}"
            );
            reported_ids.insert(
                described["instanceId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }

        assert_eq!(
            reported_ids.len(),
            registered.len(),
            "case {case:?}: each instance has its own `InstanceId`, got {reported_ids:?}"
        );
    }
}

/// A read does not advance the clock: an instance whose registration is still in flight is
/// reported in the non-terminal `REGISTERING` however many times it is read, and only the
/// transition span settles it into `ACTIVE`. The rows vary how many reads happen inside the
/// span.
#[tokio::test]
async fn reading_an_instance_does_not_settle_a_transition_before_it_is_due() {
    // Few enough reads that they all land well inside the span even on a loaded machine;
    // what the rows vary is that one read and several behave the same way.
    for reads_inside_the_span in [1_usize, 2] {
        let emulator = Emulator::new();
        let case = format!("{reads_inside_the_span} reads before the transition is due");
        let gateway_arn = gateway_with(&emulator, &format!("gw-poll-{reads_inside_the_span}")).await;
        let registered_at = Instant::now();
        let instance_arn = register_instance(&emulator, &gateway_arn);

        for read in 1..=reads_inside_the_span {
            let (status, code, body) = describe_gateway_instance(&emulator, &instance_arn).await;
            let described = reported(&case, status, code.as_deref(), &body);
            let elapsed = registered_at.elapsed();
            assert!(
                elapsed < mc::TRANSITION_SPAN,
                "case {case:?}: read {read} must happen inside the {:?} transition span, \
                 {elapsed:?} elapsed",
                mc::TRANSITION_SPAN
            );
            assert_eq!(
                described["instanceState"].as_str(),
                Some(mc::INSTANCE_STATE_REGISTERING),
                "case {case:?}: read {read} is inside the span, so the registration is still \
                 in flight, got {described}"
            );
        }

        tokio::time::sleep(PAST_THE_TRANSITION).await;
        let (status, code, body) = describe_gateway_instance(&emulator, &instance_arn).await;
        let described = reported(&case, status, code.as_deref(), &body);
        assert_eq!(
            described["instanceState"].as_str(),
            Some(mc::INSTANCE_STATE_ACTIVE),
            "case {case:?}: once the span has passed the registration is settled, got \
             {described}"
        );
    }
}
