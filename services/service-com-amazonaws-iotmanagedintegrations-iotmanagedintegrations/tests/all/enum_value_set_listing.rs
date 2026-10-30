// SPDX-License-Identifier: CC0-1.0

//! The ORDER an enum-constraint refusal lists an enum's admitted values in, driven
//! end-to-end over the real restJson1 request path (`MultiServiceRouter::dispatch`).
//!
//! RULE: an enum clause states the MEMBERSHIP the model declares, in the order this service
//! lists that enum's values — its own listing for the enums whose refusals are recorded
//! publishing one (`src/service.rs`'s `VALUE_LISTINGS`), and the model's declaration order
//! for every enum where it publishes none. The order is a property of the ENUM, not of the
//! member, the operation or the request: one enum states one ordered set wherever it is
//! reached, whichever value offended it and however often it is asked.
//!
//! The rows span what the rule generalises over: both enums with a published listing and
//! ten without, set sizes from one value to eleven, values in SCREAMING_SNAKE and in
//! lowercase, members bound to the body, to a URI label and to the query string, a member
//! nested inside a structure, operations that are implemented and ones still scaffolded,
//! and two enums reached through members of the SAME wire name (`authenticationMaterialType`
//! on `CreateManagedThing` and on `StartDeviceDiscovery`) — which is what shows the listing
//! is keyed by the enum's value set and not by a member name or an operation.
//!
//! Every row also asserts the membership invariant that holds whatever the order is: the
//! values stated are exactly the ones the model declares, none added and none dropped. The
//! declaration orders below are quoted from
//! `smithy_query(query="shape",shape="<the enum>",model_path="services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/smithy-model/com-amazonaws-iotmanagedintegrations-iotmanagedintegrations.json")`.

use serde_json::{json, Value};

use crate::enum_value_set_rendering::{call, emulator, refusal_message, stated_value_set, Reply};

/// A value no enum of this service declares, used wherever a row only needs the member to
/// be outside its set.
const OUTSIDE_EVERY_VALUE_SET: &str = "NOT_A_REAL_VALUE";

// ---------------------------------------------------------------------------------------
// The two enums this service is recorded publishing a listing for
// ---------------------------------------------------------------------------------------

/// `AuthMaterialType`, declared `[CUSTOM_PROTOCOL_QR_BAR_CODE, WIFI_SETUP_QR_BAR_CODE,
/// ZWAVE_QR_BAR_CODE, ZIGBEE_QR_BAR_CODE, DISCOVERED_DEVICE, PRE_ONBOARDED_CLOUD]` and
/// listed by this service in the order below, which is the one its recorded refusal states.
const AUTH_MATERIAL_TYPE_LISTED: &[&str] = &[
    "PRE_ONBOARDED_CLOUD",
    "ZIGBEE_QR_BAR_CODE",
    "ZWAVE_QR_BAR_CODE",
    "WIFI_SETUP_QR_BAR_CODE",
    "DISCOVERED_DEVICE",
    "CUSTOM_PROTOCOL_QR_BAR_CODE",
];

/// The same six values as the MODEL declares them, which the clause's membership must match
/// however it is ordered.
const AUTH_MATERIAL_TYPE_DECLARED: &[&str] = &[
    "CUSTOM_PROTOCOL_QR_BAR_CODE",
    "WIFI_SETUP_QR_BAR_CODE",
    "ZWAVE_QR_BAR_CODE",
    "ZIGBEE_QR_BAR_CODE",
    "DISCOVERED_DEVICE",
    "PRE_ONBOARDED_CLOUD",
];

/// `ProvisioningType`, declared `[FLEET_PROVISIONING, JITR]` and listed by this service as
/// below.
const PROVISIONING_TYPE_LISTED: &[&str] = &["JITR", "FLEET_PROVISIONING"];

/// `ProvisioningType` as the model declares it.
const PROVISIONING_TYPE_DECLARED: &[&str] = &["FLEET_PROVISIONING", "JITR"];

// ---------------------------------------------------------------------------------------
// The enums this service publishes no listing for: the clause states the model's order
// ---------------------------------------------------------------------------------------

