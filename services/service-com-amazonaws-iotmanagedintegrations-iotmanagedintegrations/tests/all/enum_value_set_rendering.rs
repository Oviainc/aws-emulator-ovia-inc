// SPDX-License-Identifier: CC0-1.0

//! What the `ValidationException` for an out-of-set enum member states about the value
//! set, over the real restJson1 request path (`MultiServiceRouter::dispatch`).
//!
//! RULE: when a request carries a value outside the value set an enum-typed member
//! declares, the refusal's requirement clause enumerates EXACTLY that member's declared
//! value set — every declared value once, nothing undeclared, rendered `[a, b, c]` — and
//! that set depends on the member's enum alone: not on which operation or which binding
//! (body, URI label, query string) carried the member, not on how deeply nested the
//! member is, not on the rejected value, and not on how many times the call is made.
//!
//! The ORDER inside the brackets is NOT part of the contract, so every assertion here
//! compares the stated set as a SET. A Smithy enum's value set is a set: the model fixes
//! no listing order, AWS documents none, and the two orders recorded from this service
//! are not produced by any rule over the model —
//! `AuthMaterialType`, declared `[CUSTOM_PROTOCOL_QR_BAR_CODE, WIFI_SETUP_QR_BAR_CODE,
//! ZWAVE_QR_BAR_CODE, ZIGBEE_QR_BAR_CODE, DISCOVERED_DEVICE, PRE_ONBOARDED_CLOUD]`
//! (`smithy_query(query="shape",shape="AuthMaterialType")`), was recorded as
//! `[PRE_ONBOARDED_CLOUD, ZIGBEE_QR_BAR_CODE, ZWAVE_QR_BAR_CODE, WIFI_SETUP_QR_BAR_CODE,
//! DISCOVERED_DEVICE, CUSTOM_PROTOCOL_QR_BAR_CODE]`, which is neither the declaration
//! order, nor its reverse (that lists `DISCOVERED_DEVICE` second, the recording lists it
//! fifth), nor sorted either way. See `src/service.rs`'s module docs.
//!
//! The rows below span the class the rule generalises over: fourteen enum-typed members
//! on nine operations, with set sizes from one value to eleven, values in SCREAMING_SNAKE
//! and in lowercase, members bound to the body, to a URI label and to the query string, a
//! member nested inside a structure, and operations that are implemented as well as ones
//! still scaffolded. Two rows (`authenticationMaterialType` on `CreateManagedThing` and
//! `provisioningType` on `CreateProvisioningProfile`) are the recorded refusals the rule
//! was derived from; the other twelve are enums no recording covers.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::state::{
    AUTH_MATERIAL_TYPE_VALUES, HUB_NETWORK_MODE_VALUES, ROLE_VALUES,
};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's requests are signed for.
const REGION: &str = "us-east-1";

/// A value no enum of this service declares, used wherever a row only needs the member to
/// be out of its set.
const OUTSIDE_EVERY_VALUE_SET: &str = "NOT_A_REAL_VALUE";

// The value sets the model declares, for the enums the crate does not already export a
// constant for. Each is quoted from
// `smithy_query(query="shape",shape="<the enum>",model_path="services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/smithy-model/com-amazonaws-iotmanagedintegrations-iotmanagedintegrations.json")`
// in the model's declaration order; the assertions compare them as sets, because the
// listing order is not part of the contract.

/// `smithy_query` -> `["FLEET_PROVISIONING", "JITR"]`.
const PROVISIONING_TYPE_VALUES: &[&str] = &["FLEET_PROVISIONING", "JITR"];
/// `smithy_query` -> `["MANAGED_INTEGRATIONS_DEFAULT_ENCRYPTION", "CUSTOMER_KEY_ENCRYPTION"]`.
const ENCRYPTION_TYPE_VALUES: &[&str] =
    &["MANAGED_INTEGRATIONS_DEFAULT_ENCRYPTION", "CUSTOMER_KEY_ENCRYPTION"];
