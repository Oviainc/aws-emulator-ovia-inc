// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateAnomaly`: suppressing — and un-suppressing — one finding of
//! a log anomaly detector.
//!
//! The rules under test:
//!
//! * a request names exactly ONE finding — "You must specify either anomalyId or patternId,
//!   but you can't specify both parameters in the same operation" — and one that names both,
//!   or neither, is refused as the bad parameter it is;
//! * `LIMITED` suppression needs a period — "If you specify LIMITED, you must also specify a
//!   suppressionPeriod";
//! * the detector the request acts on, and the finding it names, are both resolved from the
//!   store, so an ARN the account holds no detector under and an id the named detector has
//!   not reported are refused with `ResourceNotFoundException`;
//! * a finding the detector DID report is suppressed on the stored record, and a later call
//!   that names it and omits `suppressionType` and `suppressionPeriod` ends that suppression.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.UpdateAnomaly` through the router that mounts it — so an
//! answer that is not computed from the shared store cannot pass.
//!
//! The tables span what the rules generalise over: which selector the request names, every
//! value of the model's `SuppressionType` enum, every value of its `SuppressionUnit` enum,
//! `baseline` set and unset, a finding reported by the detector and one reported by another
//! one, and each form an unknown detector ARN takes.

use serde_json::{json, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    log_group_arn, ComAmazonawsCloudwatchlogsLogs20140328State, SUPPRESSION_TYPE_INFINITE,
};

use crate::harness::{now_ms, report_anomaly, Logs, REGION};

/// The operation under test.
const UPDATE: &str = "UpdateAnomaly";

/// The log group the detector in these cases watches.
const GROUP: &str = "anomaly-suppression/watched";

/// The code answered for a parameter the operation will not accept ("A parameter is
/// specified incorrectly").
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The code answered when the resource a request names is not there.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// Every value of the model's `SuppressionType` enum. Source:
/// `smithy_query(query="shape",shape="SuppressionType")` — `LIMITED | INFINITE`.
const SUPPRESSION_TYPES: [&str; 2] = ["LIMITED", SUPPRESSION_TYPE_INFINITE];

/// Every value of the model's `SuppressionUnit` enum, with how many milliseconds one of it
/// lasts. Source: `smithy_query(query="shape",shape="SuppressionUnit")` — `SECONDS |
/// MINUTES | HOURS`.
const SUPPRESSION_UNITS: [(&str, i64); 3] = [
    ("SECONDS", 1_000),
    ("MINUTES", 60 * 1_000),
    ("HOURS", 60 * 60 * 1_000),
];

/// An `anomalyId` of the length the model fixes it at (`@length(36..36)`).
fn anomaly_id(seed: char) -> String {
    std::iter::repeat_n(seed, 36).collect()
}

/// A `patternId` of the length the model fixes it at (`@length(32..32)`).
fn pattern_id(seed: char) -> String {
    std::iter::repeat_n(seed, 32).collect()
}

/// Create one anomaly detector over [`GROUP`] and answer its ARN.
async fn detector(logs: &Logs) -> String {
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    let created = logs
        .ok(
            "CreateLogAnomalyDetector",
            &json!({"logGroupArnList": [log_group_arn(REGION, GROUP, false)]}),
        )
        .await;
    created
        .get("anomalyDetectorArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateLogAnomalyDetector hands back an ARN, got {created}"))
        .to_string()
}

/// Record, in the store, that the detector under `arn` has REPORTED the anomaly `anomaly`
/// of the pattern `pattern` — the set a suppression request resolves its id against. The
/// seeding itself is [`crate::harness::report_anomaly`], which every suite that needs a
/// finding in the store shares.
fn report_finding(logs: &Logs, arn: &str, anomaly: &str, pattern: &str) {
    report_anomaly(logs, arn, anomaly, pattern);
}