/// `Role`.
const ROLE_DECLARED: &[&str] = &["CONTROLLER", "DEVICE"];
/// `HubNetworkMode` — declared in an order that is neither sorted nor its reverse.
const HUB_NETWORK_MODE_DECLARED: &[&str] = &["STANDARD", "NETWORK_WIDE_EXCLUSION"];
/// `EncryptionType`.
const ENCRYPTION_TYPE_DECLARED: &[&str] =
    &["MANAGED_INTEGRATIONS_DEFAULT_ENCRYPTION", "CUSTOMER_KEY_ENCRYPTION"];
/// `DiscoveryType`.
const DISCOVERY_TYPE_DECLARED: &[&str] =
    &["ZWAVE", "ZIGBEE", "CLOUD", "CUSTOM", "CONTROLLER_CAPABILITY_REDISCOVERY"];
/// `DiscoveryAuthMaterialType` — a one-value set, reached through a member whose wire name
/// is the one `AuthMaterialType`'s listed member carries.
const DISCOVERY_AUTH_MATERIAL_TYPE_DECLARED: &[&str] = &["ZWAVE_INSTALL_CODE"];
/// `ProtocolType`.
const PROTOCOL_TYPE_DECLARED: &[&str] = &["ZWAVE", "ZIGBEE", "CUSTOM"];
/// `EventType` — the largest set this service declares on an input member
/// (`CONNECTOR_ASSOCIATION` is `@deprecated`, which does not take it out of the set).
const EVENT_TYPE_DECLARED: &[&str] = &[
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
/// `SchemaVersionType` — values that are not SCREAMING_SNAKE.
const SCHEMA_VERSION_TYPE_DECLARED: &[&str] = &["capability", "definition"];
/// `SchemaVersionFormat`.
const SCHEMA_VERSION_FORMAT_DECLARED: &[&str] = &["AWS", "ZCL", "CONNECTOR"];
/// `SchedulingConfigEndBehavior`.
const SCHEDULING_CONFIG_END_BEHAVIOR_DECLARED: &[&str] =
    &["STOP_ROLLOUT", "CANCEL", "FORCE_CANCEL"];

/// One refused request: what is sent, which member's clause is read, and the ordered set
/// that clause must state.
struct Row {
    /// The case label every assertion names.
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The member path the clause names, in lowerCamel as the wire names it.
    member_path: &'static str,
    /// The admitted values in the order the clause must list them.
    listed: &'static [&'static str],
    /// The same values as the model declares them, which the membership must match.
    declared: &'static [&'static str],
}

/// The clause a refusal must carry for `member_path`, in `listed` order.
fn expected_clause(member_path: &str, listed: &[&str]) -> String {
    format!(
        "Value at '{member_path}' failed to satisfy constraint: Member must satisfy enum value \
         set: [{}]",
        listed.join(", ")
    )
}

/// `values` sorted, so two value sets can be compared as sets.
fn as_set<S: AsRef<str>>(values: &[S]) -> Vec<String> {
    let mut sorted: Vec<String> = values.iter().map(|value| value.as_ref().to_string()).collect();
    sorted.sort();
    sorted
}

/// Assert one refusal states `listed` in order for `member_path`, and that its membership is
/// exactly `declared`. Hands back the message so a caller can assert more about it.
fn assert_lists(reply: &Reply, case: &str, member_path: &str, listed: &[&str], declared: &[&str]) -> String {
    assert_eq!(
        (reply.status, reply.error_type.as_deref()),
        (400, Some("ValidationException")),
        "case {case:?}: expected 400 ValidationException, got {} {:?}",
        reply.status,
        reply.body
    );
    let message = refusal_message(reply, case);
    let expected = expected_clause(member_path, listed);
    assert!(
        message.contains(&expected),
        "case {case:?}: expected the clause {expected:?}, got {message:?}"
    );
    // Whatever the order, the membership is the model's: no value added, none dropped.
    let stated = stated_value_set(&message, member_path)
        .unwrap_or_else(|| panic!("case {case:?}: no enum clause for {member_path:?}: {message:?}"));
    assert_eq!(
        as_set(&stated),
        as_set(declared),
        "case {case:?}: the stated set must hold exactly the declared values, got {stated:?}"
    );
    message
}

