// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListAnomalies`, the reader of the anomalies an account's log
//! anomaly detectors have found: "Returns a list of anomalies that log anomaly detectors
//! have found."
//!
//! The rules under test:
//!
//! * an anomaly's reported `state` is DERIVED from the suppression in force at the time of
//!   the call, never read off the stored finding: "If it is still being treated as an
//!   anomaly, the value is Active. If you have suppressed this anomaly by using the
//!   UpdateAnomaly operation, the value is Suppressed. If this behavior is now considered to
//!   be normal, the value is Baseline." (`Anomaly$state`) — so a suppression placed as
//!   baseline behavior reports `Baseline`, any other suppression reports `Suppressed`, and a
//!   suppression a later call ended or the clock ran out of leaves the anomaly `Active`
//!   again;
//! * one page carries at most the `limit` the request named, and "if you don't specify a
//!   value, the default maximum value of 50 items is used" (`ListAnomaliesInput$limit`,
//!   `@range(1..50)`);
//! * `nextToken` is "the token for the next set of items to return", so it comes back only
//!   while anomalies remain past the page handed out, and walking it reports every anomaly
//!   of the selected detectors exactly once, in one total order;
//! * that token is a position within ONE filtered enumeration, so a continuation that
//!   changes a filter — and a token another paginated operation minted — is a bad parameter.
//!
//! What the enumeration SELECTS (the whole account's anomalies, one detector's, or only the
//! suppressed or unsuppressed ones) is the subject of `tests/all/anomaly_suppression.rs`,
//! which is also where the writes these reads report are made.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so an
//! answer that is not computed from the shared store cannot pass.
//!
//! The tables span what the rules generalise over rather than one request: every
//! `SuppressionType` with `baseline` set, unset and absent, a suppression ended by a call
//! and one ended by the clock, a suppression placed on the anomaly and one placed on its
//! pattern, page sizes at both published bounds of the `limit` range and one past each of
//! them, the default page over fewer, exactly and more than 50 findings, and tokens carried
//! back into a request whose filters changed in each way one can.

use serde_json::{json, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    log_group_arn, ComAmazonawsCloudwatchlogsLogs20140328State, ANOMALY_STATE_ACTIVE,
    ANOMALY_STATE_BASELINE, ANOMALY_STATE_SUPPRESSED, SUPPRESSION_TYPE_INFINITE,
};

use crate::harness::{now_ms, report_anomaly, Logs, REGION};

/// The operation under test, and the one that places the suppressions it reports.
const LIST: &str = "ListAnomalies";
const UPDATE: &str = "UpdateAnomaly";

/// The code `ListAnomalies` answers a parameter it will not accept with ("A parameter is
/// specified incorrectly") — the refusal shape it declares, with no `ValidationException`
/// in its modelled error set.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The log group the detectors in these cases watch.
const GROUP: &str = "list-anomalies/watched";

/// How many anomalies one page carries when the request names no `limit`: "The maximum
/// number of items to return. If you don't specify a value, the default maximum value of 50
/// items is used." (`ListAnomaliesInput$limit`)
const DEFAULT_LIMIT: usize = 50;

/// The published bounds of `ListAnomaliesLimit`. Source:
/// `smithy_query(query="shape",shape="ListAnomaliesLimit")` — `@range(min 1, max 50)`.
const SMALLEST_PAGE: i64 = 1;
const LARGEST_PAGE: i64 = 50;

/// An `anomalyId` of the length the model fixes it at (`@length(36..36)`), ordered by
/// `index`: the enumeration's order is the `anomalyId` order, so a zero-padded index makes
/// the order a case can predict.
fn anomaly_id(index: usize) -> String {
    format!("anomaly-{index:028}")
}

/// A `patternId` of the length the model fixes it at (`@length(32..32)`).
fn pattern_id(index: usize) -> String {
    format!("pattern-{index:024}")
}

/// The log group every detector here watches, created through the API.
async fn watched_group(logs: &Logs) {
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
}

