// SPDX-License-Identifier: CC0-1.0

//! The WORDS Service Quotas refuses a request with, over the wire.
//!
//! Every call here is a real `POST /` with `X-Amz-Target:
//! ServiceQuotasV20190624.<Op>` dispatched through the same `MultiServiceRouter` the
//! emulator serves with, over ONE shared engine. The harness (`emulator`, `client`,
//! `Client`) is `quota_workflow.rs`'s.
//!
//! The rule asserted, over a TABLE of inputs per fact: a refusal states the service's own
//! sentence for its error class, FIXED text that interpolates nothing from the request —
//! no `ServiceCode`, no `QuotaCode`, no `RequestId`, no `DesiredValue` — so two calls that
//! fail the same way read back the same string whatever they carried. The one sentence
//! with a variable part is the refusal of a value below the quota's AWS DEFAULT, and that
//! part is that default, taken from this service's own catalog and rendered as a decimal;
//! the other bound of the same refusal — a value that clears the default but is not above
//! the value in force — names no value at all.
//!
//! The facts stated by a lookup of a service or a quota are spanned by
//! `no_such_service_wording.rs` and `no_such_quota_wording.rs`; this suite spans the
//! `RequestId` lookup and the quota-value refusal (both its bounds and the ORDER they are
//! checked in), and pins that each sentence is the crate's one sentence for its fact.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_servicequotas_servicequotasv20190624::service::refusals::{
    below_default_value_message, NOT_ABOVE_CURRENT_VALUE_MESSAGE, NO_SUCH_QUOTA_REQUEST_MESSAGE,
};

use crate::quota_workflow::{client, default_quotas, emulator, service_inventory, Client};

/// Every modelled client fault is HTTP 400 on this awsJson service, whatever
/// `@httpError` its shape declares — see the `error_envelope` suite.
const CLIENT_FAULT: u16 = 400;

/// Comfortably past BOTH steps the clock owes a quota increase request
/// (`crate::state::SUPPORT_CASE_OPEN_DELAY`, then `crate::state::REQUEST_DECISION_DELAY`
/// once its support case is open), so a request made before this wait has settled
/// afterwards and its value is in force.
const PAST_DECISION: Duration = Duration::from_millis(1_800);

/// The sentence this service states an unresolvable `RequestId` in. Spelled here so the
/// suite pins the WORDING as well as its single sourcing: comparing a refusal against the
/// crate's constant alone would stay green if that constant's own sentence drifted.
const REQUEST_WORDING: &str = "The request failed because the specified requestId to increase \
                               the service quota does not exist.";

/// The fixed sentence a value BELOW the quota's AWS default is refused with, up to the
/// default value it ends in, as recorded.
const BELOW_DEFAULT: &str = "You must provide a quota value greater than the default quota \
                             value of ";

/// The fixed sentence a value that clears the default but is not above the value in force
/// is refused with, as recorded: this bound names no value at all.
const NOT_ABOVE_CURRENT: &str = "You must provide a quota value greater than the current \
                                 quota value";

/// A quota value as this service renders one in a refusal: a decimal carrying at least
/// one fractional digit, so a whole 5 reads `5.0`. Spelled here rather than imported, so
/// the suite pins the rendering instead of agreeing with whatever the crate does.
fn decimal(value: f64) -> String {
    let rendered = format!("{value}");
    if rendered.contains('.') {
        rendered
    } else {
        format!("{rendered}.0")
    }
}

/// The status, error code and `Message` of one call.
async fn answer(client: &Client, operation: &str, body: &Value) -> (u16, String, String) {
    let (status, parsed) = client.call(operation, body).await;
    let member = |name: &str| {
        parsed
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or("<absent>")
            .to_string()
    };
    (status, member("__type"), member("Message"))
}

/// One adjustable quota of the catalog that HAS an account-wide plane, as
/// `(ServiceCode, QuotaCode, its AWS default value)`.
type Quota = (String, String, f64);

/// Adjustable quotas with an account-wide plane and DISTINCT default values, read out of
/// the catalog over the wire (`ListServices` + `ListAWSDefaultServiceQuotas`) so the rows
/// span quotas no issue cited and whatever the catalog publishes.
///
/// A quota is kept only when the account-wide read of it is served, because a quota that
/// is adjustable for an individual resource only has no account plane to ask for an
/// increase on and is refused for that instead.
async fn adjustable_quotas(client: &Client, wanted: usize) -> Vec<Quota> {
    let mut picked: Vec<Quota> = Vec::new();
    for service in service_inventory(client).await {
        if picked.len() == wanted {
            break;
        }
        let Some(service_code) = service.get("ServiceCode").and_then(Value::as_str) else {
            continue;
        };
        for quota in default_quotas(client, service_code).await {
            if picked.len() == wanted {
                break;
            }
            let (Some(quota_code), Some(value)) = (
                quota.get("QuotaCode").and_then(Value::as_str),
                quota.get("Value").and_then(Value::as_f64),
            ) else {
                continue;
            };
            if quota.get("Adjustable") != Some(&json!(true)) {
                continue;
            }
            // A default value no other row already carries, so the rows differ in the
            // one variable part the sentence has.
            if picked.iter().any(|(_, _, seen)| *seen == value) {
                continue;
            }
            let (status, _) = client
                .call(
                    "GetServiceQuota",
                    &json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
                )
                .await;
            if status == 200 {
                picked.push((
                    service_code.to_string(),
                    quota_code.to_string(),
                    value,
                ));
            }
        }
    }
    assert!(
        picked.len() == wanted,
        "case setup: expected {wanted} adjustable account-wide quotas with distinct default \
         values, got {picked:?}"
    );
    picked
}