/// RULE (see the module docs): an enum clause lists the admitted values in the order this
/// service lists that enum's values — its published listing where it has one, the model's
/// declaration order otherwise.
#[tokio::test]
async fn an_enum_clause_lists_the_values_in_the_order_this_service_lists_them() {
    let router = emulator();

    let rows = vec![
        // The recorded refusal, on an implemented operation: a published listing.
        Row {
            case: "CreateManagedThing/authenticationMaterialType (a published listing)",
            method: "POST",
            path: "/managed-things".to_string(),
            body: Some(json!({
                "Role": "DEVICE",
                "AuthenticationMaterialType": OUTSIDE_EVERY_VALUE_SET,
                "AuthenticationMaterial": "qr:0000000000",
            })),
            member_path: "authenticationMaterialType",
            listed: AUTH_MATERIAL_TYPE_LISTED,
            declared: AUTH_MATERIAL_TYPE_DECLARED,
        },
        // The second recorded refusal, on an operation that is still a scaffold: a
        // constraint is a property of the input shape, so the listing does not wait for a
        // handler to exist.
        Row {
            case: "CreateProvisioningProfile/provisioningType (a published listing)",
            method: "POST",
            path: "/provisioning-profiles".to_string(),
            body: Some(json!({"ProvisioningType": OUTSIDE_EVERY_VALUE_SET})),
            member_path: "provisioningType",
            listed: PROVISIONING_TYPE_LISTED,
            declared: PROVISIONING_TYPE_DECLARED,
        },
        // A second enum on the operation of the first row, with no published listing: the
        // listing belongs to the enum, not to the operation that reached it.
        Row {
            case: "CreateManagedThing/role (no listing: the model's order)",
            method: "POST",
            path: "/managed-things".to_string(),
            body: Some(json!({
                "Role": OUTSIDE_EVERY_VALUE_SET,
                "AuthenticationMaterialType": "ZWAVE_QR_BAR_CODE",
                "AuthenticationMaterial": "qr:0000000000",
            })),
            member_path: "role",
            listed: ROLE_DECLARED,
            declared: ROLE_DECLARED,
        },
        // An enum on a mutation whose resource is named by a URI label.
        Row {
            case: "UpdateManagedThing/hubNetworkMode (no listing: the model's order)",
            method: "PUT",
            path: "/managed-things/anythingatall".to_string(),
            body: Some(json!({"HubNetworkMode": OUTSIDE_EVERY_VALUE_SET})),
            member_path: "hubNetworkMode",
            listed: HUB_NETWORK_MODE_DECLARED,
            declared: HUB_NETWORK_MODE_DECLARED,
        },
        // A member the model declares in lowerCamel already, on an account-level put.
        Row {
            case: "PutDefaultEncryptionConfiguration/encryptionType",
            method: "POST",
            path: "/configuration/account/encryption".to_string(),
            body: Some(json!({"encryptionType": OUTSIDE_EVERY_VALUE_SET})),
            member_path: "encryptionType",
            listed: ENCRYPTION_TYPE_DECLARED,
            declared: ENCRYPTION_TYPE_DECLARED,
        },
        // A five-value set.
        Row {
            case: "StartDeviceDiscovery/discoveryType",
            method: "POST",
            path: "/device-discoveries".to_string(),
            body: Some(json!({"DiscoveryType": OUTSIDE_EVERY_VALUE_SET})),
            member_path: "discoveryType",
            listed: DISCOVERY_TYPE_DECLARED,
            declared: DISCOVERY_TYPE_DECLARED,
        },
        // The SAME member wire name as the first row, targeting a DIFFERENT enum: the
        // published listing is keyed by the enum's value set, so this one-value set states
        // its own value and nothing of `AuthMaterialType`'s.
        Row {
            case: "StartDeviceDiscovery/authenticationMaterialType (another enum, one value)",
            method: "POST",
            path: "/device-discoveries".to_string(),
            body: Some(json!({
                "DiscoveryType": "ZWAVE",
                "AuthenticationMaterialType": OUTSIDE_EVERY_VALUE_SET,
            })),
            member_path: "authenticationMaterialType",
            listed: DISCOVERY_AUTH_MATERIAL_TYPE_DECLARED,
            declared: DISCOVERY_AUTH_MATERIAL_TYPE_DECLARED,
        },
        Row {
            case: "StartDeviceDiscovery/protocol",
            method: "POST",
            path: "/device-discoveries".to_string(),
            body: Some(json!({
                "DiscoveryType": "ZWAVE",
                "Protocol": OUTSIDE_EVERY_VALUE_SET,
            })),
            member_path: "protocol",
            listed: PROTOCOL_TYPE_DECLARED,
            declared: PROTOCOL_TYPE_DECLARED,
        },
        // The largest set this service declares on an input member, in the body.
        Row {
            case: "CreateNotificationConfiguration/eventType (eleven values)",
            method: "POST",
            path: "/notification-configurations".to_string(),
            body: Some(json!({
                "EventType": OUTSIDE_EVERY_VALUE_SET,
                "DestinationName": "destination-one",
            })),
            member_path: "eventType",
            listed: EVENT_TYPE_DECLARED,
            declared: EVENT_TYPE_DECLARED,
        },
        // The same enum as the row above, on another operation and bound to a URI LABEL
        // instead of the body: one enum, one ordered set.
        Row {
            case: "UpdateNotificationConfiguration/eventType (the same enum, a URI label)",
            method: "PUT",
            path: format!("/notification-configurations/{OUTSIDE_EVERY_VALUE_SET}"),
            body: Some(json!({"DestinationName": "destination-one"})),
            member_path: "eventType",
            listed: EVENT_TYPE_DECLARED,
            declared: EVENT_TYPE_DECLARED,
        },
        // Values that are not SCREAMING_SNAKE, on a read, bound to a URI label.
        Row {
            case: "GetSchemaVersion/type (lowercase values, a URI label)",
            method: "GET",
            path: format!("/schema-versions/{OUTSIDE_EVERY_VALUE_SET}/switch@1.0"),
            body: None,
            member_path: "type",
            listed: SCHEMA_VERSION_TYPE_DECLARED,
            declared: SCHEMA_VERSION_TYPE_DECLARED,
        },
        // An enum bound to the QUERY STRING of the same read.
        Row {
            case: "GetSchemaVersion/format (a query-string member)",
            method: "GET",
            path: format!("/schema-versions/capability/switch@1.0?Format={OUTSIDE_EVERY_VALUE_SET}"),
            body: None,
            member_path: "format",
            listed: SCHEMA_VERSION_FORMAT_DECLARED,
            declared: SCHEMA_VERSION_FORMAT_DECLARED,
        },
        // An enum NESTED inside a structure member: the clause names the whole path and
        // states that member's own ordered set.
        Row {
            case: "CreateOtaTask/otaSchedulingConfig.endBehavior (nested)",
            method: "POST",
            path: "/ota-tasks".to_string(),
            body: Some(json!({
                "S3Url": "s3://a-bucket/firmware.bin",
                "OtaType": "ONE_TIME",
                "OtaSchedulingConfig": {"EndBehavior": OUTSIDE_EVERY_VALUE_SET},
            })),
            member_path: "otaSchedulingConfig.endBehavior",
            listed: SCHEDULING_CONFIG_END_BEHAVIOR_DECLARED,
            declared: SCHEDULING_CONFIG_END_BEHAVIOR_DECLARED,
        },
    ];

    for row in &rows {
        let reply = call(&router, row.method, &row.path, row.body.clone()).await;
        assert_lists(&reply, row.case, row.member_path, row.listed, row.declared);
    }
}

