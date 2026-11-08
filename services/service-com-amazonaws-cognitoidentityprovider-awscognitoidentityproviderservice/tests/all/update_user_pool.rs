// SPDX-License-Identifier: CC0-1.0

//! `UpdateUserPool` over the wire, driven through the router exactly as an SDK
//! drives it (`POST /` with `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`),
//! and read back through `DescribeUserPool`.
//!
//! The rule under test: `UpdateUserPool` REPLACES the updatable configuration of
//! the pool it names — a member the request carries is stored as sent, and a
//! member the request leaves out goes back to the service default instead of
//! keeping the value the pool held ("If you don't provide a value for an
//! attribute, Amazon Cognito sets it to its default value", this operation's
//! documentation in the service model). The pool's identity (`Id`, `Arn`,
//! `CreationDate`) and the members this operation's input does NOT model
//! (`SchemaAttributes`, `UsernameAttributes`) survive the update, and
//! `LastModifiedDate` advances.
//!
//! Each table row names the case it asserts and every assertion reports the
//! actual value.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::Duration;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// A `PreSignUp` trigger ARN, in the form the recorded CLI call uses.
const TRIGGER_PRE_SIGN_UP: &str = "arn:aws:lambda:us-east-1:000000000000:function:pre-sign-up";

/// A second trigger ARN, so a row can carry more than one trigger.
const TRIGGER_POST_CONFIRMATION: &str =
    "arn:aws:lambda:us-east-1:000000000000:function:post-confirmation";

/// The name every case's pool is created with.
const CREATED_POOL_NAME: &str = "update-rule-pool";

/// The Amazon SNS caller a pool's SMS messages are sent through
/// (`SmsConfigurationType.SnsCallerArn`).
const SNS_CALLER_ARN: &str = "arn:aws:iam::000000000000:role/sms-caller";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::EmulatedService::new(engine),
    );
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

/// The `__type` of an error body, which is the wire error code.
fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// A `CreateUserPool` request that sets every member `UpdateUserPool` can reset,
/// so each case can observe whether the update kept it or returned it to its
/// default. `Schema` and `UsernameAttributes` are NOT `UpdateUserPool` input
/// members, so they are here to be checked for survival.
fn rich_create_request() -> Value {
    json!({
        "PoolName": CREATED_POOL_NAME,
        "Policies": {
            "PasswordPolicy": {
                "MinimumLength": 12,
                "RequireUppercase": true,
                "RequireLowercase": true,
                "RequireNumbers": true,
                "RequireSymbols": true,
                "TemporaryPasswordValidityDays": 3
            }
        },
        "AdminCreateUserConfig": { "AllowAdminCreateUserOnly": true, "UnusedAccountValidityDays": 2 },
        "AutoVerifiedAttributes": ["email"],
        "MfaConfiguration": "OPTIONAL",
        "DeletionProtection": "ACTIVE",
        "UserPoolTier": "PLUS",
        "LambdaConfig": { "PreSignUp": TRIGGER_PRE_SIGN_UP },
        "DeviceConfiguration": {
            "ChallengeRequiredOnNewDevice": true,
            "DeviceOnlyRememberedOnUserPrompt": true
        },
        "AccountRecoverySetting": {
            "RecoveryMechanisms": [{ "Name": "verified_phone_number", "Priority": 1 }]
        },
        "SmsAuthenticationMessage": "Created code {####}",
        "UserPoolTags": { "env": "created" },
        "Schema": [{ "Name": "department", "AttributeDataType": "String" }],
        "UsernameAttributes": ["email"]
    })
}

/// Create a pool from [`rich_create_request`] and return the `CreateUserPool`
/// response body.
async fn created_pool(router: &MultiServiceRouter) -> Value {
    ok(router, "CreateUserPool", &rich_create_request()).await["UserPool"].clone()
}

/// The pool `user_pool_id` names, as `DescribeUserPool` renders it.
async fn described_pool(router: &MultiServiceRouter, user_pool_id: &str) -> Value {
    ok(
        router,
        "DescribeUserPool",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await["UserPool"]
        .clone()
}

/// The value at a dotted path inside `value`, or `None` when any step is absent.
fn at<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for step in path.split('.') {
        current = current.get(step)?;
    }
    Some(current)
}

/// Assert the rendered pool carries `expected` at `path`. A `Value::Null`
/// expectation means the member must be ABSENT.
fn assert_member(case: &str, pool: &Value, path: &str, expected: &Value) {
    let actual = at(pool, path);
    if expected.is_null() {
        assert!(
            actual.is_none_or(Value::is_null),
            "case {case:?}: {path} must be absent after the update, got {actual:?}"
        );
    } else {
        assert_eq!(
            actual,
            Some(expected),
            "case {case:?}: {path} must be {expected}, got {actual:?}"
        );
    }
}

