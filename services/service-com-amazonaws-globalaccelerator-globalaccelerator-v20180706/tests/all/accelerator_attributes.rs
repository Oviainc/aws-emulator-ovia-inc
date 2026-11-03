// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for an accelerator's flow-log attributes, driven over the wire.
//!
//! Every call goes through the real router, using the harness in
//! [`super::accelerator_lifecycle`] (`POST /` with
//! `X-Amz-Target: GlobalAccelerator_V20180706.<Operation>`, `awsJson1_1` body), so these
//! exercise routing, deserialization, constraint validation, the handler and the response
//! envelope — not just the handler function.
//!
//! The rules under test are stated once each, as a table or an invariant over a SET of
//! inputs; a recorded request is one row.
//!
//! A reference to an accelerator this store does not hold is answered at HTTP 400 — the
//! status this service answers that error class with, pinned by
//! [`super::not_found_error_status`].

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::accelerator_lifecycle::{arn_of, call, create_accelerator, emulator, error_code, ok};
use super::input_refusals::error_message;

/// `DescribeAcceleratorAttributes` on `accelerator_arn`, requiring a 200.
async fn describe(router: &MultiServiceRouter, accelerator_arn: &str) -> Value {
    ok(
        router,
        "DescribeAcceleratorAttributes",
        json!({ "AcceleratorArn": accelerator_arn }),
    )
    .await["AcceleratorAttributes"]
        .clone()
}

/// `UpdateAcceleratorAttributes` on `accelerator_arn` with the members a row names.
async fn update(
    router: &MultiServiceRouter,
    accelerator_arn: &str,
    members: &Value,
) -> (u16, Value) {
    let mut body = json!({ "AcceleratorArn": accelerator_arn });
    for (key, value) in members.as_object().expect("a JSON object of members") {
        body[key] = value.clone();
    }
    call(router, "UpdateAcceleratorAttributes", body).await
}

/// The three members of `AcceleratorAttributes`, as a row states them: an absent
/// member is `Value::Null`, which is also how the envelope omits it.
fn attributes(enabled: bool, bucket: Option<&str>, prefix: Option<&str>) -> Value {
    json!({
        "FlowLogsEnabled": enabled,
        "FlowLogsS3Bucket": bucket,
        "FlowLogsS3Prefix": prefix,
    })
}

/// The attributes a response carries, in the shape [`attributes`] builds, so a row's
/// expectation and the wire answer compare member by member.
fn observed(body: &Value) -> Value {
    json!({
        "FlowLogsEnabled": body.get("FlowLogsEnabled").cloned().unwrap_or(Value::Null),
        "FlowLogsS3Bucket": body.get("FlowLogsS3Bucket").cloned().unwrap_or(Value::Null),
        "FlowLogsS3Prefix": body.get("FlowLogsS3Prefix").cloned().unwrap_or(Value::Null),
    })
}

#[tokio::test]
async fn a_fresh_accelerator_reports_flow_logs_off_and_no_destination() {
    let router = emulator();

    // One row per accelerator shape the create path can produce, so the reported
    // defaults are not a property of one particular create request.
    let rows: [(&str, Value); 3] = [
        ("plain", json!({})),
        ("disabled", json!({ "Enabled": false })),
        ("dual-stack", json!({ "IpAddressType": "DUAL_STACK" })),
    ];

    for (case, extra) in rows {
        let mut body = json!({ "Name": format!("attr-{case}"), "IdempotencyToken": format!("tok-{case}") });
        for (key, value) in extra.as_object().expect("a JSON object of members") {
            body[key] = value.clone();
        }
        let arn = arn_of(&ok(&router, "CreateAccelerator", body).await["Accelerator"]);

        let reported = describe(&router, &arn).await;
        assert_eq!(
            observed(&reported),
            attributes(false, None, None),
            "case {case}: expected flow logs off with no destination, got {reported}"
        );
    }
}