/// One anomaly detector over [`GROUP`], created through the API; answers its ARN.
async fn detector(logs: &Logs, name: &str) -> String {
    let created = logs
        .ok(
            "CreateLogAnomalyDetector",
            &json!({
                "logGroupArnList": [log_group_arn(REGION, GROUP, false)],
                "detectorName": name,
            }),
        )
        .await;
    created
        .get("anomalyDetectorArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateLogAnomalyDetector hands back an ARN, got {created}"))
        .to_string()
}

/// One `ListAnomalies` page: the anomalies it listed and the token it handed back.
async fn page(logs: &Logs, request: &Value) -> (Vec<Value>, Option<String>) {
    let answered = logs.ok(LIST, request).await;
    let anomalies = answered
        .get("anomalies")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{request}: ListAnomalies returns anomalies, got {answered}"))
        .clone();
    let token = answered
        .get("nextToken")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    (anomalies, token)
}

/// The `anomalyId`s of the anomalies in one page, in the order it listed them.
fn ids(listed: &[Value]) -> Vec<String> {
    listed
        .iter()
        .map(|anomaly| member(anomaly, "anomalyId"))
        .collect()
}

/// One string member of a reported anomaly.
fn member(anomaly: &Value, name: &str) -> String {
    anomaly
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a reported anomaly carries {name}, got {anomaly}"))
        .to_string()
}

/// `request`, with every member of `extra` added to it.
fn with(request: &Value, extra: &Value) -> Value {
    let mut merged = request.clone();
    if let (Some(target), Some(source)) = (merged.as_object_mut(), extra.as_object()) {
        for (member, value) in source {
            target.insert(member.clone(), value.clone());
        }
    }
    merged
}

/// Move the instant the suppression on `id` was due to run out into the past, so the clock
/// has already ended it. Only the clock can end a `LIMITED` suppression and a test cannot
/// wait one out, so the stored due instant is shifted instead.
fn expire_suppression(logs: &Logs, arn: &str, id: &str) {
    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let mut store = state.lock();
    let suppression = store
        .anomaly_detectors
        .get_mut(arn)
        .and_then(|detector| detector.suppressions.get_mut(id))
        .unwrap_or_else(|| panic!("a suppression is stored for {id}"));
    suppression.suppressed_until_ms = Some(now_ms() - 1_000);
}

/// The one anomaly the detector under `arn` reports, read back over the wire.
async fn only_anomaly(logs: &Logs, arn: &str, case: &str) -> Value {
    let (listed, token) = page(logs, &json!({"anomalyDetectorArn": arn})).await;
    assert_eq!(
        token, None,
        "case {case}: a page that holds every anomaly hands back no token, got {token:?}"
    );
    assert_eq!(
        listed.len(),
        1,
        "case {case}: the detector reports exactly one anomaly, got {listed:?}"
    );
    listed.into_iter().next().unwrap_or(Value::Null)
}