/// `pool` without the one member an update is expected to move, so two renders
/// of the same configuration can be compared.
fn without_last_modified(mut pool: Value) -> Value {
    if let Some(members) = pool.as_object_mut() {
        members.remove("LastModifiedDate");
    }
    pool
}

/// `pool`'s `LastModifiedDate`, as the seconds-since-epoch number the modelled
/// timestamp serialises to.
fn last_modified(case: &str, pool: &Value) -> f64 {
    pool["LastModifiedDate"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: a pool must carry a LastModifiedDate: {pool}"))
}

/// One row of the `UpdateUserPool` table: the members the update carries beyond
/// `UserPoolId`, and what the pool must render afterwards. Rows differ along what
/// the rule generalises over — which members are present, how many entries a map
/// or list member carries, and the form of each value.
struct UpdateCase {
    case: &'static str,
    request: Value,
    expected: Vec<(&'static str, Value)>,
}

/// The members every row that sets none of them expects back at their defaults,
/// which `src/state.rs` derives from the model's documented defaults.
fn reset_to_defaults() -> Vec<(&'static str, Value)> {
    vec![
        // A pool always applies a trigger configuration, so an update that
        // carries none leaves the pool with the EMPTY one rather than with
        // nothing.
        ("LambdaConfig", json!({})),
        ("DeviceConfiguration", Value::Null),
        // An omitted `AccountRecoverySetting` goes back to the service default,
        // which is the configuration the pool then applies.
        (
            "AccountRecoverySetting.RecoveryMechanisms",
            json!([
                { "Priority": 1, "Name": "verified_email" },
                { "Priority": 2, "Name": "verified_phone_number" },
            ]),
        ),
        ("SmsAuthenticationMessage", Value::Null),
        ("AutoVerifiedAttributes", Value::Null),
        // Tags are assigned and removed through the tagging operations, so an
        // update that names none leaves the pool's tags as they are.
        ("UserPoolTags", json!({ "env": "created" })),
        ("MfaConfiguration", json!("OFF")),
        ("DeletionProtection", json!("INACTIVE")),
        ("UserPoolTier", json!("ESSENTIALS")),
        ("Policies.PasswordPolicy.MinimumLength", json!(8)),
        (
            "Policies.PasswordPolicy.TemporaryPasswordValidityDays",
            json!(7),
        ),
        (
            "Policies.SignInPolicy.AllowedFirstAuthFactors",
            json!(["PASSWORD"]),
        ),
        (
            "AdminCreateUserConfig.UnusedAccountValidityDays",
            json!(7),
        ),
        ("Name", json!(CREATED_POOL_NAME)),
    ]
}

/// The defaults with the entry for each `path` this row sets replaced by its
/// expectation. An override of a nested path (`A.B`) also drops the default
/// expectation for the member that holds it (`A`), which the row is replacing.
fn overriding(overrides: Vec<(&'static str, Value)>) -> Vec<(&'static str, Value)> {
    let mut expected = reset_to_defaults();
    for (path, value) in overrides {
        expected.retain(|(existing, _)| {
            *existing != path && !path.starts_with(&format!("{existing}."))
        });
        expected.push((path, value));
    }
    expected
}

/// The table: each row is one `UpdateUserPool` request against a pool created
/// from [`rich_create_request`].
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "nothing but the pool id: every updatable member goes back to its default",
            request: json!({}),
            expected: reset_to_defaults(),
        },
        UpdateCase {
            case: "one Lambda trigger only, as the recorded CLI call sends it",
            request: json!({ "LambdaConfig": { "PreSignUp": TRIGGER_PRE_SIGN_UP } }),
            expected: overriding(vec![(
                "LambdaConfig",
                json!({ "PreSignUp": TRIGGER_PRE_SIGN_UP }),
            )]),
        },
        UpdateCase {
            case: "two Lambda triggers and two tags: a map member is stored whole",
            request: json!({
                "LambdaConfig": {
                    "PreSignUp": TRIGGER_PRE_SIGN_UP,
                    "PostConfirmation": TRIGGER_POST_CONFIRMATION
                },
                "UserPoolTags": { "env": "updated", "team": "auth" }
            }),
            expected: overriding(vec![
                (
                    "LambdaConfig",
                    json!({
                        "PreSignUp": TRIGGER_PRE_SIGN_UP,
                        "PostConfirmation": TRIGGER_POST_CONFIRMATION
                    }),
                ),
                (
                    "UserPoolTags",
                    json!({ "env": "updated", "team": "auth" }),
                ),
            ]),
        },
        UpdateCase {
            case: "PoolName only: the name is the member that changes",
            request: json!({ "PoolName": "renamed-pool" }),
            expected: overriding(vec![("Name", json!("renamed-pool"))]),
        },
        UpdateCase {
            case: "policies, MFA mode, deletion protection and feature plan",
            request: json!({
                "Policies": {
                    "PasswordPolicy": {
                        "MinimumLength": 6,
                        "RequireUppercase": true,
                        "RequireLowercase": true,
                        "RequireNumbers": true,
                        "RequireSymbols": true,
                        "TemporaryPasswordValidityDays": 1
                    },
                    "SignInPolicy": { "AllowedFirstAuthFactors": ["PASSWORD", "WEB_AUTHN"] }
                },
                "MfaConfiguration": "ON",
                "DeletionProtection": "ACTIVE",
                // The plan the row leaves the pool on has to include the
                // sign-in policy it configures: a first authentication factor
                // beyond username and password needs "the Essentials tier or
                // higher" (`SignInPolicyType` in this service's model).
                "UserPoolTier": "ESSENTIALS"
            }),
            expected: overriding(vec![
                ("Policies.PasswordPolicy.MinimumLength", json!(6)),
                (
                    "Policies.PasswordPolicy.TemporaryPasswordValidityDays",
                    json!(1),
                ),
                (
                    "Policies.SignInPolicy.AllowedFirstAuthFactors",
                    json!(["PASSWORD", "WEB_AUTHN"]),
                ),
                ("MfaConfiguration", json!("ON")),
                ("DeletionProtection", json!("ACTIVE")),
                ("UserPoolTier", json!("ESSENTIALS")),
            ]),
        },
        UpdateCase {
            case: "two auto-verified attributes, a device configuration and an SMS message",
            request: json!({
                "AutoVerifiedAttributes": ["email", "phone_number"],
                // Auto-verifying `phone_number` sends that verification over SMS,
                // so the update carries the Amazon SNS caller SMS is sent through:
                // the pairing is refused without it.
                "SmsConfiguration": { "SnsCallerArn": SNS_CALLER_ARN },
                "DeviceConfiguration": { "ChallengeRequiredOnNewDevice": true },
                "SmsAuthenticationMessage": "Updated code {####}"
            }),
            expected: overriding(vec![
                ("AutoVerifiedAttributes", json!(["email", "phone_number"])),
                ("SmsConfiguration.SnsCallerArn", json!(SNS_CALLER_ARN)),
                (
                    "DeviceConfiguration.ChallengeRequiredOnNewDevice",
                    json!(true),
                ),
                (
                    "SmsAuthenticationMessage",
                    json!("Updated code {####}"),
                ),
            ]),
        },
        UpdateCase {
            case: "account recovery and admin-create-user settings",
            request: json!({
                "AccountRecoverySetting": {
                    "RecoveryMechanisms": [
                        { "Name": "verified_email", "Priority": 1 },
                        { "Name": "verified_phone_number", "Priority": 2 }
                    ]
                },
                "AdminCreateUserConfig": {
                    "AllowAdminCreateUserOnly": true,
                    "UnusedAccountValidityDays": 5
                }
            }),
            expected: overriding(vec![
                (
                    "AccountRecoverySetting.RecoveryMechanisms",
                    json!([
                        { "Name": "verified_email", "Priority": 1 },
                        { "Name": "verified_phone_number", "Priority": 2 }
                    ]),
                ),
                ("AdminCreateUserConfig.UnusedAccountValidityDays", json!(5)),
                ("AdminCreateUserConfig.AllowAdminCreateUserOnly", json!(true)),
            ]),
        },
    ]
}