/// An unresolvable `RequestId` is refused with the service's ONE fixed sentence for the
/// fact, by every operation that resolves one, and the id is never echoed back.
#[tokio::test]
async fn an_unresolvable_request_id_is_stated_in_one_fixed_sentence() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    // The ids differ in FORM as well as in being unresolvable:
    // `@length 1..128` with `@pattern ^[0-9a-zA-Z][a-zA-Z0-9-]{1,128}$`
    // (`smithy_query(query="shape", shape="RequestId")`) — a word nothing was minted for,
    // a hyphenated phrase, an id at the length maximum the member admits, the shortest
    // the pattern admits, and one that starts with a digit (which this member, alone
    // among the code members, allows).
    let at_the_maximum = format!("0{}", "a".repeat(127));
    let ids = [
        ("a word nothing was minted for", "requestthatwasnevermintedhere"),
        ("a hyphenated phrase", "no-such-quota-increase-request"),
        ("an id at the length maximum", &at_the_maximum[..]),
        ("the shortest id the member admits", "0a"),
        ("an id that starts with a digit", "7abcdef"),
    ];

    for (why, request_id) in ids {
        for operation in ["GetRequestedServiceQuotaChange", "CreateSupportCase"] {
            let case = format!("{why} ({request_id}), through {operation}");
            let (status, code, message) =
                answer(&caller, operation, &json!({"RequestId": request_id})).await;
            assert_eq!(
                (status, code.as_str()),
                (CLIENT_FAULT, "NoSuchResourceException"),
                "case {case}: expected HTTP {CLIENT_FAULT} NoSuchResourceException, got {status} \
                 {code} with {message:?}"
            );
            assert_eq!(
                message, NO_SUCH_QUOTA_REQUEST_MESSAGE,
                "case {case}: expected the crate's one sentence for the fact, got {message:?}"
            );
            assert_eq!(
                message, REQUEST_WORDING,
                "case {case}: expected the sentence {REQUEST_WORDING:?}, got {message:?}"
            );
            assert!(
                !message.contains(request_id),
                "case {case}: expected the refusal not to echo the RequestId, got {message:?}"
            );
        }
    }

    // The other half of the rule: an id this endpoint DOES resolve is served, so the
    // refusals above report a missing request rather than a reader that fails for
    // everything.
    let (service_code, quota_code, default_value) = adjustable_quotas(&caller, 1).await[0].clone();
    let minted = caller
        .ok(
            "RequestServiceQuotaIncrease",
            json!({
                "ServiceCode": service_code,
                "QuotaCode": quota_code,
                "DesiredValue": default_value + 1.0,
            }),
        )
        .await;
    let id = minted["RequestedQuota"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case the minted request: expected an Id, got {minted}"))
        .to_string();
    for operation in ["GetRequestedServiceQuotaChange", "CreateSupportCase"] {
        let (status, parsed) = caller.call(operation, &json!({"RequestId": &id})).await;
        assert_eq!(
            status, 200,
            "case the minted request {id}, through {operation}: expected HTTP 200, got {status} \
             with {parsed}"
        );
    }
}

/// A `DesiredValue` that is not above the quota's AWS DEFAULT is refused in the service's
/// fixed sentence for it, whose one variable part is that quota's own default value —
/// rendered as a decimal, and taken from the catalog rather than from the request.
#[tokio::test]
async fn a_value_below_the_default_names_the_default_of_the_named_quota() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    // Four quotas with four different default values, so the sentence is seen to follow
    // the QUOTA rather than carrying one recorded number.
    let quotas = adjustable_quotas(&caller, 4).await;
    let mut stated: Vec<String> = Vec::new();

    for (service_code, quota_code, default_value) in &quotas {
        // The desired values differ along WHERE they sit relative to the default, which is
        // the bound this sentence states: the bottom of the member's
        // `@range 0..10000000000` and half the default are both below it. (The default
        // itself is not an increase either, but it is AT the default, so it falls to the
        // other bound — the suite below drives that one.)
        for (why, desired) in [
            ("the bottom of the member's range", 0.0),
            ("half the default", default_value / 2.0),
        ] {
            let case = format!("{service_code}/{quota_code}, {why} ({desired})");
            let (status, code, message) = answer(
                &caller,
                "RequestServiceQuotaIncrease",
                &json!({
                    "ServiceCode": service_code,
                    "QuotaCode": quota_code,
                    "DesiredValue": desired,
                }),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (CLIENT_FAULT, "IllegalArgumentException"),
                "case {case}: expected HTTP {CLIENT_FAULT} IllegalArgumentException, got {status} \
                 {code} with {message:?}"
            );
            let expected = format!("{BELOW_DEFAULT}{}", decimal(*default_value));
            assert_eq!(
                message, expected,
                "case {case}: expected the refusal to state {expected:?}, got {message:?}"
            );
            assert_eq!(
                message,
                below_default_value_message(*default_value),
                "case {case}: expected the crate's one sentence for the fact, got {message:?}"
            );
            // The variable part is the quota's DEFAULT, rendered as a decimal, and the
            // request's own members appear nowhere in the sentence.
            assert!(
                !message.contains(quota_code.as_str())
                    && !message.contains(service_code.as_str())
                    && message.ends_with(&decimal(*default_value)),
                "case {case}: expected a sentence ending in the quota's default value and \
                 carrying nothing of the request, got {message:?}"
            );
            stated.push(message);
        }
    }

    // Four distinct defaults, so at least four distinct sentences: the number in the
    // sentence is the named quota's, not a constant.
    let mut distinct = stated.clone();
    distinct.sort();
    distinct.dedup();
    assert!(
        distinct.len() == quotas.len(),
        "case the table as a whole: expected one sentence per distinct default value, got \
         {distinct:?}"
    );
}