/// The reported `state` of an anomaly is the one the suppression in force makes it, and it
/// goes back to `Active` when a call or the clock ends that suppression.
#[tokio::test]
async fn the_reported_state_follows_the_suppression_in_force() {
    let logs = Logs::new();
    watched_group(&logs).await;
    let arn = detector(&logs, "state detector").await;
    let anomaly = anomaly_id(0);
    report_anomaly(&logs, &arn, &anomaly, &pattern_id(0));

    // An anomaly nothing has suppressed is still being treated as an anomaly.
    let reported = only_anomaly(&logs, &arn, "no suppression at all").await;
    assert_eq!(
        member(&reported, "state"),
        ANOMALY_STATE_ACTIVE,
        "case no suppression at all: expected {ANOMALY_STATE_ACTIVE}, got {reported}"
    );
    assert_eq!(
        reported.get("suppressed").and_then(Value::as_bool),
        Some(false),
        "case no suppression at all: the anomaly is not suppressed, got {reported}"
    );

    // (case, the suppression members the request names, the state that suppression makes)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "an infinite suppression",
            json!({"suppressionType": SUPPRESSION_TYPE_INFINITE}),
            ANOMALY_STATE_SUPPRESSED,
        ),
        (
            "a limited suppression",
            json!({
                "suppressionType": "LIMITED",
                "suppressionPeriod": {"value": 2, "suppressionUnit": "HOURS"},
            }),
            ANOMALY_STATE_SUPPRESSED,
        ),
        (
            "an infinite suppression that is explicitly not baseline behavior",
            json!({"suppressionType": SUPPRESSION_TYPE_INFINITE, "baseline": false}),
            ANOMALY_STATE_SUPPRESSED,
        ),
        (
            "an infinite suppression of baseline behavior",
            json!({"suppressionType": SUPPRESSION_TYPE_INFINITE, "baseline": true}),
            ANOMALY_STATE_BASELINE,
        ),
        (
            "a limited suppression of baseline behavior",
            json!({
                "suppressionType": "LIMITED",
                "suppressionPeriod": {"value": 30, "suppressionUnit": "MINUTES"},
                "baseline": true,
            }),
            ANOMALY_STATE_BASELINE,
        ),
    ];
    for (case, suppression, expected) in rows {
        let selector = json!({"anomalyDetectorArn": arn, "anomalyId": anomaly});
        logs.ok(UPDATE, &with(&selector, &suppression)).await;
        let reported = only_anomaly(&logs, &arn, case).await;
        assert_eq!(
            member(&reported, "state"),
            expected,
            "case {case}: expected state {expected}, got {reported}"
        );
        assert_eq!(
            reported.get("suppressed").and_then(Value::as_bool),
            Some(true),
            "case {case}: a suppressed anomaly is reported suppressed, got {reported}"
        );

        // "you can use it again to cause CloudWatch Logs to end the suppression. To do this
        // […] omit the suppressionType and suppressionPeriod parameters." — the anomaly is
        // then being treated as an anomaly again.
        logs.ok(UPDATE, &selector).await;
        let reported = only_anomaly(&logs, &arn, case).await;
        assert_eq!(
            member(&reported, "state"),
            ANOMALY_STATE_ACTIVE,
            "case {case}, suppression ended: expected {ANOMALY_STATE_ACTIVE}, got {reported}"
        );
        assert_eq!(
            reported.get("suppressed").and_then(Value::as_bool),
            Some(false),
            "case {case}, suppression ended: the anomaly is not suppressed, got {reported}"
        );
    }

    // A limited suppression the clock has run past is no longer in force, with no call
    // ending it.
    logs.ok(
        UPDATE,
        &json!({
            "anomalyDetectorArn": arn,
            "anomalyId": anomaly,
            "suppressionType": "LIMITED",
            "suppressionPeriod": {"value": 1, "suppressionUnit": "HOURS"},
            "baseline": true,
        }),
    )
    .await;
    expire_suppression(&logs, &arn, &anomaly);
    let case = "a limited suppression the clock has run past";
    let reported = only_anomaly(&logs, &arn, case).await;
    assert_eq!(
        member(&reported, "state"),
        ANOMALY_STATE_ACTIVE,
        "case {case}: expected {ANOMALY_STATE_ACTIVE}, got {reported}"
    );
    assert_eq!(
        reported.get("suppressed").and_then(Value::as_bool),
        Some(false),
        "case {case}: a suppression that ran out suppresses nothing, got {reported}"
    );

    // A read reports; it changes nothing a later caller sees.
    let again = only_anomaly(&logs, &arn, case).await;
    assert_eq!(
        again, reported,
        "case {case} read twice: a report does not change what a later caller reads, got \
         {again} after {reported}"
    );
}

