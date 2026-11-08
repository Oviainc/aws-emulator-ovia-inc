// SPDX-License-Identifier: CC0-1.0

//! The configuration a user pool read reports, over the wire — driven through
//! the router exactly as an SDK drives it (`POST /` with `X-Amz-Target:
//! AWSCognitoIdentityProviderService.<Op>`) and read back through
//! `DescribeUserPool` and `ListUserPools`.
//!
//! The rules under test, each asserted over a SET of inputs rather than one
//! request:
//!
//! * a pool HOLDS every configuration block the service materialises for it, so
//!   each one is reported by every read: the value the request configured where
//!   it carried one, and the service's own default where it carried none — "if
//!   you don't provide a value for an attribute, Amazon Cognito sets it to its
//!   default value" (`CreateUserPool`);
//! * a schema attribute reports the constraints structure of ITS data type —
//!   "specifies the constraints for an attribute of the number type" / "... of
//!   the string type" (`SchemaAttributeType`) — carrying the bounds that were
//!   configured and EMPTY where none were, while an attribute of any other type
//!   reports neither structure;
//! * the schema a new pool starts with is the service's own standard attribute
//!   set, each attribute with the service's own bounds, and a read of a pool
//!   reports the attributes the service maintains itself beside the configured
//!   ones;
//! * a listing entry renders the same stored pool the reader of that one pool
//!   renders, including the trigger configuration and the Regions the pool holds
//!   a replica in;
//! * a pool's tags are assigned and removed by the tagging operations, so an
//!   update that names none leaves them as they are.
//!
//! Each table row names its case and every assertion reports the actual value.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service, and the
/// Region it names is the one a created pool lives in. The signature is never
/// checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The Region the requests above are signed for, which is the Region a created
/// pool holds its PRIMARY replica in.
const REGION: &str = "us-east-1";

/// A Lambda function ARN a trigger can name (`LambdaConfigType.PreSignUp`
/// targets `ArnType`: length 20..2048).
const TRIGGER_PRE_SIGN_UP: &str = "arn:aws:lambda:us-east-1:000000000000:function:pre-sign-up";

/// A KMS key ARN a key configuration can name (`KeyConfigurationType.KmsKeyArn`:
/// length 20..2048, the ARN pattern).
const KMS_KEY_ARN: &str = "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router.set_request_authentication(false);
    router
}

/// Call `operation` with `body` and return its status and JSON response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "cognito-idp.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, payload)
}

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// Create the pool `request` describes and answer the `UserPool` the create
/// renders.
async fn create(router: &MultiServiceRouter, request: &Value) -> Value {
    ok(router, "CreateUserPool", request).await["UserPool"].clone()
}

/// The pool `user_pool_id` names, as `DescribeUserPool` renders it.
async fn described(router: &MultiServiceRouter, user_pool_id: &str) -> Value {
    ok(
        router,
        "DescribeUserPool",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await["UserPool"]
        .clone()
}

/// `pool`'s minted id.
fn pool_id(pool: &Value) -> String {
    pool["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("a created pool carries a minted id, got {pool}"))
        .to_string()
}

/// The value at a dotted path inside `value`, or `None` when any step is absent.
fn at<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for step in path.split('.') {
        current = current.get(step)?;
    }
    Some(current)
}

/// Assert `pool` carries `expected` at `path`; a `Value::Null` expectation means
/// the member must be ABSENT.
fn assert_member(case: &str, label: &str, pool: &Value, path: &str, expected: &Value) {
    let actual = at(pool, path);
    if expected.is_null() {
        assert!(
            actual.is_none_or(Value::is_null),
            "case {case:?}: {label} must report no {path}, got {actual:?}"
        );
    } else {
        assert_eq!(
            actual,
            Some(expected),
            "case {case:?}: {label} must report {path} as {expected}, got {actual:?}"
        );
    }
}

/// The `Name` of every entry of a rendered `SchemaAttributes`, in order.
fn schema_names(pool: &Value) -> Vec<String> {
    pool["SchemaAttributes"]
        .as_array()
        .unwrap_or_else(|| panic!("a rendered pool carries its schema, got {pool}"))
        .iter()
        .map(|entry| entry["Name"].as_str().unwrap_or("<no Name>").to_string())
        .collect()
}

/// The schema entry of `pool` named `name`.
fn schema_entry(case: &str, pool: &Value, name: &str) -> Value {
    pool["SchemaAttributes"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: a rendered pool carries its schema, got {pool}"))
        .iter()
        .find(|entry| entry["Name"] == json!(name))
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the schema must declare {name:?}, got {:?}", schema_names(pool))
        })
}