/// The OTHER bound, and the ORDER the two are checked in: once a quota carries an applied
/// value above its default, a request that clears the default but not that value is refused
/// with the fixed current-value sentence (which names no value), while a request BELOW the
/// default is still refused with the default sentence — the bound follows where the
/// `DesiredValue` falls, not what is in force.
#[tokio::test]
async fn the_two_bounds_are_stated_in_the_order_the_service_checks_them() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    for (service_code, quota_code, default_value) in adjustable_quotas(&caller, 2).await {
        let in_force = default_value * 4.0 + 1.0;
        caller
            .ok(
                "RequestServiceQuotaIncrease",
                json!({
                    "ServiceCode": service_code,
                    "QuotaCode": quota_code,
                    "DesiredValue": in_force,
                }),
            )
            .await;
        tokio::time::sleep(PAST_DECISION).await;
        let read = caller
            .ok(
                "GetServiceQuota",
                json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
            )
            .await;
        assert_eq!(
            read["Quota"]["Value"].as_f64(),
            Some(in_force),
            "case {service_code}/{quota_code}: expected the approved increase in force, got {read}"
        );

        // The rows differ along where the asked-for value sits against the TWO bounds,
        // with an applied value in force for all of them: at the default and above it
        // (the current-value bound), and below the default (the default bound, which is
        // checked first).
        let mut rows = vec![
            (
                "the default itself, which the applied value is above",
                default_value,
                NOT_ABOVE_CURRENT.to_string(),
            ),
            (
                "between the default and the value in force",
                default_value * 2.0 + 1.0,
                NOT_ABOVE_CURRENT.to_string(),
            ),
            (
                "the value in force itself",
                in_force,
                NOT_ABOVE_CURRENT.to_string(),
            ),
        ];
        // A quota whose default is 0 has no value below it, so the default-bound row is
        // only a row where half the default really does sit below the default.
        let below_default = default_value / 2.0;
        if below_default < default_value {
            rows.push((
                "below the default, with an applied value in force",
                below_default,
                format!("{BELOW_DEFAULT}{}", decimal(default_value)),
            ));
        }

        for (why, desired, expected) in rows {
            let case =
                format!("{service_code}/{quota_code}, {why} ({desired} against {in_force})");
            let (status, code, message) = answer(
                &caller,
                "RequestServiceQuotaIncrease",
                &json!({
                    "ServiceCode": service_code,
                    "QuotaCode": quota_code,
                    "DesiredValue": desired,
                }),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (CLIENT_FAULT, "IllegalArgumentException"),
                "case {case}: expected HTTP {CLIENT_FAULT} IllegalArgumentException, got {status} \
                 {code} with {message:?}"
            );
            assert_eq!(
                message, expected,
                "case {case}: expected the refusal to state {expected:?}, got {message:?}"
            );
            if expected == NOT_ABOVE_CURRENT {
                // The current-value bound names NO value: not the one in force, not the
                // default, and nothing of the request.
                assert_eq!(
                    message, NOT_ABOVE_CURRENT_VALUE_MESSAGE,
                    "case {case}: expected the crate's one sentence for the bound, got {message:?}"
                );
                assert!(
                    !message.contains(&decimal(in_force))
                        && !message.contains(&decimal(default_value))
                        && !message.contains(quota_code.as_str()),
                    "case {case}: expected a sentence carrying no value at all, got {message:?}"
                );
            } else {
                assert_eq!(
                    message,
                    below_default_value_message(default_value),
                    "case {case}: expected the crate's one sentence for the default bound, got \
                     {message:?}"
                );
            }
        }

        // And a value above the one in force IS an increase, so the refusals above are
        // about the value and not about the quota.
        caller
            .ok(
                "RequestServiceQuotaIncrease",
                json!({
                    "ServiceCode": service_code,
                    "QuotaCode": quota_code,
                    "DesiredValue": in_force + default_value + 1.0,
                }),
            )
            .await;
    }
}
