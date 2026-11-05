// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for ONE rule about how this service REFUSES a client request.
//!
//! The rule under test: a client refusal of this service answers HTTP 400 — whatever status
//! the modelled error shape's `@httpError` declares — states in `ErrorCode` the condition
//! that was refused (the KIND of resource that was not found, or what made the request
//! invalid), and words it in the FIXED sentence AWS states for that condition, which
//! carries no part of the request: no ARN, no host, no name, no identifier the caller sent.
//! Which modelled shape carries it is the condition's too — an ARN that resolves to no
//! resource is a not-found condition wherever it is resolved — and the condition is a fact
//! of the STORE, so the very same call succeeds once the state it needed is there.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter`,
//! reusing the harness of `gateway_lifecycle`, so what is asserted is the wire behaviour.
//!
//! The table below spans the class: every condition this service refuses a request for a
//! resource it cannot resolve with — a missing gateway, a missing hypervisor, a hypervisor
//! that carries no property mappings, a missing virtual machine, a request whose parameters
//! the service will not act on, and a request only a gateway can carry out for a hypervisor
//! no gateway serves — each asked for through the operations that reach it, and each with
//! ARNs that differ in how they came to name nothing and in the resource type they spell.
//! The sibling `missing_resource_refusal_is_one_answer` suite spans the other axis, one
//! condition across every operation that resolves it.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{
    GATEWAY_NOT_FOUND, GATEWAY_NOT_FOUND_MESSAGE, GATEWAY_OUT_OF_DATE, GATEWAY_OUT_OF_DATE_MESSAGE,
    HYPERVISOR_NOT_FOUND, HYPERVISOR_NOT_FOUND_MESSAGE, INVALID_PARAMETER,
    INVALID_PARAMETER_MESSAGE, MAPPINGS_NOT_FOUND, MAPPINGS_NOT_FOUND_MESSAGE,
    VIRTUAL_MACHINE_NOT_FOUND, VIRTUAL_MACHINE_NOT_FOUND_MESSAGE,
};

use crate::gateway_lifecycle::{
    build_engine, build_router, call, create, never_created_arn, REGION,
};
use crate::gateway_server_association::{arn_of, associate, import, message_of};
use crate::hypervisor_property_mappings::{expected_mappings, get_mappings, mapping};
use crate::metadata_sync_over_the_wire::{role_arn, syncable_hypervisor};
use crate::metadata_sync_restart_supersedes_the_pending_sync::start_sync_at;

/// A `Host` the modelled `@length(min: 3, max: 128)` accepts.
const HOST: &str = "198.51.100.90";

/// One refusal reduced to the four parts this rule is about.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Refusal {
    status: u16,
    /// The awsJson error discriminator, namespace stripped.
    shape: &'static str,
    error_code: &'static str,
    message: &'static str,
}

/// The modelled `ResourceNotFoundException`, as this service answers it.
fn not_found(error_code: &'static str, message: &'static str) -> Refusal {
    Refusal {
        status: 400,
        shape: "ResourceNotFoundException",
        error_code,
        message,
    }
}

/// The common `ValidationException`, as this service answers it.
fn invalid(error_code: &'static str, message: &'static str) -> Refusal {
    Refusal {
        status: 400,
        shape: "ValidationException",
        error_code,
        message,
    }
}

/// What the service really answered `operation` with, compared against the row's `Refusal`.
async fn assert_refused(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    expected: &Refusal,
) {
    let reply = call(router, operation, body).await;
    let observed = (
        reply.status,
        reply.code.as_deref(),
        reply.str_at("/ErrorCode"),
        message_of(&reply),
    );
    assert_eq!(
        observed,
        (
            expected.status,
            Some(expected.shape),
            Some(expected.error_code),
            expected.message.to_string(),
        ),
        "case {case:?}: {operation} must be refused with {expected:?}, got {:?}",
        reply.body
    );
    // The sentence is AWS's fixed text for the condition, so nothing the request carried
    // may appear in it — an ARN, a host or a name is a request value, not part of the
    // template.
    let message = message_of(&reply);
    for value in request_values(body) {
        assert!(
            !message.contains(&value),
            "case {case:?}: {operation} must not echo the request value {value:?} in its \
             refusal, got {message:?}"
        );
    }
}

/// Every string the request body carries, so a refusal can be checked for echoing one.
fn request_values(body: &Value) -> Vec<String> {
    match body {
        Value::String(text) => vec![text.clone()],
        Value::Array(items) => items.iter().flat_map(request_values).collect(),
        Value::Object(members) => members.values().flat_map(request_values).collect(),
        _ => Vec::new(),
    }
}

