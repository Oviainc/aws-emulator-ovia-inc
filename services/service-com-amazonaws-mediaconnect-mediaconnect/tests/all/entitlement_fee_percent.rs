// SPDX-License-Identifier: CC0-1.0

//! An entitlement's `DataTransferSubscriberFeePercent` is a PERCENTAGE, end to end over
//! HTTP: the member is documented as a "Percentage from 0-100 of the data transfer cost to
//! be billed to the subscriber"
//! (`smithy_query(query="shape",shape="GrantEntitlementRequest")`), so only a value within
//! that inclusive range is stored — on each path that writes one, CreateFlow's
//! `Entitlements` and GrantFlowEntitlements' `Entitlements`.
//!
//! The rule is asserted over a TABLE spanning the class rather than the one value that
//! exposed the gap: the accepted rows are both published bounds plus values inside the
//! range, and the refused rows are the first value past each bound, a value far past each,
//! and the extremes the modelled `Integer` can carry. A value the rule admits is reported
//! back unchanged by the grant, by DescribeFlow and by ListEntitlements' summary; a value it
//! refuses stores nothing at all.
//!
//! Every case drives the real request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what these tests assert is what a MediaConnect
//! client sees on the wire.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_entitlements::{entitlements, flow_with, grant};
use crate::flow_lifecycle::Emulator;

/// An account ID used wherever a row needs a subscriber: "the Amazon Web Services account
/// IDs that you want to share your content with"
/// (`smithy_query(query="doc",shape="GrantEntitlementRequest",member="Subscribers")`).
const SUBSCRIBER: &str = "111122223333";

/// One `DataTransferSubscriberFeePercent`, and whether the service stores an entitlement
/// carrying it.
struct FeeCase {
    case: &'static str,
    percent: Option<i32>,
    accepted: bool,
}

/// The percentages asserted below.
///
/// Source for the bounds: `GrantEntitlementRequest$DataTransferSubscriberFeePercent` —
/// "Percentage from 0-100 of the data transfer cost to be billed to the subscriber" — whose
/// target carries no `@range`, so the documented ends are the bounds and the crate states
/// them once ([`mc::MIN_SUBSCRIBER_FEE_PERCENT`], [`mc::MAX_SUBSCRIBER_FEE_PERCENT`]).
///
/// The accepted rows are both published bounds, one value just inside each, and one in the
/// middle; the refused rows are the first value past each bound, a value far past each, and
/// each end of the modelled `Integer`. A row with the member absent is accepted: an
/// entitlement that asks for no fee share at all carries no percentage.
fn fee_cases() -> Vec<FeeCase> {
    vec![
        FeeCase {
            case: "the member absent: no fee share asked for",
            percent: None,
            accepted: true,
        },
        FeeCase {
            case: "the published minimum",
            percent: Some(mc::MIN_SUBSCRIBER_FEE_PERCENT),
            accepted: true,
        },
        FeeCase {
            case: "one past the published minimum",
            percent: Some(mc::MIN_SUBSCRIBER_FEE_PERCENT + 1),
            accepted: true,
        },
        FeeCase {
            case: "a percentage in the middle of the range",
            percent: Some(37),
            accepted: true,
        },
        FeeCase {
            case: "one short of the published maximum",
            percent: Some(mc::MAX_SUBSCRIBER_FEE_PERCENT - 1),
            accepted: true,
        },
        FeeCase {
            case: "the published maximum",
            percent: Some(mc::MAX_SUBSCRIBER_FEE_PERCENT),
            accepted: true,
        },
        FeeCase {
            case: "one below the published minimum",
            percent: Some(mc::MIN_SUBSCRIBER_FEE_PERCENT - 1),
            accepted: false,
        },
        FeeCase {
            case: "far below the published minimum",
            percent: Some(-20),
            accepted: false,
        },
        FeeCase {
            case: "one above the published maximum",
            percent: Some(mc::MAX_SUBSCRIBER_FEE_PERCENT + 1),
            accepted: false,
        },
        FeeCase {
            case: "far above the published maximum",
            percent: Some(500),
            accepted: false,
        },
        FeeCase {
            case: "the lowest value the modelled Integer carries",
            percent: Some(i32::MIN),
            accepted: false,
        },
        FeeCase {
            case: "the highest value the modelled Integer carries",
            percent: Some(i32::MAX),
            accepted: false,
        },
    ]
}