#[tokio::test]
async fn an_update_replaces_every_member_this_operation_models() {
    let router = router();
    for case in update_cases() {
        let created = created_pool(&router).await;
        let user_pool_id = created["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: a minted pool id: {created}", case.case))
            .to_string();

        // The create really did set what this case expects to be replaced or reset.
        assert_eq!(
            at(&created, "Policies.PasswordPolicy.MinimumLength"),
            Some(&json!(12)),
            "case {:?}: the created pool must carry the policy the create set, got {created}",
            case.case
        );
        assert_eq!(
            at(&created, "LambdaConfig.PreSignUp"),
            Some(&json!(TRIGGER_PRE_SIGN_UP)),
            "case {:?}: the created pool must carry the trigger the create set, got {created}",
            case.case
        );

        let mut request = case.request.clone();
        request["UserPoolId"] = json!(user_pool_id);
        // The pool as a read saw it BEFORE the update, which is what the members
        // this operation's input does not model are checked against.
        let before_update = described_pool(&router, &user_pool_id).await;
        let answered = ok(&router, "UpdateUserPool", &request).await;
        assert!(
            answered.is_null() || answered.as_object().is_some_and(serde_json::Map::is_empty),
            "case {:?}: UpdateUserPoolResponse models no members, got {answered}",
            case.case
        );

        let updated = described_pool(&router, &user_pool_id).await;
        for (path, expected) in &case.expected {
            assert_member(case.case, &updated, path, expected);
        }

        // The pool's identity and the members this operation's input does not
        // model survive the update.
        for member in ["Id", "Arn", "CreationDate", "SchemaAttributes", "UsernameAttributes"] {
            assert_eq!(
                updated.get(member),
                before_update.get(member),
                "case {:?}: {member} must survive an update, got {:?}",
                case.case,
                updated.get(member)
            );
        }

        // Replacing is idempotent: the same request again renders the same pool.
        ok(&router, "UpdateUserPool", &request).await;
        let again = described_pool(&router, &user_pool_id).await;
        assert_eq!(
            without_last_modified(again.clone()),
            without_last_modified(updated.clone()),
            "case {:?}: the same update applied twice must render the same pool, got {again}",
            case.case
        );
    }
}

