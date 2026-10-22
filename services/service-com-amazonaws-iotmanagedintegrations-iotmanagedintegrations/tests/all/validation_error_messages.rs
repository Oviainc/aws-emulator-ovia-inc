// SPDX-License-Identifier: CC0-1.0

//! How this service states a request it refuses for violating a constraint its model
//! declares, over the real restJson1 request path (`MultiServiceRouter::dispatch`).
//!
//! RULE: a request outside a constraint the model declares — an absent `@required`
//! member, or a value outside `@length`, `@range`, `@pattern` or an enum's value set —
//! is answered with this service's MODELLED `ValidationException`: HTTP 400, the wire
//! error code `ValidationException`, and the prose in the member the model names
//! `Message`
//! (`smithy_query(query="shape",shape="ValidationException")` reports
//! `ValidationException {Message: ErrorMessage}`, `httpError` 400). The prose is the AWS
//! validation envelope — `1 validation error detected: Value at '<member path>' failed
//! to satisfy constraint: <the requirement>` — which names the member path in lowerCamel
//! as the wire names it, restates the constraint it failed, and does NOT quote the
//! rejected value back.
//!
//! The recorded refusals this rule was derived from are two rows of the table below
//! (`authenticationMaterialType` and `provisioningType`); the rest span the class the
//! rule is about: every constraint class the framework enforces, a member bound to the
//! request's URI as well as to its body, a member nested inside a structure, an
//! operation that is still a scaffold, and a member the model marks `@sensitive` — whose
//! value a refusal must never publish.
//!
//! The happy path is asserted too: wrapping every operation must not change what a
//! request the model accepts is answered with.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::state::{
    AUTH_MATERIAL_TYPE_VALUES, HUB_NETWORK_MODE_VALUES, ROLE_VALUES,
};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's requests are signed for.
const REGION: &str = "us-east-1";

/// The `ProvisioningType` value set, as the model declares it
/// (`smithy_query(query="shape",shape="ProvisioningType")` -> `["FLEET_PROVISIONING",
/// "JITR"]`). The crate exports the same list as
/// `crate::ops::types::PROVISIONING_TYPE_VALUES`, which is not re-exported through
/// `state`, so it is named here with its source.
const PROVISIONING_TYPE_VALUES: &[&str] = &["FLEET_PROVISIONING", "JITR"];

/// One HTTP answer, reduced to what the assertions read.
struct Reply {
    status: u16,
    /// The restJson1 error code, from `x-amzn-errortype`.
    error_type: Option<String>,
    body: Value,
}

/// A router holding one `iotmanagedintegrations` service over a fresh shared engine:
/// the same assembly `core/router` performs, with one service mounted.
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

/// One refused request: what is sent, and what the refusal must say about it.
struct Case {
    /// The case label every assertion names.
    label: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The member path the clause must name, in lowerCamel as the wire names it.
    member_path: &'static str,
    /// The requirement the clause must restate, as the framework words that constraint
    /// class.
    requirement: String,
    /// The value the request sent for that member, which the clause must NOT quote back.
    /// `None` for an absent member, which has no value to withhold.
    rejected_value: Option<String>,
}

/// The opening of the requirement clause for an enum value set. The set's RENDERING is
/// the framework's and its order is the model's declaration order, which a value set
/// does not fix; that every declared value appears is asserted in
/// [`an_enum_refusal_names_every_value_the_model_declares`].
const ENUM_VALUE_SET_CLAUSE: &str = "Member must satisfy enum value set: [";