/// One entitlement of a request, named `name` and carrying the row's percentage — or
/// carrying no such member at all when the row has none.
fn entitlement_body(name: &str, percent: Option<i32>) -> Value {
    let mut requested = json!({ "name": name, "subscribers": [SUBSCRIBER] });
    if let Some(percent) = percent {
        requested["dataTransferSubscriberFeePercent"] = json!(percent);
    }
    requested
}

/// The `dataTransferSubscriberFeePercent` a row's entitlement must report back: the value
/// it asked for, and JSON `null` (the absent member) for a row that asked for none.
fn reported_percent(percent: Option<i32>) -> Value {
    percent.map_or(Value::Null, |percent| json!(percent))
}

/// Assert a refusal is this service's client-input error, worded so a caller can see which
/// member was unacceptable and what it held: `BadRequestException` is "thrown if the request
/// contains a semantic error. The precise meaning depends on the API, and is documented in
/// the error message".
fn assert_refusal_states_the_rule(case: &str, percent: i32, status: u16, code: Option<&str>, body: &Value) {
    assert_eq!(
        (status, code),
        (400, Some("BadRequestException")),
        "case {case:?}: expected 400 BadRequestException, got {status} {code:?}: {body}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("DataTransferSubscriberFeePercent"),
        "case {case:?}: the refusal names the member, got {body}"
    );
    assert!(
        message.contains(&percent.to_string()),
        "case {case:?}: the refusal states the value it refused, got {body}"
    );
}

/// The `name` of every flow ListFlows reports.
async fn listed_flow_names(emulator: &Emulator) -> Vec<String> {
    let (status, code, body) = emulator.list_flows("").await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "listing flows expected 200, got {status} {code:?}: {body}"
    );
    body["flows"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|flow| flow["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// GrantFlowEntitlements stores a percentage within the documented range and reports it
/// back; a value outside the range is refused with the modelled client error and leaves the
/// flow's entitlements exactly as they were.
#[tokio::test]
async fn grant_flow_entitlements_bounds_the_subscriber_fee_percent() {
    let emulator = Emulator::new();
    let flow_arn = flow_with(&emulator, "fee-percent-grant-flow", &[]).await;

    // The entitlements the ACCEPTED rows have stored so far, as `(name, percentage)`.
    let mut stored: Vec<(String, Value)> = Vec::new();
    for (row, FeeCase { case, percent, accepted }) in fee_cases().into_iter().enumerate() {
        let name = format!("granted-{row}");
        let (status, code, body) = grant(
            &emulator,
            &flow_arn,
            json!({ "entitlements": [entitlement_body(&name, percent)] }),
        )
        .await;

        if accepted {
            assert_eq!(
                (status, code.as_deref()),
                (201, None),
                "case {case:?}: granting expected 201, got {status} {code:?}: {body}"
            );
            let granted = entitlements(&body)
                .first()
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                granted["dataTransferSubscriberFeePercent"],
                reported_percent(percent),
                "case {case:?}: the percentage the grant reports, got {body}"
            );
            stored.push((name, reported_percent(percent)));
        } else {
            let unusable = percent.unwrap_or_default();
            assert_refusal_states_the_rule(case, unusable, status, code.as_deref(), &body);
        }

        // Whatever the row, the flow carries exactly what the ACCEPTED rows stored: a
        // refused grant stores nothing.
        let (status, code, described) = emulator.describe_flow(&flow_arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: describing expected 200, got {status} {code:?}: {described}"
        );
        let held: Vec<(String, Value)> = entitlements(&described["flow"])
            .iter()
            .map(|entitlement| {
                (
                    entitlement["name"].as_str().unwrap_or_default().to_string(),
                    entitlement["dataTransferSubscriberFeePercent"].clone(),
                )
            })
            .collect();
        assert_eq!(
            held, stored,
            "case {case:?}: the entitlements the flow holds and the percentages they report, \
             got {described}"
        );
    }
}

/// CreateFlow's `Entitlements` grants entitlements through the same rule: a row the service
/// refuses creates NO flow at all, and a row it accepts is reported by DescribeFlow exactly
/// as it was given.
#[tokio::test]
async fn create_flow_bounds_the_subscriber_fee_percent() {
    let emulator = Emulator::new();

    for (row, FeeCase { case, percent, accepted }) in fee_cases().into_iter().enumerate() {
        let name = format!("fee-percent-create-{row}");
        let (status, code, body) = emulator
            .create_flow(json!({
                "name": name,
                "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" },
                "entitlements": [entitlement_body("entitled", percent)],
            }))
            .await;

        if accepted {
            assert_eq!(
                (status, code.as_deref()),
                (201, None),
                "case {case:?}: creating expected 201, got {status} {code:?}: {body}"
            );
            let flow_arn = body["flow"]["flowArn"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let (status, code, described) = emulator.describe_flow(&flow_arn).await;
            assert_eq!(
                (status, code.as_deref()),
                (200, None),
                "case {case:?}: describing expected 200, got {status} {code:?}: {described}"
            );
            let stored = entitlements(&described["flow"])
                .first()
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                stored["dataTransferSubscriberFeePercent"],
                reported_percent(percent),
                "case {case:?}: the stored percentage, got {described}"
            );
        } else {
            let unusable = percent.unwrap_or_default();
            assert_refusal_states_the_rule(case, unusable, status, code.as_deref(), &body);
            assert!(
                !listed_flow_names(&emulator).await.contains(&name),
                "case {case:?}: a refused create stores no flow, got {name:?} in the listing"
            );
        }
    }
}