/// The request that puts one set of property mappings on `arn`.
fn put_mappings_body(arn: &str, role: &str) -> Value {
    json!({
        "HypervisorArn": arn,
        "IamRoleArn": role,
        "VmwareToAwsTagMappings": expected_mappings(&[mapping(
            "department",
            "finance",
            "Department",
            "Finance",
        )]),
    })
}

/// RULE: every condition is answered with the status, the `ErrorCode` and the fixed
/// sentence that belong to THAT condition, whichever operation and whichever ARN reached
/// it.
#[tokio::test]
async fn every_refusal_states_its_own_condition() {
    let engine = build_engine();
    let router = build_router(&engine);

    // The scene: a gateway and a hypervisor that really exist (so a refusal is about what
    // the request named and not about an empty store), and a hypervisor no gateway serves.
    let live_gateway = create(&router, "gw-live").await;
    let unserved = import(&router, "hv-unserved", HOST).await;
    let role = role_arn("aws", "refusals");

    // (case, operation, request body, the refusal it must take). The rows differ in the
    // condition, in the operation that reaches it, and in how the ARN came to name nothing
    // (never created, a resource type this service names for something else, another
    // partition).
    let rows: Vec<(String, &str, Value, Refusal)> = vec![
        (
            "a gateway that was never created".to_string(),
            "DisassociateGatewayFromServer",
            json!({ "GatewayArn": never_created_arn() }),
            not_found(GATEWAY_NOT_FOUND, GATEWAY_NOT_FOUND_MESSAGE),
        ),
        (
            "a GatewayArn naming a hypervisor resource".to_string(),
            "DisassociateGatewayFromServer",
            json!({ "GatewayArn": unserved.clone() }),
            not_found(GATEWAY_NOT_FOUND, GATEWAY_NOT_FOUND_MESSAGE),
        ),
        (
            "an association whose gateway names nothing".to_string(),
            "AssociateGatewayToServer",
            json!({ "GatewayArn": never_created_arn(), "ServerArn": unserved.clone() }),
            not_found(GATEWAY_NOT_FOUND, GATEWAY_NOT_FOUND_MESSAGE),
        ),
        (
            "an association whose server names nothing".to_string(),
            "AssociateGatewayToServer",
            json!({
                "GatewayArn": live_gateway.clone(),
                "ServerArn": arn_of("hypervisor", "hv-1111111111nonexistent"),
            }),
            not_found(HYPERVISOR_NOT_FOUND, HYPERVISOR_NOT_FOUND_MESSAGE),
        ),
        (
            "a connection test whose gateway names nothing".to_string(),
            "TestHypervisorConfiguration",
            json!({ "GatewayArn": never_created_arn(), "Host": HOST }),
            invalid(INVALID_PARAMETER, INVALID_PARAMETER_MESSAGE),
        ),
        (
            "a connection test naming a gateway of another partition".to_string(),
            "TestHypervisorConfiguration",
            json!({
                "GatewayArn": format!(
                    "arn:aws-cn:backup-gateway:{REGION}:{}:gateway/bgw-0000000000000888",
                    engine::EMULATOR_ACCOUNT_ID
                ),
                "Host": HOST,
            }),
            invalid(INVALID_PARAMETER, INVALID_PARAMETER_MESSAGE),
        ),
        (
            "a connection test of a host no record carries".to_string(),
            "TestHypervisorConfiguration",
            json!({ "GatewayArn": live_gateway.clone(), "Host": "203.0.113.77" }),
            not_found(HYPERVISOR_NOT_FOUND, HYPERVISOR_NOT_FOUND_MESSAGE),
        ),
        (
            "a mappings read of a hypervisor that carries none".to_string(),
            "GetHypervisorPropertyMappings",
            json!({ "HypervisorArn": unserved.clone() }),
            not_found(MAPPINGS_NOT_FOUND, MAPPINGS_NOT_FOUND_MESSAGE),
        ),
        (
            "a mappings read of an ARN naming no hypervisor".to_string(),
            "GetHypervisorPropertyMappings",
            json!({ "HypervisorArn": arn_of("hypervisor", "hv-2222222222nonexistent") }),
            not_found(HYPERVISOR_NOT_FOUND, HYPERVISOR_NOT_FOUND_MESSAGE),
        ),
        (
            "a mappings put for a hypervisor no gateway serves".to_string(),
            "PutHypervisorPropertyMappings",
            put_mappings_body(&unserved, &role),
            invalid(GATEWAY_OUT_OF_DATE, GATEWAY_OUT_OF_DATE_MESSAGE),
        ),
        (
            "a machine ARN no sync ever reported".to_string(),
            "GetVirtualMachine",
            json!({ "ResourceArn": arn_of("vm", "vm-1111111111111111") }),
            not_found(
                VIRTUAL_MACHINE_NOT_FOUND,
                VIRTUAL_MACHINE_NOT_FOUND_MESSAGE,
            ),
        ),
        (
            "a machine read naming a gateway that exists".to_string(),
            "GetVirtualMachine",
            json!({ "ResourceArn": live_gateway.clone() }),
            not_found(
                VIRTUAL_MACHINE_NOT_FOUND,
                VIRTUAL_MACHINE_NOT_FOUND_MESSAGE,
            ),
        ),
    ];

    for (case, operation, body, expected) in &rows {
        assert_refused(&router, case, operation, body, expected).await;
    }
}