/// A suppression placed on a PATTERN covers every anomaly of that pattern, so each of them
/// reports the state that suppression makes.
#[tokio::test]
async fn a_pattern_level_suppression_states_every_anomaly_of_the_pattern() {
    let logs = Logs::new();
    watched_group(&logs).await;
    let arn = detector(&logs, "pattern detector").await;
    let shared_pattern = pattern_id(0);
    let of_pattern = [anomaly_id(0), anomaly_id(1)];
    for anomaly in &of_pattern {
        report_anomaly(&logs, &arn, anomaly, &shared_pattern);
    }
    // One anomaly of ANOTHER pattern, which a pattern-level suppression must leave alone.
    let other = anomaly_id(2);
    report_anomaly(&logs, &arn, &other, &pattern_id(1));

    // (case, the suppression members the request names, the state of the pattern's anomalies)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a pattern suppressed infinitely",
            json!({"suppressionType": SUPPRESSION_TYPE_INFINITE}),
            ANOMALY_STATE_SUPPRESSED,
        ),
        (
            "a pattern accepted as baseline behavior",
            json!({"suppressionType": SUPPRESSION_TYPE_INFINITE, "baseline": true}),
            ANOMALY_STATE_BASELINE,
        ),
        (
            "a pattern suppressed for a period",
            json!({
                "suppressionType": "LIMITED",
                "suppressionPeriod": {"value": 10, "suppressionUnit": "SECONDS"},
            }),
            ANOMALY_STATE_SUPPRESSED,
        ),
    ];
    for (case, suppression, expected) in rows {
        let selector = json!({"anomalyDetectorArn": arn, "patternId": shared_pattern});
        logs.ok(UPDATE, &with(&selector, &suppression)).await;
        let (listed, _) = page(&logs, &json!({"anomalyDetectorArn": arn})).await;
        for anomaly in &listed {
            let id = member(anomaly, "anomalyId");
            let of_the_pattern = of_pattern.contains(&id);
            let expected_state = if of_the_pattern {
                expected
            } else {
                ANOMALY_STATE_ACTIVE
            };
            assert_eq!(
                member(anomaly, "state"),
                expected_state,
                "case {case}, anomaly {id}: expected state {expected_state}, got {anomaly}"
            );
            assert_eq!(
                anomaly
                    .get("isPatternLevelSuppression")
                    .and_then(Value::as_bool),
                Some(of_the_pattern),
                "case {case}, anomaly {id}: only the pattern's anomalies carry a \
                 pattern-level suppression, got {anomaly}"
            );
        }
        logs.ok(UPDATE, &selector).await;
    }
}

/// One page carries at most the `limit` the request named, and at most 50 when it named
/// none; a token comes back exactly while anomalies remain past the page.
#[tokio::test]
async fn a_page_carries_at_most_the_limit_and_defaults_to_fifty() {
    // (case, how many findings the detector reported, the limit the request names)
    let rows: Vec<(&str, usize, Option<i64>)> = vec![
        ("one finding, no limit", 1, None),
        ("three findings, no limit", 3, None),
        ("as many findings as the default page", DEFAULT_LIMIT, None),
        ("one finding past the default page", DEFAULT_LIMIT + 1, None),
        ("twice the default page", DEFAULT_LIMIT * 2, None),
        ("the smallest page the model admits", 3, Some(SMALLEST_PAGE)),
        ("a page of two", 3, Some(2)),
        ("a page as large as the findings", 3, Some(3)),
        (
            "the largest page the model admits, over more findings",
            DEFAULT_LIMIT + 1,
            Some(LARGEST_PAGE),
        ),
        (
            "the largest page the model admits, over fewer findings",
            3,
            Some(LARGEST_PAGE),
        ),
    ];
    for (case, findings, limit) in rows {
        let logs = Logs::new();
        watched_group(&logs).await;
        let arn = detector(&logs, "paging detector").await;
        for index in 0..findings {
            report_anomaly(&logs, &arn, &anomaly_id(index), &pattern_id(index));
        }

        let mut request = json!({"anomalyDetectorArn": arn});
        if let Some(limit) = limit {
            request["limit"] = json!(limit);
        }
        // The page is as large as the request asked for, or the default when it asked for
        // nothing, and never larger than what the detector has reported.
        let expected_page = match limit {
            Some(limit) => usize::try_from(limit).unwrap_or(0),
            None => DEFAULT_LIMIT,
        }
        .min(findings);
        let (listed, token) = page(&logs, &request).await;
        assert_eq!(
            listed.len(),
            expected_page,
            "case {case}: expected {expected_page} anomalies on the first page, got \
             {} ({listed:?})",
            listed.len()
        );
        assert_eq!(
            token.is_some(),
            expected_page < findings,
            "case {case}: a token comes back exactly while anomalies remain past the page, \
             got {token:?} with {expected_page} of {findings} reported"
        );
    }

    // The published bounds of `limit` are the page sizes above; one past either of them is
    // refused for the constraint it failed, and nothing is listed.
    let logs = Logs::new();
    // (case, the limit the request names, the constraint clause the refusal states)
    let out_of_range: [(&str, i64, String); 2] = [
        (
            "one below the smallest page",
            SMALLEST_PAGE - 1,
            format!("Member must have value greater than or equal to {SMALLEST_PAGE}"),
        ),
        (
            "one past the largest page",
            LARGEST_PAGE + 1,
            format!("Member must have value less than or equal to {LARGEST_PAGE}"),
        ),
    ];
    for (case, limit, clause) in out_of_range {
        let (code, message) = logs.refusal(LIST, &json!({"limit": limit})).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case}: expected {INVALID_PARAMETER}, got {code} / {message}"
        );
        assert_eq!(
            message,
            format!(
                "1 validation error detected: Value at 'limit' failed to satisfy constraint: \
                 {clause}"
            ),
            "case {case}: the refusal names the position and the constraint, got {message:?}"
        );
    }
}