/// RULE (see the module docs): every constraint the model declares is refused with the
/// modelled `ValidationException`, naming the member and the requirement and quoting no
/// rejected value back.
#[tokio::test]
async fn a_constraint_refusal_is_the_modelled_validation_exception() {
    let router = emulator();

    // A name 37 characters long: one past `ProvisioningProfileName`'s `@length` maximum
    // of 36 (`smithy_query(query="operation",operation="CreateProvisioningProfile")`).
    let too_long_name = "a".repeat(37);
    // One past `ManagedThingId`'s maximum of 64, on a member bound to the URI.
    let too_long_identifier = "b".repeat(65);

    let cases = vec![
        // The recorded refusal this issue was filed about: an out-of-set enum value on
        // a `@required` member of an implemented operation.
        Case {
            label: "CreateManagedThing/AuthenticationMaterialType outside the value set",
            method: "POST",
            path: "/managed-things".to_string(),
            body: Some(json!({
                "Role": "DEVICE",
                "AuthenticationMaterialType": "NOT_A_REAL_TYPE",
                "AuthenticationMaterial": "qr:0000000000",
            })),
            member_path: "authenticationMaterialType",
            requirement: ENUM_VALUE_SET_CLAUSE.to_string(),
            rejected_value: Some("NOT_A_REAL_TYPE".to_string()),
        },
        // The same class on the same operation's other enum member: the rule is about
        // the constraint, not about one member.
        Case {
            label: "CreateManagedThing/Role outside the value set",
            method: "POST",
            path: "/managed-things".to_string(),
            body: Some(json!({
                "Role": "SUPERVISOR",
                "AuthenticationMaterialType": "ZWAVE_QR_BAR_CODE",
                "AuthenticationMaterial": "qr:0000000000",
            })),
            member_path: "role",
            requirement: ENUM_VALUE_SET_CLAUSE.to_string(),
            rejected_value: Some("SUPERVISOR".to_string()),
        },
        // The second recorded refusal, on an operation that is still a scaffold: a
        // constraint is a property of the input shape, so the refusal does not wait for
        // a handler to exist.
        Case {
            label: "CreateProvisioningProfile/ProvisioningType outside the value set",
            method: "POST",
            path: "/provisioning-profiles".to_string(),
            body: Some(json!({"ProvisioningType": "NOT_A_REAL_TYPE"})),
            member_path: "provisioningType",
            requirement: ENUM_VALUE_SET_CLAUSE.to_string(),
            rejected_value: Some("NOT_A_REAL_TYPE".to_string()),
        },
        // An enum member on an update, reached through a URI label: the enum is
        // refused whether or not the resource the label names exists.
        Case {
            label: "UpdateManagedThing/HubNetworkMode outside the value set",
            method: "PUT",
            path: "/managed-things/anythingatall".to_string(),
            body: Some(json!({"HubNetworkMode": "MESH"})),
            member_path: "hubNetworkMode",
            requirement: ENUM_VALUE_SET_CLAUSE.to_string(),
            rejected_value: Some("MESH".to_string()),
        },
        // An absent `@required` member: the one class with no value to withhold.
        Case {
            label: "CreateManagedThing/AuthenticationMaterialType absent",
            method: "POST",
            path: "/managed-things".to_string(),
            body: Some(json!({
                "Role": "DEVICE",
                "AuthenticationMaterial": "qr:0000000000",
            })),
            member_path: "authenticationMaterialType",
            requirement: "Member must not be null".to_string(),
            rejected_value: None,
        },
        // `@length`, over the maximum.
        Case {
            label: "CreateProvisioningProfile/Name one past the length maximum",
            method: "POST",
            path: "/provisioning-profiles".to_string(),
            body: Some(json!({
                "ProvisioningType": "FLEET_PROVISIONING",
                "Name": too_long_name.clone(),
            })),
            member_path: "name",
            requirement: "Member must have length less than or equal to 36".to_string(),
            rejected_value: Some(too_long_name.clone()),
        },
        // `@pattern`, on the same member the length case used: two classes, one member.
        Case {
            label: "CreateProvisioningProfile/Name outside the pattern",
            method: "POST",
            path: "/provisioning-profiles".to_string(),
            body: Some(json!({
                "ProvisioningType": "JITR",
                "Name": "not a legal name",
            })),
            member_path: "name",
            requirement: "Member must satisfy regular expression pattern: ^[0-9A-Za-z_-]+$"
                .to_string(),
            rejected_value: Some("not a legal name".to_string()),
        },
        // `@pattern` on a member the model marks `@sensitive` (`Brand`): the clause must
        // name the member and the pattern and publish nothing of the value.
        Case {
            label: "CreateManagedThing/Brand outside the pattern (a @sensitive member)",
            method: "POST",
            path: "/managed-things".to_string(),
            body: Some(json!({
                "Role": "DEVICE",
                "AuthenticationMaterialType": "ZWAVE_QR_BAR_CODE",
                "AuthenticationMaterial": "qr:0000000000",
                "Brand": "Acme*Secret*Brand",
            })),
            member_path: "brand",
            requirement: "Member must satisfy regular expression pattern: ^[A-Za-z0-9-_ ]+$"
                .to_string(),
            rejected_value: Some("Acme*Secret*Brand".to_string()),
        },
        // `@range`, on a member nested inside a structure: the clause names the whole
        // path from the input inwards, not just the leaf.
        Case {
            label: "CreateManagedThing/WiFiSimpleSetupConfiguration.TimeoutInMinutes below \
                    the range minimum",
            method: "POST",
            path: "/managed-things".to_string(),
            body: Some(json!({
                "Role": "CONTROLLER",
                "AuthenticationMaterialType": "WIFI_SETUP_QR_BAR_CODE",
                "AuthenticationMaterial": "qr:0000000000",
                "WiFiSimpleSetupConfiguration": {"TimeoutInMinutes": 2},
            })),
            member_path: "wiFiSimpleSetupConfiguration.timeoutInMinutes",
            requirement: "Member must have value greater than or equal to 5".to_string(),
            rejected_value: Some("2".to_string()),
        },
        // `@length` on a member bound to the URI rather than to the body, on a read.
        Case {
            label: "GetManagedThing/Identifier one past the length maximum",
            method: "GET",
            path: format!("/managed-things/{too_long_identifier}"),
            body: None,
            member_path: "identifier",
            requirement: "Member must have length less than or equal to 64".to_string(),
            rejected_value: Some(too_long_identifier.clone()),
        },
    ];

    for case in &cases {
        let label = case.label;
        let reply = call(&router, case.method, &case.path, case.body.clone()).await;

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
        // The MODELLED member name. The framework's own refusal envelope writes a
        // lowercase `message` instead, which this service's model does not declare, so
        // a caller reading `Message` would find nothing.
        let message = reply.body.get("Message").and_then(Value::as_str);
        assert!(
            message.is_some(),
            "case {label:?}: the modelled Message member must carry the refusal, got body \
             {:?}",
            reply.body
        );
        assert!(
            reply.body.get("message").is_none(),
            "case {label:?}: `message` is not a member this service's model declares, got \
             body {:?}",
            reply.body
        );
        let message = message.expect("asserted present just above");
        assert!(
            message.starts_with("1 validation error detected: "),
            "case {label:?}: expected the AWS validation envelope, got {message:?}"
        );
        assert!(
            message.contains(&format!("at '{}'", case.member_path)),
            "case {label:?}: the clause must name the member path {:?}, got {message:?}",
            case.member_path
        );
        assert!(
            message.contains(&case.requirement),
            "case {label:?}: the clause must restate the requirement {:?}, got {message:?}",
            case.requirement
        );
        if let Some(value) = &case.rejected_value {
            assert!(
                !message.contains(value.as_str()),
                "case {label:?}: the clause must not quote the rejected value {value:?} back, \
                 got {message:?}"
            );
            assert!(
                message.contains(&format!("Value at '{}'", case.member_path)),
                "case {label:?}: a refused value is stated as \"Value at '<member>'\", got \
                 {message:?}"
            );
        } else {
            assert!(
                message.contains(&format!("Value null at '{}'", case.member_path)),
                "case {label:?}: an absent member is stated as \"Value null at '<member>'\", \
                 got {message:?}"
            );
        }
    }
}