/// The suppression the store holds for `id` on the detector under `arn`: its
/// `suppressionType`, whether it runs out, its `baseline` flag, and when it was placed.
fn stored_suppression(
    logs: &Logs,
    arn: &str,
    id: &str,
) -> Option<(Option<String>, Option<i64>, bool, i64)> {
    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let store = state.lock();
    store
        .anomaly_detectors
        .get(arn)
        .and_then(|detector| detector.suppressions.get(id))
        .map(|suppression| {
            (
                suppression.suppression_type.clone(),
                suppression.suppressed_until_ms,
                suppression.baseline,
                suppression.suppressed_at_ms,
            )
        })
}

/// The anomalies `ListAnomalies` reports for one request, in the order it listed them.
async fn listed_anomalies(logs: &Logs, request: &Value) -> Vec<Value> {
    let page = logs.ok("ListAnomalies", request).await;
    page.get("anomalies")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ListAnomalies returns anomalies, got {page}"))
        .clone()
}

/// A request naming both selectors, or neither, is refused and changes nothing.
#[tokio::test]
async fn a_request_names_exactly_one_finding() {
    let logs = Logs::new();
    let arn = detector(&logs).await;
    let anomaly = anomaly_id('a');
    let pattern = pattern_id('b');
    report_finding(&logs, &arn, &anomaly, &pattern);

    // (case, the selectors the request names)
    let rows: [(&str, Value); 2] = [
        (
            "both selectors",
            json!({
                "anomalyDetectorArn": arn,
                "anomalyId": anomaly,
                "patternId": pattern,
                "suppressionType": SUPPRESSION_TYPE_INFINITE,
            }),
        ),
        (
            "neither selector",
            json!({
                "anomalyDetectorArn": arn,
                "suppressionType": SUPPRESSION_TYPE_INFINITE,
            }),
        ),
    ];
    for (case, request) in rows {
        let (code, message) = logs.refusal(UPDATE, &request).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case}: expected {INVALID_PARAMETER}, got {code} / {message}"
        );
        assert_eq!(
            message,
            "You must specify either anomalyId or patternId, but you can't specify both \
             parameters in the same operation.",
            "case {case}: the refusal states the rule, got {message:?}"
        );
        assert_eq!(
            stored_suppression(&logs, &arn, &anomaly),
            None,
            "case {case}: a refused request suppresses nothing"
        );
    }

    // A `LIMITED` suppression with no period is the other parameter fault the operation
    // publishes: "If you specify LIMITED, you must also specify a suppressionPeriod."
    let (code, _) = logs
        .refusal(
            UPDATE,
            &json!({
                "anomalyDetectorArn": arn,
                "anomalyId": anomaly,
                "suppressionType": "LIMITED",
            }),
        )
        .await;
    assert_eq!(
        code, INVALID_PARAMETER,
        "case LIMITED with no suppressionPeriod: expected {INVALID_PARAMETER}, got {code}"
    );
    assert_eq!(
        stored_suppression(&logs, &arn, &anomaly),
        None,
        "case LIMITED with no suppressionPeriod: a refused request suppresses nothing"
    );
}