/// Every reader of a stored entitlement reports a percentage the rule admits: the summary
/// ListEntitlements projects carries exactly the percentage the grant stored, for each
/// accepted row.
#[tokio::test]
async fn listed_entitlements_report_the_stored_fee_percent() {
    let emulator = Emulator::new();
    let this_account = engine::emulator_account_id().to_string();
    let flow_arn = flow_with(&emulator, "fee-percent-listing-flow", &[]).await;

    // Grant one entitlement per accepted row, shared with THIS account so the listing — "all
    // entitlements that have been granted to this account" — reports every one of them.
    let accepted: Vec<(String, Option<i32>)> = fee_cases()
        .into_iter()
        .filter(|case| case.accepted)
        .enumerate()
        .map(|(row, case)| (format!("listed-{row}"), case.percent))
        .collect();
    for (name, percent) in &accepted {
        let mut requested = entitlement_body(name, *percent);
        requested["subscribers"] = json!([this_account]);
        let (status, code, body) = grant(
            &emulator,
            &flow_arn,
            json!({ "entitlements": [requested] }),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "granting {name:?} expected 201, got {status} {code:?}: {body}"
        );
    }

    let (status, code, body) = emulator.send("GET", "/v1/entitlements", None).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "listing entitlements expected 200, got {status} {code:?}: {body}"
    );
    let listed = body["entitlements"].as_array().cloned().unwrap_or_default();
    for (name, percent) in &accepted {
        let summary = listed
            .iter()
            .find(|entitlement| entitlement["entitlementName"].as_str() == Some(name.as_str()))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            summary["dataTransferSubscriberFeePercent"],
            reported_percent(*percent),
            "case {name:?}: the listed percentage, got {summary}"
        );
    }
}

/// The crate's own encoding of the rule, asserted on the same rows: the HTTP cases above go
/// through it, and this pins the predicate itself so every caller reads one bound.
#[test]
fn fee_percent_rejection_matches_the_refusals() {
    for FeeCase { case, percent, accepted } in fee_cases() {
        let rejection = mc::data_transfer_subscriber_fee_percent_rejection("Entitlements[0]", percent);
        assert_eq!(
            rejection.is_none(),
            accepted,
            "case {case:?}: {percent:?} acceptable expected {accepted}, got {rejection:?}"
        );
    }
}
