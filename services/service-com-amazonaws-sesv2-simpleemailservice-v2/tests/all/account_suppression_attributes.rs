// SPDX-License-Identifier: CC0-1.0

//! The account-level suppression attributes, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization
//! are all exercised.
//!
//! The rule these cases pin: `PutAccountSuppressionAttributes` REPLACES the
//! account's suppression attributes with the ones the request carries, so a later
//! read of the account reports exactly those — a member the request leaves out is
//! cleared rather than kept — and a value outside a member's modelled value set is
//! refused with `BadRequestException`, leaving the stored attributes as they were.
//!
//! The table spans what the rule generalises over: how many reasons the request
//! names (none, one, all of them), which ones and in which order, whether the
//! validation attributes are carried at all, each `FeatureStatus` the condition
//! threshold can be switched to, and each confidence threshold the model publishes.

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::ops::types::{
    FEATURE_STATUS_VALUES, SUPPRESSION_CONFIDENCE_VERDICT_THRESHOLD_VALUES,
    SUPPRESSION_LIST_REASON_VALUES,
};

use crate::configuration_sets::{Reply, Ses};

/// `PutAccountSuppressionAttributes`'s route, from the operation's HTTP binding
/// (`smithy_query(query="operation",operation="PutAccountSuppressionAttributes")`:
/// `PUT /v2/email/account/suppression`).
const SUPPRESSION_PATH: &str = "/v2/email/account/suppression";

/// `GetAccount`'s route, which reads the stored attributes back.
const ACCOUNT_PATH: &str = "/v2/email/account";

async fn put_suppression(ses: &Ses, body: Value) -> Reply {
    ses.call("PUT", SUPPRESSION_PATH, Some(body)).await
}

async fn get_account(ses: &Ses) -> Reply {
    ses.call("GET", ACCOUNT_PATH, None).await
}

/// The `SuppressionAttributes` the account reports, or `Value::Null` when it
/// reports none.
async fn reported_attributes(ses: &Ses) -> Value {
    get_account(ses).await.member("SuppressionAttributes")
}

/// The `ValidationAttributes` body for a condition threshold, with the confidence
/// threshold only when one is given.
fn validation(enabled: &str, confidence: Option<&str>) -> Value {
    let mut threshold = json!({"ConditionThresholdEnabled": enabled});
    if let Some(confidence) = confidence {
        threshold["OverallConfidenceThreshold"] =
            json!({"ConfidenceVerdictThreshold": confidence});
    }
    json!({"ConditionThreshold": threshold})
}

/// One row of the replace-and-read table: the request body, and the
/// `SuppressionAttributes` a read of the account must report afterwards.
struct Case {
    case: String,
    body: Value,
    expected: Value,
}

/// The rows, built from the model's own value sets so every published value is
/// covered rather than only the ones a caller happened to use.
fn cases() -> Vec<Case> {
    let mut cases = vec![
        Case {
            case: "member omitted".to_string(),
            body: json!({}),
            expected: json!({"SuppressedReasons": []}),
        },
        Case {
            case: "empty reason list".to_string(),
            body: json!({"SuppressedReasons": []}),
            expected: json!({"SuppressedReasons": []}),
        },
        Case {
            case: "every reason".to_string(),
            body: json!({"SuppressedReasons": SUPPRESSION_LIST_REASON_VALUES}),
            expected: json!({"SuppressedReasons": SUPPRESSION_LIST_REASON_VALUES}),
        },
    ];

    // The order the request supplies the reasons in is the order the account holds
    // them in, so a reversed list is a distinct row.
    let reversed: Vec<&str> = SUPPRESSION_LIST_REASON_VALUES
        .iter()
        .rev()
        .copied()
        .collect();
    cases.push(Case {
        case: "every reason, reversed".to_string(),
        body: json!({"SuppressedReasons": reversed}),
        expected: json!({"SuppressedReasons": reversed}),
    });

    // One row per `SuppressionListReason` the model publishes.
    for reason in SUPPRESSION_LIST_REASON_VALUES {
        cases.push(Case {
            case: format!("only {reason}"),
            body: json!({"SuppressedReasons": [reason]}),
            expected: json!({"SuppressedReasons": [reason]}),
        });
    }

    // One row per `FeatureStatus` the condition threshold can be switched to, with
    // no confidence threshold named.
    for status in FEATURE_STATUS_VALUES {
        let attributes = validation(status, None);
        cases.push(Case {
            case: format!("validation {status}, no confidence threshold"),
            body: json!({
                "SuppressedReasons": SUPPRESSION_LIST_REASON_VALUES,
                "ValidationAttributes": attributes,
            }),
            expected: json!({
                "SuppressedReasons": SUPPRESSION_LIST_REASON_VALUES,
                "ValidationAttributes": attributes,
            }),
        });
    }

    // One row per `SuppressionConfidenceVerdictThreshold` the model publishes.
    for confidence in SUPPRESSION_CONFIDENCE_VERDICT_THRESHOLD_VALUES {
        let attributes = validation("ENABLED", Some(confidence));
        cases.push(Case {
            case: format!("validation enabled at {confidence}"),
            body: json!({
                "SuppressedReasons": ["BOUNCE"],
                "ValidationAttributes": attributes,
            }),
            expected: json!({
                "SuppressedReasons": ["BOUNCE"],
                "ValidationAttributes": attributes,
            }),
        });
    }

    cases
}

