// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the account's own settings: `GetAccount` and `UpdateAccount`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns.
//!
//! The RULE under test: `GetAccount` reports the account record, and the only member of
//! it a caller sets is the CloudWatch role, which `UpdateAccount`'s `patchOperations`
//! write and a later `GetAccount` reads back unchanged. The rows span what that
//! generalises over: each patch op the member takes (`replace`, `add`, `remove`), a
//! sequence of them, the members no patch may name, the values that are not an IAM role
//! ARN, and the read before anything was ever written.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    API_KEY_VERSION, DEFAULT_ACCOUNT_THROTTLE, USAGE_PLANS_FEATURE,
};

use crate::rest_api_lifecycle::Gateway;

/// `GetAccount`.
async fn get_account(gateway: &Gateway) -> Value {
    let answer = gateway.call("GET", "/account", None).await;
    assert_eq!(
        answer.status, 200,
        "case GetAccount: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// `UpdateAccount` with one patch operation.
async fn patch_account(gateway: &Gateway, op: &str, path: &str, value: Option<&str>) -> Value {
    let mut operation = json!({ "op": op, "path": path });
    if let (Some(target), Some(value)) = (operation.as_object_mut(), value) {
        target.insert("value".to_string(), Value::String(value.to_string()));
    }
    let answer = gateway
        .call(
            "PATCH",
            "/account",
            Some(json!({ "patchOperations": [operation] })),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case UpdateAccount({op} {path}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// An IAM role ARN of the kind API Gateway's account-level CloudWatch role is.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::000000000000:role/{name}")
}

#[tokio::test]
async fn the_account_settings_a_patch_writes_are_what_a_read_reports() {
    let gateway = Gateway::new();

    // An account nobody has configured carries the service's own values and NO
    // CloudWatch role, which is what distinguishes it from a configured one.
    let initial = get_account(&gateway).await;
    assert_eq!(
        initial["cloudwatchRoleArn"],
        Value::Null,
        "case an unconfigured account: expected no cloudwatchRoleArn, got {initial:?}"
    );
    assert_eq!(
        initial["throttleSettings"]["burstLimit"],
        json!(DEFAULT_ACCOUNT_THROTTLE.0),
        "case an unconfigured account: expected the account burst limit, got {initial:?}"
    );
    assert_eq!(
        initial["throttleSettings"]["rateLimit"],
        json!(DEFAULT_ACCOUNT_THROTTLE.1),
        "case an unconfigured account: expected the account rate limit, got {initial:?}"
    );
    assert_eq!(
        initial["features"],
        json!([USAGE_PLANS_FEATURE]),
        "case an unconfigured account: expected the usage-plan feature entry, got {initial:?}"
    );
    assert_eq!(
        initial["apiKeyVersion"],
        json!(API_KEY_VERSION),
        "case an unconfigured account: expected the account's API key version, got {initial:?}"
    );

    // Each patch op the member takes, in sequence: every row asserts the value the
    // UPDATE reported and the value a following READ reports, so nothing is echoed from
    // the request alone.
    // (case, op, value, the role the account then has)
    let rows: &[(&str, &str, Option<&str>, Option<&str>)] = &[
        (
            "replace sets the role on an account that has none",
            "replace",
            Some("first"),
            Some("first"),
        ),
        (
            "replace again points it at another role",
            "replace",
            Some("second"),
            Some("second"),
        ),
        (
            "add sets the single-valued member too",
            "add",
            Some("third"),
            Some("third"),
        ),
        ("remove clears it", "remove", None, None),
        (
            "replace sets it again after a remove",
            "replace",
            Some("fourth"),
            Some("fourth"),
        ),
    ];
    for (case, op, value, expected) in rows {
        let expected = expected.map(role_arn);
        let expected = expected
            .as_deref()
            .map_or(Value::Null, |arn| Value::String(arn.to_string()));
        let updated = patch_account(
            &gateway,
            op,
            "/cloudwatchRoleArn",
            value.map(role_arn).as_deref(),
        )
        .await;
        assert_eq!(
            updated["cloudwatchRoleArn"], expected,
            "case {case:?}: expected UpdateAccount to report {expected:?}, got {updated:?}"
        );
        let read = get_account(&gateway).await;
        assert_eq!(
            read["cloudwatchRoleArn"], expected,
            "case {case:?}: expected GetAccount to read back {expected:?}, got {read:?}"
        );
        // The service's own members are untouched by a patch of the caller's member.
        assert_eq!(
            read["apiKeyVersion"],
            json!(API_KEY_VERSION),
            "case {case:?}: expected the API key version to be unchanged, got {read:?}"
        );
    }
}

#[tokio::test]
async fn a_patch_of_a_member_the_caller_does_not_own_is_refused_and_writes_nothing() {
    let gateway = Gateway::new();
    patch_account(
        &gateway,
        "replace",
        "/cloudwatchRoleArn",
        Some(&role_arn("configured")),
    )
    .await;

    // Every other member of `Account` is API Gateway's own, so no patch may name it;
    // neither may an op the surface does not support, nor a path that names nothing.
    // (case, op, path, value)
    let rows: &[(&str, &str, &str, Option<&str>)] = &[
        ("the account's throttle settings", "replace", "/throttleSettings/rateLimit", Some("1")),
        ("the account's features", "add", "/features", Some("UsagePlans")),
        ("the account's API key version", "replace", "/apiKeyVersion", Some("5")),
        ("a member no shape declares", "replace", "/notAMember", Some("x")),
        ("an unsupported op", "copy", "/cloudwatchRoleArn", Some("x")),
        (
            // The member names the IAM role API Gateway assumes, so a value that is not
            // one is refused rather than stored.
            "a cloudwatchRoleArn that is not an ARN at all",
            "replace",
            "/cloudwatchRoleArn",
            Some("not-an-arn"),
        ),
        (
            "a cloudwatchRoleArn naming an IAM resource that is not a role",
            "replace",
            "/cloudwatchRoleArn",
            Some("arn:aws:iam::000000000000:user/bob"),
        ),
        (
            "a cloudwatchRoleArn of another service",
            "replace",
            "/cloudwatchRoleArn",
            Some("arn:aws:logs:us-east-1:000000000000:log-group:/aws/apigateway/welcome"),
        ),
    ];
    for (case, op, path, value) in rows {
        let mut operation = json!({ "op": op, "path": path });
        if let (Some(target), Some(value)) = (operation.as_object_mut(), value) {
            target.insert("value".to_string(), Value::String((*value).to_string()));
        }
        let answer = gateway
            .call(
                "PATCH",
                "/account",
                Some(json!({ "patchOperations": [operation] })),
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("BadRequestException"),
            "case {case:?}: expected BadRequestException, got {:?} with {:?}",
            answer.error,
            answer.body
        );
        // The refused patch left the record as it was.
        let read = get_account(&gateway).await;
        assert_eq!(
            read["cloudwatchRoleArn"],
            json!(role_arn("configured")),
            "case {case:?}: expected the stored role to be untouched, got {read:?}"
        );
    }
}