// ---------------------------------------------------------------------------
// The configuration blocks a pool materialises
// ---------------------------------------------------------------------------

/// One row of the configuration table: the members a `CreateUserPool` request
/// carries beyond `PoolName`, and what every read of the pool must then report.
/// The rows differ along WHICH blocks the request configures, how much of each
/// one it fills in, and the form of the values it carries.
struct ConfigurationCase {
    case: &'static str,
    members: fn() -> Value,
    expected: fn() -> Vec<(&'static str, Value)>,
}

/// What a pool whose request configured nothing reports: every block the service
/// materialises, at its default.
fn server_set_defaults() -> Vec<(&'static str, Value)> {
    vec![
        ("LambdaConfig", json!({})),
        ("EmailConfiguration", json!({ "EmailSendingAccount": "COGNITO_DEFAULT" })),
        ("IssuerConfiguration", json!({ "Type": "ORIGINAL" })),
        ("KeyConfiguration", json!({ "KeyType": "AWS_OWNED_KEY" })),
        (
            "UserAttributeUpdateSettings",
            json!({ "AttributesRequireVerificationBeforeUpdate": [] }),
        ),
        (
            "VerificationMessageTemplate",
            json!({ "DefaultEmailOption": "CONFIRM_WITH_CODE" }),
        ),
        (
            "AccountRecoverySetting",
            json!({
                "RecoveryMechanisms": [
                    { "Name": "verified_email", "Priority": 1 },
                    { "Name": "verified_phone_number", "Priority": 2 },
                ],
            }),
        ),
        // A pool whose request configured no threat protection carries none at
        // all, so this block is the one a read reports only when it is asked for.
        ("UserPoolAddOns", Value::Null),
    ]
}

/// [`server_set_defaults`] with the expectation for each `path` this row
/// configures replaced.
fn overriding(overrides: Vec<(&'static str, Value)>) -> Vec<(&'static str, Value)> {
    let mut expected = server_set_defaults();
    for (path, value) in overrides {
        expected.retain(|(existing, _)| *existing != path);
        expected.push((path, value));
    }
    expected
}

fn configuration_cases() -> Vec<ConfigurationCase> {
    vec![
        ConfigurationCase {
            case: "nothing but the required name: every block at its default",
            members: || json!({}),
            expected: server_set_defaults,
        },
        ConfigurationCase {
            case: "an email configuration naming only a reply-to address",
            members: || {
                json!({
                    "EmailConfiguration": { "ReplyToEmailAddress": "replies@example.com" },
                })
            },
            expected: || {
                overriding(vec![(
                    "EmailConfiguration",
                    json!({
                        "ReplyToEmailAddress": "replies@example.com",
                        "EmailSendingAccount": "COGNITO_DEFAULT",
                    }),
                )])
            },
        },
        ConfigurationCase {
            case: "an email configuration that sends through the caller's own account",
            members: || {
                json!({
                    "EmailConfiguration": {
                        "EmailSendingAccount": "DEVELOPER",
                        "SourceArn": "arn:aws:ses:us-east-1:000000000000:identity/example.com",
                        "From": "Test User <sender@example.com>",
                    },
                })
            },
            expected: || {
                overriding(vec![(
                    "EmailConfiguration",
                    json!({
                        "EmailSendingAccount": "DEVELOPER",
                        "SourceArn": "arn:aws:ses:us-east-1:000000000000:identity/example.com",
                        "From": "Test User <sender@example.com>",
                    }),
                )])
            },
        },
        ConfigurationCase {
            case: "a verification template carrying only an SMS message",
            members: || {
                json!({
                    "VerificationMessageTemplate": { "SmsMessage": "Your code is {####}" },
                })
            },
            expected: || {
                overriding(vec![(
                    "VerificationMessageTemplate",
                    json!({
                        "SmsMessage": "Your code is {####}",
                        "DefaultEmailOption": "CONFIRM_WITH_CODE",
                    }),
                )])
            },
        },
        ConfigurationCase {
            case: "a verification template choosing the clickable-link option",
            members: || {
                json!({
                    "VerificationMessageTemplate": {
                        "DefaultEmailOption": "CONFIRM_WITH_LINK",
                        "EmailMessageByLink": "Confirm here: {##Click here##}",
                    },
                })
            },
            expected: || {
                overriding(vec![(
                    "VerificationMessageTemplate",
                    json!({
                        "DefaultEmailOption": "CONFIRM_WITH_LINK",
                        "EmailMessageByLink": "Confirm here: {##Click here##}",
                    }),
                )])
            },
        },
        ConfigurationCase {
            case: "two Lambda triggers: a configured block is reported as configured",
            members: || {
                json!({
                    "LambdaConfig": {
                        "PreSignUp": TRIGGER_PRE_SIGN_UP,
                        "PostConfirmation": TRIGGER_PRE_SIGN_UP,
                    },
                })
            },
            expected: || {
                overriding(vec![(
                    "LambdaConfig",
                    json!({
                        "PreSignUp": TRIGGER_PRE_SIGN_UP,
                        "PostConfirmation": TRIGGER_PRE_SIGN_UP,
                    }),
                )])
            },
        },
        ConfigurationCase {
            case: "an issuer and a customer-managed key named in full",
            members: || {
                json!({
                    "IssuerConfiguration": { "Type": "UPDATED" },
                    "KeyConfiguration": {
                        "KeyType": "CUSTOMER_MANAGED_KEY",
                        "KmsKeyArn": KMS_KEY_ARN,
                    },
                })
            },
            expected: || {
                overriding(vec![
                    ("IssuerConfiguration", json!({ "Type": "UPDATED" })),
                    (
                        "KeyConfiguration",
                        json!({ "KeyType": "CUSTOMER_MANAGED_KEY", "KmsKeyArn": KMS_KEY_ARN }),
                    ),
                ])
            },
        },
        ConfigurationCase {
            case: "attribute updates that wait on verification of both contact attributes",
            members: || {
                json!({
                    "UserAttributeUpdateSettings": {
                        "AttributesRequireVerificationBeforeUpdate": ["email", "phone_number"],
                    },
                    "AccountRecoverySetting": {
                        "RecoveryMechanisms": [{ "Name": "verified_phone_number", "Priority": 1 }],
                    },
                })
            },
            expected: || {
                overriding(vec![
                    (
                        "UserAttributeUpdateSettings",
                        json!({
                            "AttributesRequireVerificationBeforeUpdate": ["email", "phone_number"],
                        }),
                    ),
                    (
                        "AccountRecoverySetting",
                        json!({
                            "RecoveryMechanisms": [
                                { "Name": "verified_phone_number", "Priority": 1 },
                            ],
                        }),
                    ),
                ])
            },
        },
        ConfigurationCase {
            case: "threat protection logging only: the additional flows get their mode",
            members: || json!({ "UserPoolAddOns": { "AdvancedSecurityMode": "AUDIT" } }),
            expected: || {
                overriding(vec![(
                    "UserPoolAddOns",
                    json!({
                        "AdvancedSecurityMode": "AUDIT",
                        "AdvancedSecurityAdditionalFlows": { "CustomAuthMode": "AUDIT" },
                    }),
                )])
            },
        },
        ConfigurationCase {
            case: "threat protection whose custom-authentication mode the request chose",
            members: || {
                json!({
                    "UserPoolAddOns": {
                        "AdvancedSecurityMode": "AUDIT",
                        "AdvancedSecurityAdditionalFlows": { "CustomAuthMode": "ENFORCED" },
                    },
                })
            },
            expected: || {
                overriding(vec![(
                    "UserPoolAddOns",
                    json!({
                        "AdvancedSecurityMode": "AUDIT",
                        "AdvancedSecurityAdditionalFlows": { "CustomAuthMode": "ENFORCED" },
                    }),
                )])
            },
        },
    ]
}

#[tokio::test]
async fn every_read_of_a_pool_reports_the_configuration_the_pool_holds() {
    let router = router();

    for (index, row) in configuration_cases().iter().enumerate() {
        let case = row.case;
        let mut body: Map<String, Value> = (row.members)()
            .as_object()
            .cloned()
            .expect("a row's members are a JSON object");
        body.insert("PoolName".to_string(), json!(format!("configured-{index}")));
        let created = create(&router, &Value::Object(body)).await;
        let user_pool_id = pool_id(&created);
        let read = described(&router, &user_pool_id).await;

        for (path, expected) in (row.expected)() {
            assert_member(case, "the create's answer", &created, path, &expected);
            assert_member(case, "a later read", &read, path, &expected);
        }
    }
}

#[tokio::test]
async fn an_update_restores_the_blocks_it_names_none_of_and_keeps_the_pools_tags() {
    let router = router();
    let created = create(
        &router,
        &json!({
            "PoolName": "configured-then-reset",
            "LambdaConfig": { "PreSignUp": TRIGGER_PRE_SIGN_UP },
            "EmailConfiguration": { "EmailSendingAccount": "DEVELOPER",
                                    "SourceArn": "arn:aws:ses:us-east-1:000000000000:identity/example.com" },
            "IssuerConfiguration": { "Type": "UPDATED" },
            "KeyConfiguration": { "KeyType": "CUSTOMER_MANAGED_KEY", "KmsKeyArn": KMS_KEY_ARN },
            "UserAttributeUpdateSettings": {
                "AttributesRequireVerificationBeforeUpdate": ["email"],
            },
            "VerificationMessageTemplate": { "DefaultEmailOption": "CONFIRM_WITH_LINK",
                                             "EmailMessageByLink": "Confirm here: {##Click here##}" },
            "UserPoolTags": { "owner": "platform" },
        }),
    )
    .await;
    let user_pool_id = pool_id(&created);

    // An update that carries none of them returns each block to the default the
    // service materialises, exactly as a create that configured none stores it.
    ok(
        &router,
        "UpdateUserPool",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await;
    let reset = described(&router, &user_pool_id).await;
    for (path, expected) in server_set_defaults() {
        assert_member("an update naming no block", "a later read", &reset, path, &expected);
    }
    // The tags are not a block an update resets: they are assigned and removed
    // by the tagging operations.
    assert_member(
        "an update naming no tag",
        "a later read",
        &reset,
        "UserPoolTags",
        &json!({ "owner": "platform" }),
    );

    // A tag assigned through the tagging operation reads back the same way, and
    // survives a further update that names no tag.
    let arn = reset["Arn"].as_str().expect("a pool carries its ARN").to_string();
    ok(
        &router,
        "TagResource",
        &json!({ "ResourceArn": arn, "Tags": { "env": "probe" } }),
    )
    .await;
    ok(
        &router,
        "UpdateUserPool",
        &json!({ "UserPoolId": user_pool_id, "DeletionProtection": "ACTIVE" }),
    )
    .await;
    assert_member(
        "a tag assigned through TagResource",
        "a later read",
        &described(&router, &user_pool_id).await,
        "UserPoolTags",
        &json!({ "owner": "platform", "env": "probe" }),
    );

    // An update that DOES name tags assigns exactly the set it names.
    ok(
        &router,
        "UpdateUserPool",
        &json!({ "UserPoolId": user_pool_id, "UserPoolTags": { "team": "auth" } }),
    )
    .await;
    assert_member(
        "an update naming its own tags",
        "a later read",
        &described(&router, &user_pool_id).await,
        "UserPoolTags",
        &json!({ "team": "auth" }),
    );
}

// ---------------------------------------------------------------------------
// The default schema
// ---------------------------------------------------------------------------

/// One attribute of the schema a new pool starts with: its name, data type and
/// the bounds the service stamps on its values, as `MinLength`/`MaxLength` for a
/// string attribute and `MinValue`/`MaxValue` for a number one. `None` means the
/// bound is left out of the attribute's constraints, and `constraints: false`
/// that the attribute reports no constraints structure at all.
///
/// The catalog is Amazon Cognito's standard user-pool attributes (the Amazon
/// Cognito Developer Guide's "User pool attributes", in the order a pool stores
/// them) with the service's own per-attribute bounds, and it closes with the
/// attribute the service maintains itself (`identities`, the rendered form of a
/// profile's links to external identity providers).
struct StandardAttribute {
    name: &'static str,
    data_type: &'static str,
    min: Option<&'static str>,
    max: Option<&'static str>,
}

const DEFAULT_SCHEMA: [StandardAttribute; 21] = [
    StandardAttribute { name: "sub", data_type: "String", min: Some("1"), max: Some("2048") },
    StandardAttribute { name: "name", data_type: "String", min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "given_name", data_type: "String", min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "family_name", data_type: "String", min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "middle_name", data_type: "String", min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "nickname", data_type: "String", min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "preferred_username", data_type: "String", min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "profile", data_type: "String", min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "picture", data_type: "String", min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "website", data_type: "String", min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "email", data_type: "String", min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "email_verified", data_type: "Boolean", min: None, max: None },
    StandardAttribute { name: "gender", data_type: "String", min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "birthdate", data_type: "String", min: Some("10"), max: Some("10") },
    StandardAttribute { name: "zoneinfo", data_type: "String", min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "locale", data_type: "String", min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "phone_number", data_type: "String", min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "phone_number_verified", data_type: "Boolean", min: None, max: None },
    StandardAttribute { name: "address", data_type: "String", min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "updated_at", data_type: "Number", min: Some("0"), max: None },
    StandardAttribute { name: "identities", data_type: "String", min: None, max: None },
];

/// The constraints structures `data_type` reports with the bounds `min`/`max`,
/// as `(NumberAttributeConstraints, StringAttributeConstraints)` — each
/// `Value::Null` where the attribute reports no such structure.
fn expected_constraints(
    data_type: &str,
    min: Option<&str>,
    max: Option<&str>,
) -> (Value, Value) {
    let mut bounds = Map::new();
    match data_type {
        "Number" => {
            if let Some(min) = min {
                bounds.insert("MinValue".to_string(), json!(min));
            }
            if let Some(max) = max {
                bounds.insert("MaxValue".to_string(), json!(max));
            }
            (Value::Object(bounds), Value::Null)
        }
        "String" => {
            if let Some(min) = min {
                bounds.insert("MinLength".to_string(), json!(min));
            }
            if let Some(max) = max {
                bounds.insert("MaxLength".to_string(), json!(max));
            }
            (Value::Null, Value::Object(bounds))
        }
        _ => (Value::Null, Value::Null),
    }
}

/// Assert the schema entry of `pool` named by `expected` carries that
/// attribute's data type and exactly the constraints structure its data type
/// has.
fn assert_schema_attribute(case: &str, pool: &Value, expected: &StandardAttribute) {
    let entry = schema_entry(case, pool, expected.name);
    assert_eq!(
        entry["AttributeDataType"],
        json!(expected.data_type),
        "case {case:?}: {} is a {} attribute, got {entry}",
        expected.name,
        expected.data_type
    );
    let (number, string) = expected_constraints(expected.data_type, expected.min, expected.max);
    for (member, want) in [
        ("NumberAttributeConstraints", number),
        ("StringAttributeConstraints", string),
    ] {
        let actual = entry.get(member);
        if want.is_null() {
            assert!(
                actual.is_none_or(Value::is_null),
                "case {case:?}: {} reports no {member}, got {actual:?}",
                expected.name
            );
        } else {
            assert_eq!(
                actual,
                Some(&want),
                "case {case:?}: {} reports {member} as {want}, got {actual:?}",
                expected.name
            );
        }
    }
}

#[tokio::test]
async fn the_default_schema_is_the_services_own_standard_attribute_set() {
    let router = router();
    let created = create(&router, &json!({ "PoolName": "default-schema" })).await;
    let user_pool_id = pool_id(&created);
    let read = described(&router, &user_pool_id).await;

    // The create answers with the schema it CONFIGURED; a read of the pool
    // reports that schema and the attribute the service maintains itself.
    let configured: Vec<String> = DEFAULT_SCHEMA
        .iter()
        .map(|attribute| attribute.name.to_string())
        .filter(|name| name != "identities")
        .collect();
    assert_eq!(
        schema_names(&created),
        configured,
        "case {:?}: a create answers with the standard attributes it stored, got {:?}",
        "the default schema",
        schema_names(&created)
    );
    let reported: Vec<String> = DEFAULT_SCHEMA
        .iter()
        .map(|attribute| attribute.name.to_string())
        .collect();
    assert_eq!(
        schema_names(&read),
        reported,
        "case {:?}: a read reports the configured attributes and the service's own, got {:?}",
        "the default schema",
        schema_names(&read)
    );

    // Every attribute of the catalog, including the ones no recorded request
    // named: each carries its own bounds rather than one generic template.
    for attribute in &DEFAULT_SCHEMA {
        assert_schema_attribute("the default schema", &read, attribute);
    }
    assert_eq!(
        schema_entry("the default schema", &read, "sub")["Required"],
        json!(true),
        "case {:?}: the minted attribute is required, got {read}",
        "the default schema"
    );

    // Neither the minted attribute nor the service-maintained one is a column of
    // the import format: no import supplies either.
    let header: Vec<String> = ok(
        &router,
        "GetCSVHeader",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await["CSVHeader"]
        .as_array()
        .unwrap_or_else(|| panic!("GetCSVHeader answers its columns"))
        .iter()
        .map(|column| column.as_str().unwrap_or_default().to_string())
        .collect();
    for absent in ["sub", "identities"] {
        assert!(
            !header.contains(&absent.to_string()),
            "case {absent:?}: no import supplies this attribute, so the header carries no column \
             for it, got {header:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The constraints an attribute of each data type reports
// ---------------------------------------------------------------------------

/// One requested attribute: what the request carries for it, and the data type
/// and bounds the stored attribute must then report. The rows span the whole
/// `AttributeDataType` enum (`String | Number | DateTime | Boolean`, from
/// `smithy_query(query="shape",shape="AttributeDataType")`), each asked for both
/// with bounds and without.
struct AttributeCase {
    case: &'static str,
    requested: fn() -> Value,
    expected: StandardAttribute,
}

fn attribute_cases() -> Vec<AttributeCase> {
    vec![
        AttributeCase {
            case: "a string attribute with no bounds at all",
            requested: || json!({ "Name": "plain", "AttributeDataType": "String" }),
            expected: StandardAttribute {
                name: "custom:plain",
                data_type: "String",
                min: None,
                max: None,
            },
        },
        AttributeCase {
            case: "a string attribute bounded at both ends",
            requested: || {
                json!({
                    "Name": "bounded",
                    "AttributeDataType": "String",
                    "StringAttributeConstraints": { "MinLength": "2", "MaxLength": "32" },
                })
            },
            expected: StandardAttribute {
                name: "custom:bounded",
                data_type: "String",
                min: Some("2"),
                max: Some("32"),
            },
        },
        AttributeCase {
            case: "a string attribute bounded from below only",
            requested: || {
                json!({
                    "Name": "lower",
                    "AttributeDataType": "String",
                    "StringAttributeConstraints": { "MinLength": "1" },
                })
            },
            expected: StandardAttribute {
                name: "custom:lower",
                data_type: "String",
                min: Some("1"),
                max: None,
            },
        },
        AttributeCase {
            case: "an attribute whose data type the request leaves out",
            requested: || json!({ "Name": "untyped" }),
            expected: StandardAttribute {
                name: "custom:untyped",
                data_type: "String",
                min: None,
                max: None,
            },
        },
        AttributeCase {
            case: "a number attribute with no bounds at all",
            requested: || json!({ "Name": "count", "AttributeDataType": "Number" }),
            expected: StandardAttribute {
                name: "custom:count",
                data_type: "Number",
                min: None,
                max: None,
            },
        },
        AttributeCase {
            case: "a number attribute bounded at both ends",
            requested: || {
                json!({
                    "Name": "seats",
                    "AttributeDataType": "Number",
                    "NumberAttributeConstraints": { "MinValue": "0", "MaxValue": "99" },
                })
            },
            expected: StandardAttribute {
                name: "custom:seats",
                data_type: "Number",
                min: Some("0"),
                max: Some("99"),
            },
        },
        AttributeCase {
            case: "a boolean attribute, whose values no bound describes",
            requested: || json!({ "Name": "member", "AttributeDataType": "Boolean" }),
            expected: StandardAttribute {
                name: "custom:member",
                data_type: "Boolean",
                min: None,
                max: None,
            },
        },
        AttributeCase {
            case: "a date-time attribute, whose values no bound describes",
            requested: || json!({ "Name": "joined", "AttributeDataType": "DateTime" }),
            expected: StandardAttribute {
                name: "custom:joined",
                data_type: "DateTime",
                min: None,
                max: None,
            },
        },
    ]
}

#[tokio::test]
async fn an_attribute_reports_the_constraints_of_its_own_data_type() {
    let router = router();

    for (index, row) in attribute_cases().iter().enumerate() {
        // The same attribute asked for in the two ways a pool's schema grows:
        // in the create's own `Schema`, and through a later add.
        let created = create(
            &router,
            &json!({
                "PoolName": format!("attribute-create-{index}"),
                "Schema": [(row.requested)()],
            }),
        )
        .await;
        let user_pool_id = pool_id(&created);
        assert_schema_attribute(row.case, &created, &row.expected);
        assert_schema_attribute(
            row.case,
            &described(&router, &user_pool_id).await,
            &row.expected,
        );

        let added = pool_id(&create(&router, &json!({ "PoolName": format!("attribute-add-{index}") })).await);
        ok(
            &router,
            "AddCustomAttributes",
            &json!({ "UserPoolId": added, "CustomAttributes": [(row.requested)()] }),
        )
        .await;
        assert_schema_attribute(row.case, &described(&router, &added).await, &row.expected);
    }
}

// ---------------------------------------------------------------------------
// The listing's own view of the stored pool
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_listing_entry_reports_the_trigger_configuration_and_the_pools_regions() {
    let router = router();
    // The rows differ along the configuration each pool carries, so an entry is
    // checked for a pool with triggers, one with none and one carrying other
    // configuration entirely.
    let pools = [
        ("a pool with nothing but its name", json!({ "PoolName": "listed-plain" })),
        (
            "a pool carrying two Lambda triggers",
            json!({
                "PoolName": "listed-triggers",
                "LambdaConfig": {
                    "PreSignUp": TRIGGER_PRE_SIGN_UP,
                    "PostConfirmation": TRIGGER_PRE_SIGN_UP,
                },
            }),
        ),
        (
            "a pool configured well past its defaults",
            json!({
                "PoolName": "listed-configured",
                "MfaConfiguration": "OPTIONAL",
                "UserPoolTags": { "owner": "listing" },
            }),
        ),
    ];

    let mut created: Vec<(&str, String)> = Vec::new();
    for (case, request) in &pools {
        created.push((case, pool_id(&create(&router, request).await)));
    }

    let listed = ok(&router, "ListUserPools", &json!({ "MaxResults": 60 })).await;
    let entries = listed["UserPools"]
        .as_array()
        .unwrap_or_else(|| panic!("ListUserPools answers a list, got {listed}"))
        .clone();
    for (case, user_pool_id) in &created {
        let entry = entries
            .iter()
            .find(|entry| entry["Id"] == json!(user_pool_id))
            .unwrap_or_else(|| panic!("case {case:?}: the created pool must be listed, got {listed}"));
        let read = described(&router, user_pool_id).await;
        assert_member(case, "the listing entry", entry, "LambdaConfig", &read["LambdaConfig"]);
        // Every pool holds its PRIMARY replica in the Region it was created in,
        // so a pool nothing was replicated to reports that one Region.
        assert_member(
            case,
            "the listing entry",
            entry,
            "ReplicaRegions",
            &json!([REGION]),
        );
    }

    // A pool replicated to another Region reports both Regions, so the member is
    // computed from the pool's replicas rather than fixed. The pool is encrypted
    // under a customer-managed key, which is what a pool has to hold to be
    // replicated at all: "you must use a multi-region key to enable
    // multi-region replication for a user pool"
    // (`smithy_query(query="doc",shape="KeyConfigurationType",member="KeyType")`).
    let replicated = pool_id(
        &create(
            &router,
            &json!({
                "PoolName": "listed-replicated",
                "KeyConfiguration": {
                    "KeyType": "CUSTOMER_MANAGED_KEY",
                    "KmsKeyArn": "arn:aws:kms:us-east-1:123456789012:key/multi-region-example",
                },
            }),
        )
        .await,
    );
    ok(
        &router,
        "CreateUserPoolReplica",
        &json!({ "UserPoolId": replicated, "RegionName": "us-west-2" }),
    )
    .await;
    let listed = ok(&router, "ListUserPools", &json!({ "MaxResults": 60 })).await;
    let entry = listed["UserPools"]
        .as_array()
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry["Id"] == json!(replicated))
                .cloned()
        })
        .unwrap_or_else(|| panic!("the replicated pool must be listed, got {listed}"));
    assert_member(
        "a pool replicated to a second Region",
        "the listing entry",
        &entry,
        "ReplicaRegions",
        &json!([REGION, "us-west-2"]),
    );
}