#[tokio::test]
async fn put_account_suppression_attributes_persists_what_the_request_carries() {
    for case in cases() {
        let case_name = case.case.clone();
        // A fresh service per row, so each row is answered from the attributes it
        // wrote and nothing else.
        let ses = Ses::new();

        let put = put_suppression(&ses, case.body.clone()).await;
        assert_eq!(
            put.status, 200,
            "case {case_name:?}: expected 200 from the suppression put, got {} {:?}",
            put.status,
            put.member("message")
        );
        assert!(
            put.is_empty_body(),
            "case {case_name:?}: PutAccountSuppressionAttributesResponse models no members, so the body must be empty, got {:?}",
            put.member("SuppressionAttributes")
        );

        let reported = reported_attributes(&ses).await;
        assert_eq!(
            reported, case.expected,
            "case {case_name:?}: expected the account to report {:?}, got {reported:?}",
            case.expected
        );

        // Reading the account does not change what a later caller sees.
        let again = reported_attributes(&ses).await;
        assert_eq!(
            again, case.expected,
            "case {case_name:?}: expected a second read to report {:?}, got {again:?}",
            case.expected
        );
    }
}

#[tokio::test]
async fn each_put_replaces_the_previous_attributes() {
    let ses = Ses::new();

    // What the account holds before anything configures it: the suppression list is
    // on for every reason the model publishes.
    let initial = reported_attributes(&ses).await;
    assert_eq!(
        initial,
        json!({"SuppressedReasons": SUPPRESSION_LIST_REASON_VALUES}),
        "case \"initial\": expected the account-level suppression list on for every reason, got {initial:?}"
    );

    // Each step replaces the whole sub-resource, so the attributes after it are the
    // ones that step carried — never the union with an earlier step's.
    let steps: Vec<(&str, Value, Value)> = vec![
        (
            "narrow to one reason and enable validation",
            json!({
                "SuppressedReasons": ["BOUNCE"],
                "ValidationAttributes": validation("ENABLED", Some("HIGH")),
            }),
            json!({
                "SuppressedReasons": ["BOUNCE"],
                "ValidationAttributes": validation("ENABLED", Some("HIGH")),
            }),
        ),
        (
            "widen the reasons, dropping the validation attributes",
            json!({"SuppressedReasons": SUPPRESSION_LIST_REASON_VALUES}),
            json!({"SuppressedReasons": SUPPRESSION_LIST_REASON_VALUES}),
        ),
        (
            "turn the suppression list off",
            json!({"SuppressedReasons": []}),
            json!({"SuppressedReasons": []}),
        ),
        (
            "validation only, with no reasons named",
            json!({"ValidationAttributes": validation("DISABLED", None)}),
            json!({
                "SuppressedReasons": [],
                "ValidationAttributes": validation("DISABLED", None),
            }),
        ),
    ];

    for (case, body, expected) in steps {
        let put = put_suppression(&ses, body).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected 200 from the suppression put, got {}",
            put.status
        );
        let reported = reported_attributes(&ses).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the account to report {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_value_outside_its_modelled_set_is_refused_and_changes_nothing() {
    // Each row carries a value no member's value set holds: a reason, a feature
    // status and a confidence threshold, plus a reason that differs only in case.
    let rows: Vec<(&str, Value)> = vec![
        ("unknown reason", json!({"SuppressedReasons": ["SPAM"]})),
        (
            "lower-case reason",
            json!({"SuppressedReasons": ["bounce"]}),
        ),
        (
            "one good reason and one unknown",
            json!({"SuppressedReasons": ["BOUNCE", "UNSUBSCRIBE"]}),
        ),
        (
            "unknown feature status",
            json!({
                "SuppressedReasons": ["BOUNCE"],
                "ValidationAttributes": validation("ON", None),
            }),
        ),
        (
            "unknown confidence threshold",
            json!({
                "SuppressedReasons": ["BOUNCE"],
                "ValidationAttributes": validation("ENABLED", Some("LOW")),
            }),
        ),
    ];

    for (case, body) in rows {
        let ses = Ses::new();
        // A known state to refuse against, so the assertion shows the refusal kept
        // it rather than that nothing had been configured.
        let settled = json!({"SuppressedReasons": ["COMPLAINT"]});
        let prepared = put_suppression(&ses, settled.clone()).await;
        assert_eq!(
            prepared.status, 200,
            "case {case:?}: expected 200 while preparing the account, got {}",
            prepared.status
        );

        let refused = put_suppression(&ses, body).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected 400 BadRequestException, got {} {:?}",
            refused.status,
            refused.error_code()
        );
        assert!(
            !refused.message().is_empty(),
            "case {case:?}: expected the refusal to name what was invalid, got an empty message"
        );

        let reported = reported_attributes(&ses).await;
        assert_eq!(
            reported, settled,
            "case {case:?}: expected the refused put to leave {settled:?} in place, got {reported:?}"
        );
    }
}