#[tokio::test]
async fn an_update_sets_only_the_members_it_carries_and_describe_reports_them() {
    let router = emulator();
    let arn = arn_of(&create_accelerator(&router, "attr-patch", "tok-patch").await);

    // Applied in order to ONE accelerator: each row's expectation is the whole
    // configuration the update leaves behind, so a member a row omits is asserted to
    // keep the value the row before it set.
    let rows: [(&str, Value, Value); 6] = [
        (
            "turn on with a destination",
            json!({
                "FlowLogsEnabled": true,
                "FlowLogsS3Bucket": "flow-logs-bucket",
                "FlowLogsS3Prefix": "prefix/",
            }),
            attributes(true, Some("flow-logs-bucket"), Some("prefix/")),
        ),
        (
            // The doc's own example prefix: "If you specify slash (/) for the S3
            // bucket prefix" (`UpdateAcceleratorAttributes$FlowLogsS3Prefix`).
            "prefix only",
            json!({ "FlowLogsS3Prefix": "/" }),
            attributes(true, Some("flow-logs-bucket"), Some("/")),
        ),
        (
            "bucket only",
            json!({ "FlowLogsS3Bucket": "other-bucket" }),
            attributes(true, Some("other-bucket"), Some("/")),
        ),
        (
            "flag only, off",
            json!({ "FlowLogsEnabled": false }),
            attributes(false, Some("other-bucket"), Some("/")),
        ),
        (
            "nothing but the ARN",
            json!({}),
            attributes(false, Some("other-bucket"), Some("/")),
        ),
        (
            "flag only, back on over the stored destination",
            json!({ "FlowLogsEnabled": true }),
            attributes(true, Some("other-bucket"), Some("/")),
        ),
    ];

    for (case, members, expected) in rows {
        let (status, body) = update(&router, &arn, &members).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} with {body}"
        );
        let updated = body["AcceleratorAttributes"].clone();
        assert_eq!(
            observed(&updated),
            expected,
            "case {case}: expected the update to answer {expected}, got {updated}"
        );

        // The read-back: what the update answered is what the store now holds.
        let reported = describe(&router, &arn).await;
        assert_eq!(
            observed(&reported),
            expected,
            "case {case}: expected describe to report {expected}, got {reported}"
        );
    }
}

#[tokio::test]
async fn enabling_flow_logs_without_a_destination_is_refused_and_changes_nothing() {
    let router = emulator();

    // "If the value is true, FlowLogsS3Bucket and FlowLogsS3Prefix must be specified"
    // (`UpdateAcceleratorAttributes$FlowLogsEnabled`), so every row below leaves one of
    // the two unset — including the empty string, which names no bucket and no prefix.
    let rows: [(&str, Value, &str); 5] = [
        (
            "flag alone",
            json!({ "FlowLogsEnabled": true }),
            "FlowLogsS3Bucket",
        ),
        (
            "bucket, no prefix",
            json!({ "FlowLogsEnabled": true, "FlowLogsS3Bucket": "flow-logs-bucket" }),
            "FlowLogsS3Prefix",
        ),
        (
            "prefix, no bucket",
            json!({ "FlowLogsEnabled": true, "FlowLogsS3Prefix": "prefix/" }),
            "FlowLogsS3Bucket",
        ),
        (
            "empty bucket",
            json!({
                "FlowLogsEnabled": true,
                "FlowLogsS3Bucket": "",
                "FlowLogsS3Prefix": "prefix/",
            }),
            "FlowLogsS3Bucket",
        ),
        (
            "empty prefix",
            json!({
                "FlowLogsEnabled": true,
                "FlowLogsS3Bucket": "flow-logs-bucket",
                "FlowLogsS3Prefix": "",
            }),
            "FlowLogsS3Prefix",
        ),
    ];

    for (case, members, missing) in rows {
        // An accelerator name takes only alphanumerics, periods and hyphens
        // (`CreateAccelerator$Name`), so the row's label is slugified into one.
        let slug: String = case
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let arn = arn_of(&create_accelerator(&router, &format!("attr-{slug}"), &slug).await);
        let (status, body) = update(&router, &arn, &members).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidArgumentException",
            "case {case}: expected InvalidArgumentException, got {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            format!("{missing} must be specified when FlowLogsEnabled is true."),
            "case {case}: expected the refusal to name {missing}, got {message:?}"
        );

        // A refused update leaves the configuration exactly as it was.
        let reported = describe(&router, &arn).await;
        assert_eq!(
            observed(&reported),
            attributes(false, None, None),
            "case {case}: expected the refused update to change nothing, got {reported}"
        );
    }
}