/// `smithy_query` -> `["ZWAVE", "ZIGBEE", "CLOUD", "CUSTOM", "CONTROLLER_CAPABILITY_REDISCOVERY"]`.
const DISCOVERY_TYPE_VALUES: &[&str] =
    &["ZWAVE", "ZIGBEE", "CLOUD", "CUSTOM", "CONTROLLER_CAPABILITY_REDISCOVERY"];
/// `smithy_query` -> `["ZWAVE_INSTALL_CODE"]` — a one-value set.
const DISCOVERY_AUTH_MATERIAL_TYPE_VALUES: &[&str] = &["ZWAVE_INSTALL_CODE"];
/// `smithy_query` -> `["ZWAVE", "ZIGBEE", "CUSTOM"]`.
const PROTOCOL_TYPE_VALUES: &[&str] = &["ZWAVE", "ZIGBEE", "CUSTOM"];
/// `smithy_query` -> the eleven values below (`CONNECTOR_ASSOCIATION` is `@deprecated`,
/// which does not take it out of the set).
const EVENT_TYPE_VALUES: &[&str] = &[
    "DEVICE_COMMAND",
    "DEVICE_COMMAND_REQUEST",
    "DEVICE_DISCOVERY_STATUS",
    "DEVICE_EVENT",
    "DEVICE_LIFE_CYCLE",
    "DEVICE_STATE",
    "DEVICE_OTA",
    "DEVICE_WSS",
    "CONNECTOR_ASSOCIATION",
    "ACCOUNT_ASSOCIATION",
    "CONNECTOR_ERROR_REPORT",
];
/// `smithy_query` -> `["capability", "definition"]` — values that are not SCREAMING_SNAKE.
const SCHEMA_VERSION_TYPE_VALUES: &[&str] = &["capability", "definition"];
/// `smithy_query` -> `["AWS", "ZCL", "CONNECTOR"]`.
const SCHEMA_VERSION_FORMAT_VALUES: &[&str] = &["AWS", "ZCL", "CONNECTOR"];
/// `smithy_query` -> `["STOP_ROLLOUT", "CANCEL", "FORCE_CANCEL"]`.
const SCHEDULING_CONFIG_END_BEHAVIOR_VALUES: &[&str] = &["STOP_ROLLOUT", "CANCEL", "FORCE_CANCEL"];
/// `smithy_query` -> `["ONE_TIME", "CONTINUOUS"]`.
const OTA_TYPE_VALUES: &[&str] = &["ONE_TIME", "CONTINUOUS"];
/// `smithy_query` -> `["HTTP"]` — a second one-value set, on a different operation.
const OTA_PROTOCOL_VALUES: &[&str] = &["HTTP"];

/// One HTTP answer, reduced to what the assertions read.
struct Reply {
    status: u16,
    /// The restJson1 error code, from `x-amzn-errortype`.
    error_type: Option<String>,
    body: Value,
}

/// A router holding one `iotmanagedintegrations` service over a fresh shared engine: the
/// same assembly `core/router` performs, with one service mounted. Mirrors the harness
/// the sibling suites in this crate use.
fn emulator() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here, so requests are unauthenticated.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::new(engine::InMemoryEngine::new())));
    router
}

/// Send one request and reduce the answer. The `Authorization` credential scope is what
/// routes the request to this service.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Reply {
    let payload = body.map_or_else(bytes::Bytes::new, |value| {
        bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize the request body"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://iotmanagedintegrations.{REGION}.amazonaws.com{path_and_query}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{REGION}/\
                 iotmanagedintegrations/aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply {
        status,
        error_type,
        body,
    }
}

/// The refusal prose, from the member the model names `Message`.
fn refusal_message(reply: &Reply, label: &str) -> String {
    reply
        .body
        .get("Message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {label:?}: expected the modelled Message member to carry the refusal, got \
                 status {} with body {:?}",
                reply.status, reply.body
            )
        })
        .to_string()
}

/// The value set the refusal states for `member_path`: the text between the `[` that opens
/// that member's enum clause and the `]` that closes it, split on the `", "` the clause
/// joins with. `None` when the message carries no enum clause for that member.
fn stated_value_set(message: &str, member_path: &str) -> Option<Vec<String>> {
    let opening = format!(
        "at '{member_path}' failed to satisfy constraint: Member must satisfy enum value set: ["
    );
    let start = message.find(&opening)? + opening.len();
    let rest = &message[start..];
    let end = rest.find(']')?;
    Some(rest[..end].split(", ").map(str::to_string).collect())
}