/// The counterpart: each refusal above is a fact of the STORE and not of the request, so
/// the very same call succeeds once the state it needed exists — a refusal this service
/// states is never the only answer an operation has.
#[tokio::test]
async fn the_same_call_succeeds_once_the_state_it_needed_exists() {
    let engine = build_engine();
    let router = build_router(&engine);

    // A gateway associated with a hypervisor whose host this service holds: the scene every
    // refusal of the suite above was missing.
    let hypervisor = import(&router, "hv-served", HOST).await;
    let gateway = create(&router, "gw-served").await;
    let associated = associate(&router, &gateway, &hypervisor).await;
    assert_eq!(
        associated.status, 200,
        "the association must answer 200, got {} {:?}",
        associated.status, associated.body
    );

    // The connection test the unresolvable gateway was refused for.
    let tested = call(
        &router,
        "TestHypervisorConfiguration",
        &json!({ "GatewayArn": gateway.clone(), "Host": HOST }),
    )
    .await;
    assert_eq!(
        tested.status, 200,
        "case connection test: a gateway this store holds must pass, got {} {:?}",
        tested.status, tested.body
    );

    // The mappings put the unserved hypervisor was refused for, and the read that was
    // refused as missing mappings before it.
    let role = role_arn("aws", "accepted");
    let put = call(
        &router,
        "PutHypervisorPropertyMappings",
        &put_mappings_body(&hypervisor, &role),
    )
    .await;
    assert_eq!(
        (put.status, put.str_at("/HypervisorArn")),
        (200, Some(hypervisor.as_str())),
        "case mappings put: a served hypervisor must take the mappings, got {} {:?}",
        put.status,
        put.body
    );
    let read = get_mappings(&router, &hypervisor).await;
    assert_eq!(
        (read.status, read.str_at("/IamRoleArn")),
        (200, Some(role.as_str())),
        "case mappings read: the mappings just written must read back, got {} {:?}",
        read.status,
        read.body
    );

    // The disassociation the unresolvable gateway was refused for.
    let cleared = call(
        &router,
        "DisassociateGatewayFromServer",
        &json!({ "GatewayArn": gateway.clone() }),
    )
    .await;
    assert_eq!(
        (cleared.status, cleared.str_at("/GatewayArn")),
        (200, Some(gateway.as_str())),
        "case disassociation: a stored gateway must be disassociated, got {} {:?}",
        cleared.status,
        cleared.body
    );

    // And the machine read: a machine a succeeded sync discovered reads back, where an ARN
    // no sync ever reported is refused as a missing machine.
    let machine = synced_machine(&engine, &router).await;
    let found = call(
        &router,
        "GetVirtualMachine",
        &json!({ "ResourceArn": machine.clone() }),
    )
    .await;
    assert_eq!(
        (found.status, found.str_at("/VirtualMachine/ResourceArn")),
        (200, Some(machine.as_str())),
        "case machine read: a machine the service reports must read back, got {} {:?}",
        found.status,
        found.body
    );
}

/// The ARN of one virtual machine this service reports: a hypervisor a sync can run on,
/// with a sync started far enough in the past that its inventory has already fallen due.
async fn synced_machine(engine: &Arc<engine::InMemoryEngine>, router: &MultiServiceRouter) -> String {
    let hypervisor = syncable_hypervisor(router, "hv-synced", "198.51.100.91").await;
    start_sync_at(
        engine,
        &hypervisor,
        SystemTime::now() - Duration::from_secs(3_600),
    );
    let listed = call(
        router,
        "ListVirtualMachines",
        &json!({ "HypervisorArn": hypervisor }),
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "ListVirtualMachines must answer 200, got {} {:?}",
        listed.status, listed.body
    );
    let arn = listed
        .body
        .pointer("/VirtualMachines/0/ResourceArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "a sync that has fallen due must report an inventory, got {:?}",
                listed.body
            )
        });
    arn.to_string()
}