/// The detector and the finding are both resolved from the store: a detector ARN the account
/// holds none under, and an id the named detector has not reported, are missing resources.
#[tokio::test]
async fn a_detector_or_a_finding_the_store_does_not_hold_is_a_missing_resource() {
    let logs = Logs::new();
    let arn = detector(&logs).await;
    let reported_anomaly = anomaly_id('a');
    let reported_pattern = pattern_id('b');
    report_finding(&logs, &arn, &reported_anomaly, &reported_pattern);

    // A second detector reports a finding of its OWN, so an id is resolved against the
    // detector the request acts on rather than against the store as a whole.
    let other = logs
        .ok(
            "CreateLogAnomalyDetector",
            &json!({
                "logGroupArnList": [log_group_arn(REGION, GROUP, false)],
                "detectorName": "another detector",
            }),
        )
        .await;
    let other_arn = other
        .get("anomalyDetectorArn")
        .and_then(Value::as_str)
        .expect("the second detector carries an ARN")
        .to_string();
    let other_anomaly = anomaly_id('c');
    let other_pattern = pattern_id('d');
    report_finding(&logs, &other_arn, &other_anomaly, &other_pattern);

    // (case, the request, the message the refusal carries)
    let rows: Vec<(String, Value, String)> = vec![
        (
            "an ARN no detector is stored under".to_string(),
            json!({
                "anomalyDetectorArn": format!("{arn}-absent"),
                "anomalyId": reported_anomaly,
                "suppressionType": SUPPRESSION_TYPE_INFINITE,
            }),
            format!("The specified ARN does not exist or already deleted : {arn}-absent"),
        ),
        (
            "an ARN of another resource kind".to_string(),
            json!({
                "anomalyDetectorArn": log_group_arn(REGION, GROUP, false),
                "anomalyId": reported_anomaly,
                "suppressionType": SUPPRESSION_TYPE_INFINITE,
            }),
            format!(
                "The specified ARN does not exist or already deleted : {}",
                log_group_arn(REGION, GROUP, false)
            ),
        ),
        (
            "an anomalyId this detector has not reported".to_string(),
            json!({
                "anomalyDetectorArn": arn,
                "anomalyId": anomaly_id('z'),
                "suppressionType": SUPPRESSION_TYPE_INFINITE,
            }),
            format!("No matching anomaly found for anomalyId: {}", anomaly_id('z')),
        ),
        (
            "a patternId this detector has not reported".to_string(),
            json!({
                "anomalyDetectorArn": arn,
                "patternId": pattern_id('z'),
                "suppressionType": SUPPRESSION_TYPE_INFINITE,
            }),
            format!("No matching anomaly found for patternId: {}", pattern_id('z')),
        ),
        (
            "an anomalyId another detector reported".to_string(),
            json!({
                "anomalyDetectorArn": arn,
                "anomalyId": other_anomaly,
                "suppressionType": SUPPRESSION_TYPE_INFINITE,
            }),
            format!("No matching anomaly found for anomalyId: {other_anomaly}"),
        ),
        (
            "a patternId another detector reported".to_string(),
            json!({
                "anomalyDetectorArn": arn,
                "patternId": other_pattern,
                "suppressionType": SUPPRESSION_TYPE_INFINITE,
            }),
            format!("No matching anomaly found for patternId: {other_pattern}"),
        ),
    ];
    for (case, request, expected) in rows {
        let (code, message) = logs.refusal(UPDATE, &request).await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case}: expected {NOT_FOUND}, got {code} / {message}"
        );
        assert_eq!(
            message, expected,
            "case {case}: expected {expected:?}, got {message:?}"
        );
    }
}