/// `values` sorted, so two value sets can be compared as sets.
fn as_set(values: &[String]) -> Vec<String> {
    let mut sorted = values.to_vec();
    sorted.sort();
    sorted
}

/// One refused request: what is sent, and which enum's value set the refusal must state.
struct Row {
    /// The case label every assertion names.
    label: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The member path the clause names, in lowerCamel as the wire names it.
    member_path: &'static str,
    /// The value set the model declares for that member's enum.
    declared: &'static [&'static str],
}

/// Assert the whole rule for one row's answer, and hand back the set it stated.
fn assert_states_the_declared_set(reply: &Reply, label: &str, row_path: &str, declared: &[&str]) {
    assert_eq!(
        reply.status, 400,
        "case {label:?}: expected HTTP 400, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("ValidationException"),
        "case {label:?}: expected the modelled ValidationException code, got {:?}",
        reply.error_type
    );
    let message = refusal_message(reply, label);
    let stated = stated_value_set(&message, row_path).unwrap_or_else(|| {
        panic!(
            "case {label:?}: expected an enum value-set clause for {row_path:?}, got {message:?}"
        )
    });

    // Exactly the declared set: the same number of values (so nothing is listed twice and
    // nothing undeclared is listed) and the same values (compared as a set, because the
    // listing order is not part of the contract).
    assert_eq!(
        stated.len(),
        declared.len(),
        "case {label:?}: expected {} values in the stated set, got {} ({stated:?})",
        declared.len(),
        stated.len()
    );
    let declared_set = as_set(&declared.iter().map(|value| (*value).to_string()).collect::<Vec<_>>());
    assert_eq!(
        as_set(&stated),
        declared_set,
        "case {label:?}: the stated set must be exactly the declared value set {declared_set:?}, \
         got {stated:?}"
    );
    // The rendering itself: `[a, b, c]`, so no element is empty or carries the separator's
    // whitespace.
    for value in &stated {
        assert!(
            !value.is_empty() && value.trim() == value.as_str(),
            "case {label:?}: {value:?} is not a cleanly rendered value in {stated:?}"
        );
    }
}

