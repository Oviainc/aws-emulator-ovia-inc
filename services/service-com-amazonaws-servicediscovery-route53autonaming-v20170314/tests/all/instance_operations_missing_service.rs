// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what the INSTANCE-level operations answer when the service
//! reference they were given resolves to no stored service.
//!
//! The rule under test: each of these operations refuses such a request with its OWN
//! modelled error identity and its own message template — the instance-scoped reads
//! report an instance miss, the instance mutations report a service miss — at the one
//! HTTP status this JSON front end serves every client-class refusal with, and the
//! variable part of each message is derived only from the request: the instance id the
//! caller named, or the service it named rendered as an ARN. The enclosing service's
//! absence is never surfaced as emulator prose, and never changes the operation's class.
//!
//! The table spans the four operations that take a `ServiceId` plus an `InstanceId` (or
//! an instance scope), each of the forms a member modelled `Arn` accepts ("the ID or
//! Amazon Resource Name (ARN)", so an id, an ARN, and the empty string `Arn`'s
//! `length 0..255` admits), and a set of instance ids spanning what `ResourceId`
//! (`length 0..64`, no pattern) admits — so no row is keyed to one recorded shape.
//!
//! Every request goes through `MultiServiceRouter::dispatch`, the same path the emulator
//! serves in production, reusing the harness in `cloud_map_core.rs`.

use serde_json::{json, Value};
use service_com_amazonaws_servicediscovery_route53autonaming_v20170314 as cloudmap;
use smithy_server_runtime::MultiServiceRouter;

use super::cloud_map_core::{call, emulator, namespace, ok, REGION};

/// The status this JSON front end answers every client-class refusal with: the class is
/// carried in the body's error code, not in the status line.
const CLIENT_ERROR_STATUS: u16 = 400;

