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
//!
//! `GetAccount` is also a REPORTING operation, so the invariant
//! [`a_read_of_the_account_reports_the_stored_record_and_changes_nothing`] pins holds
//! over every one of those states: however often the record is read, and whatever else
//! the caller does to the service in between, each read answers the same body — the one
//! the last `UpdateAccount` reported.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    API_KEY_VERSION, DEFAULT_ACCOUNT_THROTTLE, USAGE_PLANS_FEATURE,
};

use crate::rest_api_lifecycle::{create_api, Gateway};

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

/// One patch of the account's CloudWatch role: the `op` and the value it carries
/// (absent for `remove`, which takes none).
type RolePatch<'a> = (&'a str, Option<&'a str>);

/// One state of the account record: the case name, the patch sequence that puts the
/// record in it, and the CloudWatch role the record then carries.
type AccountState<'a> = (&'a str, &'a [RolePatch<'a>], Option<&'a str>);

#[tokio::test]
async fn a_read_of_the_account_reports_the_stored_record_and_changes_nothing() {
    // The states a caller can put the one account record in. `GetAccount` takes no
    // input, so this IS the whole input space of the read: the stored record.
    let states: &[AccountState<'_>] = &[
        ("an account nobody has configured", &[], None),
        (
            "an account configured once",
            &[("replace", Some("logging"))],
            Some("logging"),
        ),
        (
            "an account pointed at another role",
            &[("replace", Some("first")), ("replace", Some("second"))],
            Some("second"),
        ),
        (
            "an account whose role was added",
            &[("add", Some("added"))],
            Some("added"),
        ),
        (
            "an account whose role was removed",
            &[("replace", Some("first")), ("remove", None)],
            None,
        ),
        (
            "an account reconfigured after a remove",
            &[
                ("replace", Some("first")),
                ("remove", None),
                ("replace", Some("again")),
            ],
            Some("again"),
        ),
    ];
    // How many times in a row the record is read. A reporting operation answers the same
    // body however often it is called, so the count is what the rule generalises over.
    let read_counts = [1_usize, 2, 3, 5];

    for (case, patches, expected_role) in states {
        let expected_role = *expected_role;
        for reads in read_counts {
            let gateway = Gateway::new();
            // The body the LAST patch reported, so the read can be held against it: the
            // update and the read render one record, and must not drift.
            let mut reported: Option<Value> = None;
            for (op, value) in *patches {
                reported = Some(
                    patch_account(
                        &gateway,
                        op,
                        "/cloudwatchRoleArn",
                        value.map(role_arn).as_deref(),
                    )
                    .await,
                );
            }

            let expected = expected_role
                .map(role_arn)
                .map_or(Value::Null, Value::String);
            let first = get_account(&gateway).await;
            assert_eq!(
                first["cloudwatchRoleArn"], expected,
                "case {case:?} with {reads} read(s): expected cloudwatchRoleArn {expected:?}, \
                 got {first:?}"
            );
            // Every other member of `Account` is API Gateway's own, so it is reported on
            // every state of the record, configured or not.
            assert_eq!(
                first["throttleSettings"]["burstLimit"],
                json!(DEFAULT_ACCOUNT_THROTTLE.0),
                "case {case:?} with {reads} read(s): expected the account burst limit, \
                 got {first:?}"
            );
            assert_eq!(
                first["throttleSettings"]["rateLimit"],
                json!(DEFAULT_ACCOUNT_THROTTLE.1),
                "case {case:?} with {reads} read(s): expected the account rate limit, \
                 got {first:?}"
            );
            assert_eq!(
                first["features"],
                json!([USAGE_PLANS_FEATURE]),
                "case {case:?} with {reads} read(s): expected the usage-plan feature entry, \
                 got {first:?}"
            );
            assert_eq!(
                first["apiKeyVersion"],
                json!(API_KEY_VERSION),
                "case {case:?} with {reads} read(s): expected the account's API key version, \
                 got {first:?}"
            );

            for read in 1..reads {
                // A mutation of an UNRELATED record between two reads: the account's own
                // settings are no part of it, so the next read answers what the first did.
                create_api(&gateway, &format!("account-read-{reads}-{read}")).await;
                let again = get_account(&gateway).await;
                assert_eq!(
                    again, first,
                    "case {case:?} read {read} of {reads}: expected the same body as the \
                     first read {first:?}, got {again:?}"
                );
            }

            if let Some(reported) = reported {
                assert_eq!(
                    reported, first,
                    "case {case:?} with {reads} read(s): expected a read to report the body \
                     UpdateAccount reported {reported:?}, got {first:?}"
                );
            }
        }
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