/// A finding the detector reported is suppressed on the stored record: the rows span every
/// `SuppressionType`, every `SuppressionUnit`, both selectors and `baseline` either way, and
/// a later call that omits the suppression members ends the suppression.
#[tokio::test]
async fn a_reported_finding_is_suppressed_and_the_suppression_can_be_ended() {
    let logs = Logs::new();
    let arn = detector(&logs).await;
    let anomaly = anomaly_id('a');
    let pattern = pattern_id('b');
    report_finding(&logs, &arn, &anomaly, &pattern);

    // (case, the selector member, its value)
    let selectors: [(&str, &str, &str); 2] = [
        ("by anomalyId", "anomalyId", anomaly.as_str()),
        ("by patternId", "patternId", pattern.as_str()),
    ];
    for (selector_case, member, id) in selectors {
        for suppression_type in SUPPRESSION_TYPES {
            for (unit, unit_ms) in SUPPRESSION_UNITS {
                for baseline in [false, true] {
                    let case = format!(
                        "{selector_case}, {suppression_type} for 2 {unit}, baseline {baseline}"
                    );
                    let before = now_ms();
                    logs.ok(
                        UPDATE,
                        &json!({
                            "anomalyDetectorArn": arn,
                            member: id,
                            "suppressionType": suppression_type,
                            "suppressionPeriod": {"value": 2, "suppressionUnit": unit},
                            "baseline": baseline,
                        }),
                    )
                    .await;
                    let stored = stored_suppression(&logs, &arn, id).unwrap_or_else(|| {
                        panic!("case {case}: the suppression is stored on the detector")
                    });
                    assert_eq!(
                        stored.0.as_deref(),
                        Some(suppression_type),
                        "case {case}: the stored suppression carries the type the call named, got {stored:?}"
                    );
                    assert_eq!(
                        stored.2, baseline,
                        "case {case}: the stored suppression carries the baseline flag, got {stored:?}"
                    );
                    // "If you specify INFINITE, any value for suppressionPeriod is ignored."
                    if suppression_type == SUPPRESSION_TYPE_INFINITE {
                        assert_eq!(
                            stored.1, None,
                            "case {case}: an infinite suppression does not run out, got {stored:?}"
                        );
                    } else {
                        let until = stored.1.unwrap_or_else(|| {
                            panic!("case {case}: a limited suppression runs out, got {stored:?}")
                        });
                        assert!(
                            until >= before + 2 * unit_ms,
                            "case {case}: expected the suppression to last 2 {unit} from the \
                             call, got {until} against {before}"
                        );
                    }

                    // The write is proved through the reader a client has: `ListAnomalies`
                    // reports the suppression in force over the anomaly.
                    let reported = listed_anomalies(&logs, &json!({"anomalyDetectorArn": arn}))
                        .await
                        .into_iter()
                        .next()
                        .unwrap_or_else(|| {
                            panic!("case {case}: the detector's anomaly is reported back")
                        });
                    assert_eq!(
                        reported.get("suppressed").and_then(Value::as_bool),
                        Some(true),
                        "case {case}: the anomaly is reported as suppressed, got {reported}"
                    );
                    assert_eq!(
                        reported
                            .get("isPatternLevelSuppression")
                            .and_then(Value::as_bool),
                        Some(member == "patternId"),
                        "case {case}: a suppression placed on the pattern is reported as \
                         pattern-level, got {reported}"
                    );
                    assert_eq!(
                        reported.get("suppressedDate").and_then(Value::as_i64),
                        stored_suppression(&logs, &arn, id).map(|stored| stored.3),
                        "case {case}: the reported suppressedDate is the instant it was \
                         placed, got {reported}"
                    );

                    // "If you have previously used this operation to suppress detection of a
                    // pattern or anomaly, you can use it again to cause CloudWatch Logs to
                    // end the suppression. To do this […] omit the suppressionType and
                    // suppressionPeriod parameters."
                    logs.ok(
                        UPDATE,
                        &json!({"anomalyDetectorArn": arn, member: id}),
                    )
                    .await;
                    assert_eq!(
                        stored_suppression(&logs, &arn, id),
                        None,
                        "case {case}: omitting both suppression members ends the suppression"
                    );
                    let reported = listed_anomalies(&logs, &json!({"anomalyDetectorArn": arn}))
                        .await
                        .into_iter()
                        .next()
                        .unwrap_or_else(|| {
                            panic!("case {case}: the detector's anomaly is still reported")
                        });
                    assert_eq!(
                        reported.get("suppressed").and_then(Value::as_bool),
                        Some(false),
                        "case {case}: the anomaly is reported as unsuppressed again, got {reported}"
                    );
                }
            }
        }
    }
}