/// Walking `nextToken` reports every anomaly of the selected detectors exactly once, in the
/// one order the unpaged read reports them in.
#[tokio::test]
async fn paging_walks_every_anomaly_of_the_account_exactly_once() {
    let logs = Logs::new();
    watched_group(&logs).await;
    // Three detectors reporting a different number of findings each, so a page boundary
    // falls inside one detector's findings, on a detector boundary, and past the last one.
    let detectors = [
        (detector(&logs, "first detector").await, 2usize),
        (detector(&logs, "second detector").await, 3),
        (detector(&logs, "third detector").await, 1),
    ];
    let mut index = 0;
    for (arn, findings) in &detectors {
        for _ in 0..*findings {
            report_anomaly(&logs, arn, &anomaly_id(index), &pattern_id(index));
            index += 1;
        }
    }

    // The order is a total order over (detector, anomaly), so the unpaged read is sorted by
    // the detector's ARN and then the anomaly's id.
    let (whole_account, token) = page(&logs, &json!({})).await;
    assert_eq!(
        token, None,
        "case the whole account: every anomaly fits one default page, got {token:?}"
    );
    let keys: Vec<(String, String)> = whole_account
        .iter()
        .map(|anomaly| {
            (
                member(anomaly, "anomalyDetectorArn"),
                member(anomaly, "anomalyId"),
            )
        })
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(
        keys, sorted,
        "case the whole account: the anomalies are reported in one total order, got {keys:?}"
    );

    // (case, the request the enumeration starts from, the anomalies it reports)
    let mut enumerations: Vec<(String, Value, Vec<String>)> = vec![(
        "the whole account".to_string(),
        json!({}),
        ids(&whole_account),
    )];
    for (arn, _) in &detectors {
        let (listed, _) = page(&logs, &json!({"anomalyDetectorArn": arn})).await;
        enumerations.push((
            format!("the detector {arn}"),
            json!({"anomalyDetectorArn": arn}),
            ids(&listed),
        ));
    }

    for (case, request, expected) in enumerations {
        for limit in [
            None,
            Some(SMALLEST_PAGE),
            Some(2),
            Some(3),
            Some(LARGEST_PAGE),
        ] {
            let mut collected: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut paged = request.clone();
                if let Some(limit) = limit {
                    paged["limit"] = json!(limit);
                }
                if let Some(token) = &token {
                    paged["nextToken"] = json!(token);
                }
                let (listed, next) = page(&logs, &paged).await;
                if let Some(limit) = limit {
                    assert!(
                        i64::try_from(listed.len()).unwrap_or(i64::MAX) <= limit,
                        "case {case} at limit {limit}: a page carries at most {limit} \
                         anomalies, got {}",
                        listed.len()
                    );
                }
                collected.extend(ids(&listed));
                token = next;
                pages += 1;
                assert!(
                    pages <= expected.len() + 1,
                    "case {case} at limit {limit:?}: paging terminates, got {pages} pages \
                     for {} anomalies",
                    expected.len()
                );
                if token.is_none() {
                    break;
                }
            }
            assert_eq!(
                collected, expected,
                "case {case} at limit {limit:?}: expected {expected:?}, got {collected:?}"
            );
        }
    }
}