/// RULE: an enum refusal names every value the model declares for that member, so a
/// caller can repair the request from the message alone. The set is unordered on the
/// wire, so each value is asserted present rather than in a position.
#[tokio::test]
async fn an_enum_refusal_names_every_value_the_model_declares() {
    let router = emulator();

    let cases: Vec<(&str, &str, String, Value, &[&str])> = vec![
        (
            "CreateManagedThing/AuthenticationMaterialType",
            "POST",
            "/managed-things".to_string(),
            json!({
                "Role": "DEVICE",
                "AuthenticationMaterialType": "NOT_A_REAL_TYPE",
                "AuthenticationMaterial": "qr:0000000000",
            }),
            AUTH_MATERIAL_TYPE_VALUES,
        ),
        (
            "CreateManagedThing/Role",
            "POST",
            "/managed-things".to_string(),
            json!({
                "Role": "SUPERVISOR",
                "AuthenticationMaterialType": "ZWAVE_QR_BAR_CODE",
                "AuthenticationMaterial": "qr:0000000000",
            }),
            ROLE_VALUES,
        ),
        (
            "CreateProvisioningProfile/ProvisioningType",
            "POST",
            "/provisioning-profiles".to_string(),
            json!({"ProvisioningType": "NOT_A_REAL_TYPE"}),
            PROVISIONING_TYPE_VALUES,
        ),
        (
            "UpdateManagedThing/HubNetworkMode",
            "PUT",
            "/managed-things/anythingatall".to_string(),
            json!({"HubNetworkMode": "MESH"}),
            HUB_NETWORK_MODE_VALUES,
        ),
    ];

    for (label, method, path, body, values) in &cases {
        let reply = call(&router, method, path, Some(body.clone())).await;
        let message = reply
            .body
            .get("Message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            reply.status, 400,
            "case {label:?}: expected HTTP 400, got {} with body {:?}",
            reply.status, reply.body
        );
        for value in *values {
            assert!(
                message.contains(value),
                "case {label:?}: the value set must name {value:?}, got {message:?}"
            );
        }
    }
}