/// `ListAnomalies` reports the anomalies the account's detectors have found: the whole
/// account's or one detector's, filtered by the suppression state in force, a page at a
/// time. The rows differ along which detector is selected, which state is asked for and how
/// large a page is.
#[tokio::test]
async fn the_reported_anomalies_are_filtered_by_detector_and_suppression_state() {
    let logs = Logs::new();
    let arn = detector(&logs).await;
    let other = logs
        .ok(
            "CreateLogAnomalyDetector",
            &json!({
                "logGroupArnList": [log_group_arn(REGION, GROUP, false)],
                "detectorName": "second detector",
            }),
        )
        .await;
    let other_arn = other
        .get("anomalyDetectorArn")
        .and_then(Value::as_str)
        .expect("the second detector carries an ARN")
        .to_string();

    // Three findings on the first detector, one on the second.
    let first_three = [anomaly_id('1'), anomaly_id('2'), anomaly_id('3')];
    for (index, anomaly) in first_three.iter().enumerate() {
        let pattern = pattern_id(char::from_digit(index as u32 + 1, 10).unwrap_or('x'));
        report_finding(&logs, &arn, anomaly, &pattern);
    }
    let on_other = anomaly_id('9');
    report_finding(&logs, &other_arn, &on_other, &pattern_id('9'));

    // A detector with no finding of its own reports none, and the account-wide read reports
    // every detector's.
    let ids = |listed: &[Value]| -> Vec<String> {
        listed
            .iter()
            .map(|anomaly| {
                anomaly
                    .get("anomalyId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    };
    let listed = listed_anomalies(&logs, &json!({})).await;
    let mut expected: Vec<String> = first_three.to_vec();
    expected.push(on_other.clone());
    expected.sort();
    let mut reported = ids(&listed);
    reported.sort();
    assert_eq!(
        reported, expected,
        "case the whole account: every detector's anomalies are reported, got {listed:?}"
    );
    assert_eq!(
        ids(&listed_anomalies(&logs, &json!({"anomalyDetectorArn": other_arn})).await),
        vec![on_other.clone()],
        "case one detector: only its own anomalies are reported"
    );

    // Suppress one of the three, then read each suppression state back.
    logs.ok(
        UPDATE,
        &json!({
            "anomalyDetectorArn": arn,
            "anomalyId": first_three[1],
            "suppressionType": SUPPRESSION_TYPE_INFINITE,
        }),
    )
    .await;

    // (case, the suppressionState asked for, the anomalies of the first detector expected)
    let states: [(&str, Option<&str>, Vec<String>); 3] = [
        (
            "no state at all",
            None,
            first_three.to_vec(),
        ),
        (
            "only the suppressed ones",
            Some("SUPPRESSED"),
            vec![first_three[1].clone()],
        ),
        (
            "only the unsuppressed ones",
            Some("UNSUPPRESSED"),
            vec![first_three[0].clone(), first_three[2].clone()],
        ),
    ];
    for (case, state, expected) in states {
        let mut request = json!({"anomalyDetectorArn": arn});
        if let Some(state) = state {
            request["suppressionState"] = json!(state);
        }
        let listed = listed_anomalies(&logs, &request).await;
        assert_eq!(
            ids(&listed),
            expected,
            "case {case}: expected {expected:?}, got {listed:?}"
        );
    }

    // One page at a time, in the order the enumeration holds: every anomaly exactly once.
    for page_size in [1, 2, 3] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({"anomalyDetectorArn": arn, "limit": page_size});
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let page = logs.ok("ListAnomalies", &request).await;
            let listed = page
                .get("anomalies")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert!(
                listed.len() <= page_size,
                "case limit {page_size}: a page carries at most {page_size} anomalies, got {page}"
            );
            collected.extend(ids(&listed));
            token = page
                .get("nextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            collected,
            first_three.to_vec(),
            "case limit {page_size}: paging reports every anomaly exactly once"
        );
    }

    // A detector the account does not hold is a missing resource, and a token this operation
    // never minted is a bad parameter.
    let (code, message) = logs
        .refusal(
            "ListAnomalies",
            &json!({"anomalyDetectorArn": format!("{arn}-absent")}),
        )
        .await;
    assert_eq!(
        code, NOT_FOUND,
        "case an ARN no detector is stored under: expected {NOT_FOUND}, got {code} / {message}"
    );
    assert_eq!(
        message,
        format!("The specified ARN does not exist or already deleted : {arn}-absent"),
        "case an ARN no detector is stored under: expected the sentence naming the ARN that \
         was not found, got {message:?}"
    );
    let (code, _) = logs
        .refusal("ListAnomalies", &json!({"nextToken": "not-a-token"}))
        .await;
    assert_eq!(
        code, INVALID_PARAMETER,
        "case a token this operation never minted: expected {INVALID_PARAMETER}, got {code}"
    );
}