/// The ARN form of a service reference, which is what the refusals that echo a service
/// carry however the request named it.
fn service_arn_of(reference: &str) -> String {
    if reference.starts_with("arn:") {
        return reference.to_string();
    }
    format!(
        "arn:aws:servicediscovery:{REGION}:{}:service/{reference}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// One refused call's status, wire error code and `Message`.
async fn refusal(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, String, Option<String>) {
    let (status, json) = call(router, operation, body).await;
    // The awsJson error envelope names the class in `__type`, qualified by namespace.
    let code = json["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    let message = json
        .get("Message")
        .or_else(|| json.get("message"))
        .and_then(Value::as_str)
        .map(str::to_string);
    (status, code, message)
}

/// Assert one row: the client-error status, the expected class, and the exact `Message`.
async fn assert_refusal(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: Value,
    expected_code: &str,
    expected_message: &str,
) {
    let (status, code, message) = refusal(router, operation, body.clone()).await;
    assert_eq!(
        (status, code.as_str()),
        (CLIENT_ERROR_STATUS, expected_code),
        "case {case:?} on {operation} with {body}: expected {CLIENT_ERROR_STATUS} \
         {expected_code}, got {status} {code} {message:?}"
    );
    assert_eq!(
        message.as_deref(),
        Some(expected_message),
        "case {case:?} on {operation} with {body}: expected the Message \
         {expected_message:?}, got {message:?}"
    );
}

/// The rows each instance-level operation is driven with for one (service reference,
/// instance id) pair: `(operation, request, expected class, expected Message)`.
fn rows(
    reference: &str,
    instance_id: &str,
    status: &str,
) -> Vec<(&'static str, Value, &'static str, String)> {
    let arn = service_arn_of(reference);
    vec![
        (
            // This read resolves the (service, instance) pair in one lookup, so a
            // service it cannot find has no instance to report either: the refusal
            // echoes the instance the request named, and nothing around it.
            "GetInstance",
            json!({ "ServiceId": reference, "InstanceId": instance_id }),
            "InstanceNotFound",
            instance_id.to_string(),
        ),
        (
            // The status read reports no instance either; its own template names the
            // service as a single-quoted ARN and the scope the request asked about —
            // the whole service when the request listed no `Instances`.
            "GetInstancesHealthStatus",
            json!({ "ServiceId": reference }),
            "InstanceNotFound",
            format!(
                "Instance(s) not found for service '{arn}' when trying to retreive: All instances"
            ),
        ),
        (
            // Same template, with the scope the request DID name.
            "GetInstancesHealthStatus",
            json!({ "ServiceId": reference, "Instances": [instance_id] }),
            "InstanceNotFound",
            format!(
                "Instance(s) not found for service '{arn}' when trying to retreive: {instance_id}"
            ),
        ),
        (
            // The instance registry's mutation reports the service miss, carrying the
            // service the request named as its ARN and nothing else.
            "DeregisterInstance",
            json!({ "ServiceId": reference, "InstanceId": instance_id }),
            "ServiceNotFound",
            arn.clone(),
        ),
        (
            // The custom-health submit reports the service miss with its own fixed
            // lower-case text and the single-quoted ARN.
            "UpdateInstanceCustomHealthStatus",
            json!({ "ServiceId": reference, "InstanceId": instance_id, "Status": status }),
            "ServiceNotFound",
            format!("no service '{arn}'"),
        ),
    ]
}

/// Every instance-level operation refuses an unresolvable service reference with its own
/// class and its own message template, at the client-error status, whatever form named
/// the service and whatever instance id (or instance scope) the request carried.
#[tokio::test]
async fn instance_level_operations_refuse_a_missing_service_with_their_own_bodies() {
    let router = emulator();
    // A namespace holding one service, so the store is not empty: the refusals below are
    // about the reference the request named, not about an empty registry.
    let (_, namespace_id) = namespace(&router, "instance-ops-ns").await;
    ok(
        &router,
        "CreateService",
        json!({ "Name": "instance-ops-svc", "NamespaceId": namespace_id }),
    )
    .await;

    // The forms a `ServiceId` member modelled `Arn` accepts, each resolving to no
    // stored service.
    let references: Vec<(&str, String)> = vec![
        ("an id no service carries", "srv-nosuchservice000".to_string()),
        (
            "an ARN no service carries",
            service_arn_of("srv-0000000000000009"),
        ),
        ("the empty reference", String::new()),
        (
            "a namespace id in the service slot",
            "ns-nosuchnamespace".to_string(),
        ),
        (
            "a reference at the modelled 255-character maximum",
            "s".repeat(255),
        ),
    ];

    // What `ResourceId` (`length 0..64`, no pattern) admits.
    let instance_ids: Vec<(&str, String)> = vec![
        ("a plain id", "i-9".to_string()),
        ("the empty id", String::new()),
        ("an id at the modelled 64-character maximum", "i".repeat(64)),
        (
            "an id carrying the punctuation the registry allows",
            "host.example_1/9:0@x-y".to_string(),
        ),
    ];

    // `CustomHealthStatus`'s values, from the model's enum, so the custom-health row's
    // refusal is shown not to depend on the status it submits.
    for status in cloudmap::ops::types::CUSTOM_HEALTH_STATUS_VALUES {
        for (reference_case, reference) in &references {
            for (instance_case, instance_id) in &instance_ids {
                let case = format!("{reference_case} with {instance_case}, Status {status}");
                for (operation, body, expected_code, expected_message) in
                    rows(reference, instance_id, status)
                {
                    assert_refusal(
                        &router,
                        &case,
                        operation,
                        body,
                        expected_code,
                        &expected_message,
                    )
                    .await;
                }
            }
        }
    }
}

/// The control for the table above: against a service that IS stored, every one of those
/// four operations serves the registered instance — so the refusals are the missing
/// reference's, not the operations'. A service whose health check is a custom one is used
/// because that is the only kind whose status a client may submit.
#[tokio::test]
async fn the_same_operations_serve_a_stored_service_and_its_instance() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "instance-ops-live-ns").await;
    let created = ok(
        &router,
        "CreateService",
        json!({
            "Name": "instance-ops-live-svc",
            "NamespaceId": namespace_id,
            "HealthCheckCustomConfig": { "FailureThreshold": 1 },
        }),
    )
    .await;
    let service_id = created["Service"]["Id"]
        .as_str()
        .expect("CreateService returns the service it stored")
        .to_string();
    ok(
        &router,
        "RegisterInstance",
        json!({
            "ServiceId": service_id,
            "InstanceId": "i-live",
            "Attributes": { "AWS_INSTANCE_IPV4": "10.0.0.1" },
        }),
    )
    .await;

    let read = ok(
        &router,
        "GetInstance",
        json!({ "ServiceId": service_id, "InstanceId": "i-live" }),
    )
    .await;
    assert_eq!(
        read["Instance"]["Id"],
        json!("i-live"),
        "case a stored service: GetInstance reports the registered instance, got {read}"
    );

    let health = ok(
        &router,
        "GetInstancesHealthStatus",
        json!({ "ServiceId": service_id }),
    )
    .await;
    assert!(
        health["Status"].get("i-live").is_some(),
        "case a stored service: the status read reports the registered instance, got {health}"
    );

    let submitted = ok(
        &router,
        "UpdateInstanceCustomHealthStatus",
        json!({ "ServiceId": service_id, "InstanceId": "i-live", "Status": "UNHEALTHY" }),
    )
    .await;
    assert_eq!(
        submitted,
        json!({}),
        "case a stored service: the custom-health submit is modelled with no output, got \
         {submitted}"
    );

    let deregistered = ok(
        &router,
        "DeregisterInstance",
        json!({ "ServiceId": service_id, "InstanceId": "i-live" }),
    )
    .await;
    assert!(
        deregistered["OperationId"].is_string(),
        "case a stored service: the deregister files an operation, got {deregistered}"
    );

    // The instance is gone, so the instance-scoped reads now report the instance miss —
    // the same class the missing-service table expects, with the stored service's ARN.
    let service_arn = service_arn_of(&service_id);
    assert_refusal(
        &router,
        "a stored service whose instance was deregistered",
        "GetInstance",
        json!({ "ServiceId": service_id, "InstanceId": "i-live" }),
        "InstanceNotFound",
        "i-live",
    )
    .await;
    assert_refusal(
        &router,
        "a stored service whose instance was deregistered",
        "GetInstancesHealthStatus",
        json!({ "ServiceId": service_id, "Instances": ["i-live"] }),
        "InstanceNotFound",
        &format!(
            "Instance(s) not found for service '{service_arn}' when trying to retreive: i-live"
        ),
    )
    .await;
}