/// RULE: a request that violates more than one constraint is refused once, with every
/// clause in the one envelope — the count, then the clauses joined by `"; "` — and still
/// quotes no rejected value back.
#[tokio::test]
async fn several_violations_are_one_refusal_with_every_clause() {
    let router = emulator();

    // One past `ProvisioningProfileName`'s `@length` maximum of 36, and a
    // `ProvisioningType` outside its value set: two members, two constraint classes.
    let too_long_name = "a".repeat(37);
    let reply = call(
        &router,
        "POST",
        "/provisioning-profiles",
        Some(json!({"ProvisioningType": "NOT_A_REAL_TYPE", "Name": too_long_name.clone()})),
    )
    .await;

    let label = "CreateProvisioningProfile/two violations";
    assert_eq!(
        reply.status, 400,
        "case {label:?}: expected HTTP 400, got {} with body {:?}",
        reply.status, reply.body
    );
    let message = reply
        .body
        .get("Message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        message.starts_with("2 validation errors detected: "),
        "case {label:?}: two violations are counted in one envelope, got {message:?}"
    );
    assert!(
        message.contains("; "),
        "case {label:?}: the clauses are joined by \"; \", got {message:?}"
    );
    for member_path in ["provisioningType", "name"] {
        assert!(
            message.contains(&format!("Value at '{member_path}'")),
            "case {label:?}: the envelope must carry the clause for {member_path:?}, got \
             {message:?}"
        );
    }
    for value in ["NOT_A_REAL_TYPE", too_long_name.as_str()] {
        assert!(
            !message.contains(value),
            "case {label:?}: no clause may quote the rejected value {value:?} back, got \
             {message:?}"
        );
    }
}

/// RULE: a request the model accepts is unaffected — it reaches the handler and is
/// answered with the operation's own success, which a read then projects. Serving every
/// operation through the refusal wrapper must not change the happy path, and a create is
/// proved by reading the resource back.
#[tokio::test]
async fn a_request_the_model_accepts_still_reaches_its_handler() {
    let router = emulator();

    for &material_type in AUTH_MATERIAL_TYPE_VALUES {
        let created = call(
            &router,
            "POST",
            "/managed-things",
            Some(json!({
                "Role": "DEVICE",
                "AuthenticationMaterialType": material_type,
                "AuthenticationMaterial": "qr:0000000000",
                "Name": "lamp one",
            })),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {material_type:?}: expected HTTP 201 from CreateManagedThing, got {} with \
             body {:?}",
            created.status, created.body
        );
        let id = created
            .body
            .get("Id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!("case {material_type:?}: expected an Id, got body {:?}", created.body)
            })
            .to_string();

        let read = call(&router, "GET", &format!("/managed-things/{id}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {material_type:?}: expected HTTP 200 from GetManagedThing, got {} with body \
             {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body.get("Id").and_then(Value::as_str),
            Some(id.as_str()),
            "case {material_type:?}: the read must report the created id, got {:?}",
            read.body.get("Id")
        );
        assert_eq!(
            read.body.get("Name").and_then(Value::as_str),
            Some("lamp one"),
            "case {material_type:?}: the read must project what the create stored, got {:?}",
            read.body.get("Name")
        );
    }

    // A scaffolded operation whose input the model accepts still answers the scaffold's
    // 501: the wrapper refuses what the model refuses and nothing else.
    let scaffolded = call(
        &router,
        "POST",
        "/provisioning-profiles",
        Some(json!({"ProvisioningType": "FLEET_PROVISIONING", "Name": "profile-1"})),
    )
    .await;
    assert_eq!(
        scaffolded.status, 501,
        "case \"CreateProvisioningProfile/valid\": a valid request must reach the operation, \
         which is still scaffolded, got {} with body {:?}",
        scaffolded.status, scaffolded.body
    );
}