/// RULE (see the module docs): an out-of-set enum member is refused with exactly the value
/// set the model declares for it, whichever operation, binding or nesting carried it.
#[tokio::test]
async fn an_enum_refusal_states_exactly_the_declared_value_set() {
    let router = emulator();

    let rows = vec![
        // The recorded refusal this issue was filed about, on an implemented operation.
        // The declared set is read from the crate's own exported constant, so the test
        // fails if that constant and the model's enum ever disagree.
        Row {
            label: "CreateManagedThing/authenticationMaterialType (recorded)",
            method: "POST",
            path: "/managed-things".to_string(),
            body: Some(json!({
                "Role": "DEVICE",
                "AuthenticationMaterialType": OUTSIDE_EVERY_VALUE_SET,
                "AuthenticationMaterial": "qr:0000000000",
            })),
            member_path: "authenticationMaterialType",
            declared: AUTH_MATERIAL_TYPE_VALUES,
        },
        // The second recorded refusal, on an operation that is still a scaffold.
        Row {
            label: "CreateProvisioningProfile/provisioningType (recorded)",
            method: "POST",
            path: "/provisioning-profiles".to_string(),
            body: Some(json!({"ProvisioningType": OUTSIDE_EVERY_VALUE_SET})),
            member_path: "provisioningType",
            declared: PROVISIONING_TYPE_VALUES,
        },
        // A second enum on the same operation as the first row.
        Row {
            label: "CreateManagedThing/role",
            method: "POST",
            path: "/managed-things".to_string(),
            body: Some(json!({
                "Role": OUTSIDE_EVERY_VALUE_SET,
                "AuthenticationMaterialType": "ZWAVE_QR_BAR_CODE",
                "AuthenticationMaterial": "qr:0000000000",
            })),
            member_path: "role",
            declared: ROLE_VALUES,
        },
        // An enum on a mutation reached through a URI label.
        Row {
            label: "UpdateManagedThing/hubNetworkMode",
            method: "PUT",
            path: "/managed-things/anythingatall".to_string(),
            body: Some(json!({"HubNetworkMode": OUTSIDE_EVERY_VALUE_SET})),
            member_path: "hubNetworkMode",
            declared: HUB_NETWORK_MODE_VALUES,
        },
        // A member the model declares in lowerCamel already, on an account-level put.
        Row {
            label: "PutDefaultEncryptionConfiguration/encryptionType",
            method: "POST",
            path: "/configuration/account/encryption".to_string(),
            body: Some(json!({"encryptionType": OUTSIDE_EVERY_VALUE_SET})),
            member_path: "encryptionType",
            declared: ENCRYPTION_TYPE_VALUES,
        },
        // A five-value set.
        Row {
            label: "StartDeviceDiscovery/discoveryType",
            method: "POST",
            path: "/device-discoveries".to_string(),
            body: Some(json!({"DiscoveryType": OUTSIDE_EVERY_VALUE_SET})),
            member_path: "discoveryType",
            declared: DISCOVERY_TYPE_VALUES,
        },
        // A ONE-value set: the smallest set there is still renders as a list.
        Row {
            label: "StartDeviceDiscovery/authenticationMaterialType (a one-value set)",
            method: "POST",
            path: "/device-discoveries".to_string(),
            body: Some(json!({
                "DiscoveryType": "ZWAVE",
                "AuthenticationMaterialType": OUTSIDE_EVERY_VALUE_SET,
            })),
            member_path: "authenticationMaterialType",
            declared: DISCOVERY_AUTH_MATERIAL_TYPE_VALUES,
        },
        // A different enum on a member named `Protocol`: the set comes from the member's
        // enum, not from the member's name (compare the `CreateOtaTask/protocol` row).
        Row {
            label: "StartDeviceDiscovery/protocol",
            method: "POST",
            path: "/device-discoveries".to_string(),
            body: Some(json!({
                "DiscoveryType": "ZWAVE",
                "Protocol": OUTSIDE_EVERY_VALUE_SET,
            })),
            member_path: "protocol",
            declared: PROTOCOL_TYPE_VALUES,
        },
        // The largest set this service declares on an input member, in the body.
        Row {
            label: "CreateNotificationConfiguration/eventType",
            method: "POST",
            path: "/notification-configurations".to_string(),
            body: Some(json!({
                "EventType": OUTSIDE_EVERY_VALUE_SET,
                "DestinationName": "destination-one",
            })),
            member_path: "eventType",
            declared: EVENT_TYPE_VALUES,
        },
        // The SAME enum as the row above, on another operation and bound to a URI label
        // instead of the body: one enum, one set.
        Row {
            label: "UpdateNotificationConfiguration/eventType (the same enum, a URI label)",
            method: "PUT",
            path: format!("/notification-configurations/{OUTSIDE_EVERY_VALUE_SET}"),
            body: Some(json!({"DestinationName": "destination-one"})),
            member_path: "eventType",
            declared: EVENT_TYPE_VALUES,
        },
        // Values that are not SCREAMING_SNAKE, on a read, bound to a URI label.
        Row {
            label: "GetSchemaVersion/type (lowercase values, a URI label)",
            method: "GET",
            path: format!("/schema-versions/{OUTSIDE_EVERY_VALUE_SET}/switch@1.0"),
            body: None,
            member_path: "type",
            declared: SCHEMA_VERSION_TYPE_VALUES,
        },
        // An enum bound to the QUERY STRING of the same read.
        Row {
            label: "GetSchemaVersion/format (a query-string member)",
            method: "GET",
            path: format!("/schema-versions/capability/switch@1.0?Format={OUTSIDE_EVERY_VALUE_SET}"),
            body: None,
            member_path: "format",
            declared: SCHEMA_VERSION_FORMAT_VALUES,
        },
        // An enum NESTED inside a structure member: the clause names the whole path and
        // still states that member's own set.
        Row {
            label: "CreateOtaTask/otaSchedulingConfig.endBehavior (nested)",
            method: "POST",
            path: "/ota-tasks".to_string(),
            body: Some(json!({
                "S3Url": "s3://a-bucket/firmware.bin",
                "OtaType": "ONE_TIME",
                "OtaSchedulingConfig": {"EndBehavior": OUTSIDE_EVERY_VALUE_SET},
            })),
            member_path: "otaSchedulingConfig.endBehavior",
            declared: SCHEDULING_CONFIG_END_BEHAVIOR_VALUES,
        },
        // A second one-value set, on a member named `Protocol` whose enum is a different
        // one from `StartDeviceDiscovery`'s.
        Row {
            label: "CreateOtaTask/protocol (a one-value set)",
            method: "POST",
            path: "/ota-tasks".to_string(),
            body: Some(json!({
                "S3Url": "s3://a-bucket/firmware.bin",
                "OtaType": "ONE_TIME",
                "Protocol": OUTSIDE_EVERY_VALUE_SET,
            })),
            member_path: "protocol",
            declared: OTA_PROTOCOL_VALUES,
        },
        Row {
            label: "CreateOtaTask/otaType",
            method: "POST",
            path: "/ota-tasks".to_string(),
            body: Some(json!({
                "S3Url": "s3://a-bucket/firmware.bin",
                "OtaType": OUTSIDE_EVERY_VALUE_SET,
            })),
            member_path: "otaType",
            declared: OTA_TYPE_VALUES,
        },
    ];

    for row in &rows {
        let reply = call(&router, row.method, &row.path, row.body.clone()).await;
        assert_states_the_declared_set(&reply, row.label, row.member_path, row.declared);
    }
}