/// A `nextToken` is a position within ONE filtered enumeration: carried back into a request
/// whose filters are the same it continues that enumeration, and anything else is refused.
#[tokio::test]
async fn a_token_is_a_position_in_one_filtered_enumeration() {
    let logs = Logs::new();
    watched_group(&logs).await;
    let arn = detector(&logs, "token detector").await;
    let other_arn = detector(&logs, "other token detector").await;
    for index in 0..3 {
        report_anomaly(&logs, &arn, &anomaly_id(index), &pattern_id(index));
    }
    report_anomaly(&logs, &other_arn, &anomaly_id(9), &pattern_id(9));
    // One of the three is suppressed, so the suppression-state filters select different
    // enumerations and a token minted in one cannot be read in another.
    logs.ok(
        UPDATE,
        &json!({
            "anomalyDetectorArn": arn,
            "anomalyId": anomaly_id(1),
            "suppressionType": SUPPRESSION_TYPE_INFINITE,
        }),
    )
    .await;

    let first = json!({"anomalyDetectorArn": arn, "limit": SMALLEST_PAGE});
    let (listed, token) = page(&logs, &first).await;
    let token = token.unwrap_or_else(|| panic!("a first page of one of three hands back a token"));
    assert_eq!(
        ids(&listed),
        vec![anomaly_id(0)],
        "case the first page: the enumeration starts at its first anomaly, got {listed:?}"
    );

    // The same filters continue the same enumeration.
    let (continued, _) = page(&logs, &with(&first, &json!({"nextToken": token}))).await;
    assert_eq!(
        ids(&continued),
        vec![anomaly_id(1)],
        "case the same filters: the token continues that enumeration, got {continued:?}"
    );

    // A token DescribeLogGroups minted, for an enumeration of another resource altogether.
    for group in ["list-anomalies/first", "list-anomalies/second"] {
        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
    }
    let groups = logs.ok("DescribeLogGroups", &json!({"limit": 1})).await;
    let foreign = groups
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("DescribeLogGroups pages, got {groups}"))
        .to_string();

    // (case, the request the token is carried back into)
    let refused: Vec<(String, Value)> = vec![
        (
            "a continuation naming another detector".to_string(),
            json!({"anomalyDetectorArn": other_arn, "nextToken": token}),
        ),
        (
            "a continuation naming no detector at all".to_string(),
            json!({"nextToken": token}),
        ),
        (
            "a continuation that adds a suppression state".to_string(),
            json!({
                "anomalyDetectorArn": arn,
                "suppressionState": "UNSUPPRESSED",
                "nextToken": token,
            }),
        ),
        (
            "a continuation that adds the other suppression state".to_string(),
            json!({
                "anomalyDetectorArn": arn,
                "suppressionState": "SUPPRESSED",
                "nextToken": token,
            }),
        ),
        (
            "a token another paginated operation minted".to_string(),
            json!({"anomalyDetectorArn": arn, "nextToken": foreign}),
        ),
        (
            "a value that is not a token at all".to_string(),
            json!({"anomalyDetectorArn": arn, "nextToken": "not-a-token"}),
        ),
    ];
    for (case, request) in refused {
        let (code, message) = logs.refusal(LIST, &request).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case}: expected {INVALID_PARAMETER}, got {code} / {message}"
        );
    }
}