#[tokio::test]
async fn attributes_are_per_accelerator_and_survive_deleting_a_listener() {
    let router = emulator();

    let first = arn_of(&create_accelerator(&router, "attr-one", "tok-one").await);
    let second = arn_of(&create_accelerator(&router, "attr-two", "tok-two").await);
    let (status, body) = update(
        &router,
        &first,
        &json!({
            "FlowLogsEnabled": true,
            "FlowLogsS3Bucket": "one-bucket",
            "FlowLogsS3Prefix": "one/",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case seed first: expected HTTP 200, got {status} with {body}"
    );

    // One accelerator's attributes are its own.
    assert_eq!(
        observed(&describe(&router, &second).await),
        attributes(false, None, None),
        "case sibling: expected the other accelerator to keep its defaults"
    );

    // A listener comes and goes under the accelerator whose attributes are set; the
    // attributes belong to the accelerator, so neither call touches them.
    let listener_arn = ok(
        &router,
        "CreateListener",
        json!({
            "AcceleratorArn": first,
            "PortRanges": [{ "FromPort": 80, "ToPort": 80 }],
            "Protocol": "TCP",
            "IdempotencyToken": "lsn-attr",
        }),
    )
    .await["Listener"]["ListenerArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    for case in ["with a listener", "after deleting it"] {
        if case == "after deleting it" {
            ok(
                &router,
                "DeleteListener",
                json!({ "ListenerArn": listener_arn }),
            )
            .await;
        }
        let reported = describe(&router, &first).await;
        assert_eq!(
            observed(&reported),
            attributes(true, Some("one-bucket"), Some("one/")),
            "case {case}: expected the accelerator's attributes unaffected, got {reported}"
        );
    }
}

#[tokio::test]
async fn attributes_of_an_accelerator_the_store_does_not_hold_are_refused() {
    let router = emulator();
    let live = arn_of(&create_accelerator(&router, "attr-live", "tok-live").await);
    let deleted = arn_of(&create_accelerator(&router, "attr-gone", "tok-gone").await);
    ok(
        &router,
        "UpdateAccelerator",
        json!({ "AcceleratorArn": deleted, "Enabled": false }),
    )
    .await;
    ok(
        &router,
        "DeleteAccelerator",
        json!({ "AcceleratorArn": deleted }),
    )
    .await;

    // Rows: an ARN of the right shape that was never minted, one whose accelerator was
    // deleted, a non-ARN string, and the empty string the model's `@length(0..255)`
    // admits.
    let rows: [(&str, String); 4] = [
        (
            "never minted",
            format!("{live}-nope"),
        ),
        ("deleted", deleted.clone()),
        ("not an ARN", "attr-live".to_string()),
        ("empty", String::new()),
    ];

    for (case, accelerator_arn) in rows {
        for operation in [
            "DescribeAcceleratorAttributes",
            "UpdateAcceleratorAttributes",
        ] {
            let (status, body) = call(
                &router,
                operation,
                json!({ "AcceleratorArn": accelerator_arn }),
            )
            .await;
            // The status this service answers a missing resource with: HTTP 400, the
            // rule `not_found_error_status` states for the error class.
            assert_eq!(
                status, 400,
                "case {case}/{operation}: expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "AcceleratorNotFoundException",
                "case {case}/{operation}: expected AcceleratorNotFoundException, got {body}"
            );
            let message = error_message(&body);
            assert_eq!(
                message,
                format!("Accelerator {accelerator_arn} not found."),
                "case {case}/{operation}: expected the refusal to name the ARN, got {message:?}"
            );
        }
    }

    // The live accelerator is still readable, so the refusals above are about the ARN
    // the request named and not about the store.
    assert_eq!(
        observed(&describe(&router, &live).await),
        attributes(false, None, None),
        "case live: expected the remaining accelerator to still report its attributes"
    );
}

#[tokio::test]
async fn describing_attributes_does_not_change_what_a_later_caller_sees() {
    let router = emulator();
    let arn = arn_of(&create_accelerator(&router, "attr-read", "tok-read").await);
    let (status, body) = update(
        &router,
        &arn,
        &json!({
            "FlowLogsEnabled": true,
            "FlowLogsS3Bucket": "read-bucket",
            "FlowLogsS3Prefix": "read/",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case seed: expected HTTP 200, got {status} with {body}"
    );

    let first = describe(&router, &arn).await;
    for read in 2..=4 {
        let again = describe(&router, &arn).await;
        assert_eq!(
            observed(&again),
            observed(&first),
            "case read {read}: expected a repeated describe to report {first}, got {again}"
        );
    }
}