/// RULE (invariant half, see the module docs): the stated value set is a property of the
/// member's enum alone — it does not vary with the value the request sent, nor between two
/// identical calls, and it never publishes the rejected value.
#[tokio::test]
async fn the_stated_value_set_does_not_vary_with_the_rejected_value() {
    let router = emulator();

    // A long value (past the 256 characters the framework echoes before eliding), a
    // declared value in the wrong case (enum values are case-sensitive), a declared value
    // with trailing whitespace, a value carrying the character the set is bracketed with,
    // and the empty string: five rejected values spanning what a client can send.
    let long_value = "Z".repeat(300);
    let rejected_values = [
        OUTSIDE_EVERY_VALUE_SET.to_string(),
        "zwave_qr_bar_code".to_string(),
        "ZWAVE_QR_BAR_CODE ".to_string(),
        "BRACKET]IN]VALUE".to_string(),
        long_value.clone(),
        String::new(),
    ];

    let mut seen: Option<Vec<String>> = None;
    for rejected in &rejected_values {
        let label = format!("CreateManagedThing/authenticationMaterialType = {rejected:?}");
        let body = json!({
            "Role": "DEVICE",
            "AuthenticationMaterialType": rejected,
            "AuthenticationMaterial": "qr:0000000000",
        });
        let reply = call(&router, "POST", "/managed-things", Some(body.clone())).await;
        assert_states_the_declared_set(
            &reply,
            &label,
            "authenticationMaterialType",
            AUTH_MATERIAL_TYPE_VALUES,
        );

        let message = refusal_message(&reply, &label);
        // The refusal states the member and its set, never what was sent — the empty
        // string is no value to withhold, so it is the one row with nothing to check.
        if !rejected.is_empty() {
            assert!(
                !message.contains(rejected.as_str()),
                "case {label:?}: the refusal must not quote the rejected value back, got \
                 {message:?}"
            );
        }

        // Every rejected value gets the same rendering, byte for byte.
        let stated = stated_value_set(&message, "authenticationMaterialType")
            .expect("asserted present just above");
        match &seen {
            None => seen = Some(stated),
            Some(first) => assert_eq!(
                &stated, first,
                "case {label:?}: the stated set must not vary with the rejected value, got \
                 {stated:?} after {first:?}"
            ),
        }

        // And the same request twice gets the same answer: the refusal reads nothing that
        // a call changes.
        let again = call(&router, "POST", "/managed-things", Some(body)).await;
        assert_eq!(
            refusal_message(&again, &label),
            message,
            "case {label:?}: two identical calls must be refused identically"
        );
    }
}