#[tokio::test]
async fn an_update_advances_the_pools_last_modified_instant() {
    let router = router();
    let created = created_pool(&router).await;
    let user_pool_id = created["Id"].as_str().expect("a minted pool id").to_string();
    let case = "last modified advances";

    let before = last_modified(case, &created);
    tokio::time::sleep(Duration::from_millis(5)).await;
    ok(
        &router,
        "UpdateUserPool",
        &json!({ "UserPoolId": user_pool_id, "PoolName": "advanced-pool" }),
    )
    .await;
    let after_first = described_pool(&router, &user_pool_id).await;
    let first = last_modified(case, &after_first);
    assert!(
        first > before,
        "case {case:?}: an update must advance LastModifiedDate past {before}, got {first}"
    );

    tokio::time::sleep(Duration::from_millis(5)).await;
    ok(
        &router,
        "UpdateUserPool",
        &json!({ "UserPoolId": user_pool_id, "PoolName": "advanced-pool" }),
    )
    .await;
    let second = last_modified(case, &described_pool(&router, &user_pool_id).await);
    assert!(
        second > first,
        "case {case:?}: a second update must advance LastModifiedDate past {first}, got {second}"
    );

    // A read does not move it.
    let read_again = last_modified(case, &described_pool(&router, &user_pool_id).await);
    assert_eq!(
        read_again, second,
        "case {case:?}: DescribeUserPool must not move LastModifiedDate, got {read_again}"
    );
}

#[tokio::test]
async fn resetting_deletion_protection_is_observable_through_delete_user_pool() {
    let router = router();
    // The pool is created WITH deletion protection, which `DeleteUserPool`
    // refuses to delete through.
    let created = created_pool(&router).await;
    let user_pool_id = created["Id"].as_str().expect("a minted pool id").to_string();
    let (status, payload) = call(
        &router,
        "DeleteUserPool",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"protected pool\": DeleteUserPool must refuse a protected pool, got {status} with {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "InvalidParameterException",
        "case \"protected pool\": got {payload}"
    );

    // An update that does not carry DeletionProtection resets it to INACTIVE,
    // and the pool becomes deletable.
    ok(
        &router,
        "UpdateUserPool",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await;
    let described = described_pool(&router, &user_pool_id).await;
    assert_eq!(
        described["DeletionProtection"],
        json!("INACTIVE"),
        "case \"protection reset\": got {described}"
    );
    ok(
        &router,
        "DeleteUserPool",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await;

    // And the pool is gone, so the next update is a 400.
    let (status, payload) = call(
        &router,
        "UpdateUserPool",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"deleted pool\": got {status} with {payload}"
    );
}

#[tokio::test]
async fn an_update_of_an_absent_pool_is_a_400() {
    let router = router();
    // Ids no pool was created under, each satisfying the modelled
    // `UserPoolIdType` pattern `^[\w-]+_[0-9a-zA-Z]+$` in a different form.
    let absent_ids = [
        ("another region", "us-west-2_39e6f417e"),
        ("an upper-case suffix", "eu-central-1_ABCDEFGHI"),
        ("a single-character suffix", "us-east-1_0"),
        ("an underscore-only prefix", "pool_abc123"),
    ];
    // A live pool, so the refusal is about the id and not about an empty store.
    let live = created_pool(&router).await;
    let live_id = live["Id"].as_str().expect("a minted pool id").to_string();

    for (case, user_pool_id) in absent_ids {
        let (status, payload) = call(
            &router,
            "UpdateUserPool",
            &json!({ "UserPoolId": user_pool_id, "PoolName": "no-such-pool" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an absent pool must answer 400, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ResourceNotFoundException",
            "case {case:?}: got {payload}"
        );
    }

    // The refused updates changed nothing about the pool that does exist.
    let described = described_pool(&router, &live_id).await;
    assert_eq!(
        described["Name"],
        json!(CREATED_POOL_NAME),
        "case \"live pool untouched\": got {described}"
    );
}