/// RULE (the recorded refusals): the WHOLE message of a request whose single fault is an
/// out-of-set enum member is the count prefix, the member's position and the listed set.
#[tokio::test]
async fn one_out_of_set_member_is_refused_with_the_whole_sentence() {
    let router = emulator();

    let cases = [
        (
            "CreateManagedThing/authenticationMaterialType",
            "POST",
            "/managed-things".to_string(),
            Some(json!({
                "Role": "DEVICE",
                "AuthenticationMaterialType": OUTSIDE_EVERY_VALUE_SET,
                "AuthenticationMaterial": "qr:0000000000",
            })),
            "authenticationMaterialType",
            AUTH_MATERIAL_TYPE_LISTED,
        ),
        (
            "CreateProvisioningProfile/provisioningType",
            "POST",
            "/provisioning-profiles".to_string(),
            Some(json!({"ProvisioningType": OUTSIDE_EVERY_VALUE_SET})),
            "provisioningType",
            PROVISIONING_TYPE_LISTED,
        ),
        (
            "CreateManagedThing/role",
            "POST",
            "/managed-things".to_string(),
            Some(json!({
                "Role": OUTSIDE_EVERY_VALUE_SET,
                "AuthenticationMaterialType": "ZWAVE_QR_BAR_CODE",
                "AuthenticationMaterial": "qr:0000000000",
            })),
            "role",
            ROLE_DECLARED,
        ),
    ];

    for (case, method, path, body, member_path, listed) in &cases {
        let reply = call(&router, method, path, body.clone()).await;
        let message = assert_lists(&reply, case, member_path, listed, listed);
        let expected =
            format!("1 validation error detected: {}", expected_clause(member_path, listed));
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

/// RULE (invariant half): the ordered set a clause states is a fact about the enum — it does
/// not vary with the value the request sent, nor between two identical calls, and it is the
/// same when the clause stands beside another member's clause in one refusal.
#[tokio::test]
async fn the_listed_order_does_not_vary_with_the_request() {
    let router = emulator();

    // A declared value in the wrong case, a declared value with trailing whitespace, a
    // value carrying the bracket the set is rendered with, a value past the 256 characters
    // the framework echoes before eliding, and the empty string.
    let long_value = "Z".repeat(300);
    let rejected_values = [
        OUTSIDE_EVERY_VALUE_SET.to_string(),
        "jitr".to_string(),
        "JITR ".to_string(),
        "BRACKET]IN]VALUE".to_string(),
        long_value,
        String::new(),
    ];

    for rejected in &rejected_values {
        let case = format!("CreateProvisioningProfile/provisioningType = {rejected:?}");
        let body = Some(json!({"ProvisioningType": rejected}));
        let message = assert_lists(
            &call(&router, "POST", "/provisioning-profiles", body.clone()).await,
            &case,
            "provisioningType",
            PROVISIONING_TYPE_LISTED,
            PROVISIONING_TYPE_DECLARED,
        );
        // The same request twice is refused identically: the clause reads nothing a call
        // changes.
        let again = call(&router, "POST", "/provisioning-profiles", body).await;
        assert_eq!(
            refusal_message(&again, &case),
            message,
            "case {case:?}: two identical calls must be refused identically"
        );
    }

    // Beside another member's clause: one past `ProvisioningProfileName`'s `@length`
    // maximum of 36 (`smithy_query(query="operation",operation="CreateProvisioningProfile")`)
    // and an out-of-set `ProvisioningType`, so the refusal carries two clauses.
    let case = "CreateProvisioningProfile/two clauses in one refusal";
    let reply = call(
        &router,
        "POST",
        "/provisioning-profiles",
        Some(json!({
            "ProvisioningType": OUTSIDE_EVERY_VALUE_SET,
            "Name": "a".repeat(37),
        })),
    )
    .await;
    let message = assert_lists(
        &reply,
        case,
        "provisioningType",
        PROVISIONING_TYPE_LISTED,
        PROVISIONING_TYPE_DECLARED,
    );
    assert!(
        message.starts_with("2 validation errors detected: ") && message.contains("'name'"),
        "case {case:?}: expected both clauses in one envelope, got {message:?}"
    );
}
