// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateLogAnomalyDetector`, proved through the read it is observed
//! by, `GetLogAnomalyDetector`.
//!
//! The rule under test: an update writes, onto the stored detector the `anomalyDetectorArn`
//! identifies, every configuration member the request names, leaves the members it does not
//! name holding the value they already held, puts the detector in the paused or the running
//! state the required `enabled` member asks for, and stamps it as modified; an update of a
//! detector the account does not hold, or one carrying a value the model or the
//! filter-pattern syntax does not admit, changes nothing.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The table spans what the rule generalises over rather than one request: an update naming
//! no configuration member, each member on its own and all of them at once, EVERY value of
//! the model's `EvaluationFrequency` enum (source:
//! `smithy_query(query="shape",shape="EvaluationFrequency")`), `anomalyVisibilityTime` at
//! both published bounds of its `@range(7..90)`, a value between them and one past each
//! bound, each form a `filterPattern` takes (empty, a term, a bracketed one) and an
//! unbalanced one, and `enabled` both ways over a detector in each status of its lifecycle.
//! The second update of one detector is driven from the first update's values, so "a member
//! the request does not name keeps what it held" is asserted against a value an update
//! wrote and not only against the create's.

use serde_json::{json, Map, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    anomaly_detector_arn, log_group_arn, AnomalyDetectorRecord,
    ComAmazonawsCloudwatchlogsLogs20140328State, ANOMALY_DETECTOR_ANALYSIS_DELAY_MS,
    ANOMALY_DETECTOR_STATUS_ANALYZING, ANOMALY_DETECTOR_STATUS_INITIALIZING,
    ANOMALY_DETECTOR_STATUS_PAUSED, ANOMALY_DETECTOR_STATUS_TRAINING,
    ANOMALY_DETECTOR_TRAINING_DELAY_MS,
};

use crate::harness::{now_ms, Logs, REGION};

/// The operation under test, the one that creates what it updates, and the one the update
/// is read back through.
const UPDATE: &str = "UpdateLogAnomalyDetector";
const CREATE: &str = "CreateLogAnomalyDetector";
const GET: &str = "GetLogAnomalyDetector";

/// The log group the detectors of these cases watch.
const GROUP: &str = "anomaly-detector-updates/watched";

/// The code CloudWatch Logs answers for a parameter it will not accept, which is what
/// `UpdateLogAnomalyDetector` declares for a refusal ("A parameter is specified
/// incorrectly").
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The code answered when the resource a request names is not there.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// Every value of the model's `EvaluationFrequency` enum, in the order the model lists them.
/// Source: `smithy_query(query="shape",shape="EvaluationFrequency")`.
const EVALUATION_FREQUENCIES: [&str; 6] = [
    "ONE_MIN",
    "FIVE_MIN",
    "TEN_MIN",
    "FIFTEEN_MIN",
    "THIRTY_MIN",
    "ONE_HOUR",
];

/// The configuration a detector is created with, so every row starts from a detector that
/// already holds each member an update can replace — which is what makes "a member the
/// request does not name is left as it was" observable.
const CREATED_FREQUENCY: &str = "ONE_HOUR";
const CREATED_PATTERN: &str = "[ip, user, timestamp]";
const CREATED_VISIBILITY: i64 = 21;

/// The configuration an update names, member by member. `None` is a member the request
/// leaves out, which the detector must keep the value it already held for.
#[derive(Clone, Copy)]
struct Row {
    /// What this row varies, for the assertion messages.
    case: &'static str,
    evaluation_frequency: Option<&'static str>,
    filter_pattern: Option<&'static str>,
    anomaly_visibility_time: Option<i64>,
    /// `UpdateLogAnomalyDetector$enabled` is `@required`, so every row states it.
    enabled: bool,
}

impl Row {
    /// A row naming nothing but the required members.
    const fn bare(case: &'static str, enabled: bool) -> Self {
        Self {
            case,
            evaluation_frequency: None,
            filter_pattern: None,
            anomaly_visibility_time: None,
            enabled,
        }
    }
}

/// The accepted corpus: an update naming no configuration member at all (both ways round on
/// `enabled`), each member on its own, all of them at once, one row per
/// `EvaluationFrequency` value, one per published bound of `anomalyVisibilityTime` and one
/// between them, and each form a `filterPattern` takes.
fn rows() -> Vec<Row> {
    let mut rows = vec![
        Row::bare("nothing but enabled, left running", true),
        Row::bare("nothing but enabled, paused", false),
        Row {
            filter_pattern: Some(""),
            ..Row::bare("the empty filter pattern, which FilterPattern's length 0.. admits", true)
        },
        Row {
            filter_pattern: Some("ERROR"),
            ..Row::bare("a term filter pattern", true)
        },
        Row {
            filter_pattern: Some("{ $.level = \"ERROR\" }"),
            ..Row::bare("a JSON filter pattern", false)
        },
        Row {
            evaluation_frequency: Some("FIVE_MIN"),
            filter_pattern: Some("[host, code]"),
            anomaly_visibility_time: Some(45),
            ..Row::bare("every configuration member at once", true)
        },
    ];
    for frequency in EVALUATION_FREQUENCIES {
        rows.push(Row {
            evaluation_frequency: Some(frequency),
            ..Row::bare(frequency, true)
        });
    }
    // `AnomalyVisibilityTime` is `@range(min: 7, max: 90)`: both published bounds, and a
    // value between them.
    for days in [7, 50, 90] {
        rows.push(Row {
            anomaly_visibility_time: Some(days),
            ..Row::bare("an anomaly visibility time", true)
        });
    }
    rows
}

/// The update body for one row: the detector's ARN and `enabled`, plus only the
/// configuration members the row names, so an absent member is absent on the wire too.
fn update_body(arn: &str, row: &Row) -> Value {
    let mut body = Map::new();
    body.insert("anomalyDetectorArn".to_string(), json!(arn));
    body.insert("enabled".to_string(), json!(row.enabled));
    if let Some(frequency) = row.evaluation_frequency {
        body.insert("evaluationFrequency".to_string(), json!(frequency));
    }
    if let Some(pattern) = row.filter_pattern {
        body.insert("filterPattern".to_string(), json!(pattern));
    }
    if let Some(days) = row.anomaly_visibility_time {
        body.insert("anomalyVisibilityTime".to_string(), json!(days));
    }
    Value::Object(body)
}

/// One string member of a response, or `None` when the response does not carry it.
fn member<'a>(body: &'a Value, key: &str) -> Option<&'a str> {
    body.get(key).and_then(Value::as_str)
}

/// One epoch-millisecond member of a response.
fn stamp(body: &Value, key: &str, case: &str) -> i64 {
    body.get(key)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: a detector carries {key}, got {body}"))
}

/// The status a detector carries, which every `AnomalyDetector` reader reports.
fn status(body: &Value, case: &str) -> String {
    member(body, "anomalyDetectorStatus")
        .unwrap_or_else(|| {
            panic!("case {case:?}: a detector carries anomalyDetectorStatus, got {body}")
        })
        .to_string()
}

/// The statuses the clock admits for a RUNNING detector created at `creation_ms` and read
/// somewhere between `read_from_ms` and `read_to_ms`: initializing until training is due,
/// training until analysis is due, analyzing after that.
///
/// A read window that straddles a due instant admits the statuses on both sides of it, so a
/// case asserts what the clock allows and never races it.
fn admissible_statuses(creation_ms: i64, read_from_ms: i64, read_to_ms: i64) -> Vec<&'static str> {
    let training_due = creation_ms + ANOMALY_DETECTOR_TRAINING_DELAY_MS;
    let analyzing_due = training_due + ANOMALY_DETECTOR_ANALYSIS_DELAY_MS;
    let mut admissible = Vec::new();
    if read_from_ms < training_due {
        admissible.push(ANOMALY_DETECTOR_STATUS_INITIALIZING);
    }
    if read_to_ms >= training_due && read_from_ms < analyzing_due {
        admissible.push(ANOMALY_DETECTOR_STATUS_TRAINING);
    }
    if read_to_ms >= analyzing_due {
        admissible.push(ANOMALY_DETECTOR_STATUS_ANALYZING);
    }
    admissible
}

/// The lifecycle the clock carries a running detector through, in order, so a case can state
/// that a restart never puts a detector BACK to an earlier one. Source:
/// `smithy_query(query="shape",shape="AnomalyDetectorStatus")`, whose other values
/// (`PAUSED`, `FAILED`, `DELETED`) are not statuses the clock moves a detector into.
const LIFECYCLE: [&str; 3] = [
    ANOMALY_DETECTOR_STATUS_INITIALIZING,
    ANOMALY_DETECTOR_STATUS_TRAINING,
    ANOMALY_DETECTOR_STATUS_ANALYZING,
];

/// How far through [`LIFECYCLE`] a status is, or `None` for one outside it.
fn lifecycle_position(status: &str) -> Option<usize> {
    LIFECYCLE.iter().position(|known| *known == status)
}

/// The log group the cases' detectors watch, created, in the ARN form the modelled
/// `LogGroupArn` pattern admits.
async fn watched_group(logs: &Logs) -> String {
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    let (_arn, log_group_arn) = logs.group_arns(GROUP).await;
    log_group_arn
}

/// One detector, created over `group` with every member an update can replace already set.
async fn created_detector(logs: &Logs, group: &str) -> String {
    let created = logs
        .ok(
            CREATE,
            &json!({
                "logGroupArnList": [group],
                "detectorName": "updated-detector",
                "evaluationFrequency": CREATED_FREQUENCY,
                "filterPattern": CREATED_PATTERN,
                "anomalyVisibilityTime": CREATED_VISIBILITY,
            }),
        )
        .await;
    member(&created, "anomalyDetectorArn")
        .unwrap_or_else(|| panic!("a create answers anomalyDetectorArn, got {created}"))
        .to_string()
}

/// An accepted update writes every member it names onto the detector, keeps the ones it does
/// not name, and leaves the detector's identity — its ARN, its name, the log group it
/// watches and when it was created — alone.
#[tokio::test]
async fn an_update_writes_the_members_it_names_and_keeps_the_rest() {
    let logs = Logs::new();
    let group = watched_group(&logs).await;

    for row in rows() {
        let case = row.case;
        let arn = created_detector(&logs, &group).await;
        let created = logs.ok(GET, &json!({"anomalyDetectorArn": arn})).await;
        let creation = stamp(&created, "creationTimeStamp", case);

        // What the detector holds before the update: the create's configuration.
        let mut expected_frequency = CREATED_FREQUENCY.to_string();
        let mut expected_pattern = CREATED_PATTERN.to_string();
        let mut expected_visibility = CREATED_VISIBILITY;

        // Two updates of the same detector: the second names the complement of the members
        // the first named, so each member is checked both after an update wrote it and after
        // a later update left it alone.
        let second = Row {
            case,
            evaluation_frequency: row.evaluation_frequency.is_none().then_some("THIRTY_MIN"),
            filter_pattern: row.filter_pattern.is_none().then_some("WARN"),
            anomaly_visibility_time: row.anomaly_visibility_time.is_none().then_some(60),
            enabled: row.enabled,
        };

        for (pass, update) in [row, second].into_iter().enumerate() {
            if let Some(frequency) = update.evaluation_frequency {
                expected_frequency = frequency.to_string();
            }
            if let Some(pattern) = update.filter_pattern {
                expected_pattern = pattern.to_string();
            }
            if let Some(days) = update.anomaly_visibility_time {
                expected_visibility = days;
            }

            let before = now_ms();
            let answer = logs.ok(UPDATE, &update_body(&arn, &update)).await;
            let after = now_ms();
            assert_eq!(
                answer,
                Value::Object(Map::new()),
                "case {case:?} pass {pass}: an update answers the modelled empty output, got \
                 {answer}"
            );

            let reported = logs.ok(GET, &json!({"anomalyDetectorArn": arn})).await;
            let read_after = now_ms();

            assert_eq!(
                member(&reported, "evaluationFrequency"),
                Some(expected_frequency.as_str()),
                "case {case:?} pass {pass}: expected evaluationFrequency \
                 {expected_frequency:?}, got {reported}"
            );
            assert_eq!(
                member(&reported, "filterPattern"),
                Some(expected_pattern.as_str()),
                "case {case:?} pass {pass}: expected filterPattern {expected_pattern:?}, got \
                 {reported}"
            );
            assert_eq!(
                reported.get("anomalyVisibilityTime").and_then(Value::as_i64),
                Some(expected_visibility),
                "case {case:?} pass {pass}: expected anomalyVisibilityTime \
                 {expected_visibility}, got {reported}"
            );

            // The update changes the detector's configuration and nothing about which
            // resource it is.
            assert_eq!(
                member(&reported, "detectorName"),
                Some("updated-detector"),
                "case {case:?} pass {pass}: an update names no detectorName, so the created \
                 name stands, got {reported}"
            );
            assert_eq!(
                reported.get("logGroupArnList"),
                Some(&json!([group])),
                "case {case:?} pass {pass}: an update names no log group, so the watched group \
                 stands, got {reported}"
            );
            assert_eq!(
                stamp(&reported, "creationTimeStamp", case),
                creation,
                "case {case:?} pass {pass}: an update does not re-create the detector, so its \
                 creation stamp stays {creation}, got {reported}"
            );

            // "when this anomaly detector was most recently modified": the update modified
            // it, so the stamp is from the update's own window.
            let modified = stamp(&reported, "lastModifiedTimeStamp", case);
            assert!(
                (before..=after).contains(&modified),
                "case {case:?} pass {pass}: expected a lastModifiedTimeStamp in \
                 {before}..={after}, got {modified}"
            );

            // "Use this parameter to pause or restart the anomaly detector": a paused
            // detector reports the paused status, and a running one reports the status the
            // clock has made due.
            let reported_status = status(&reported, case);
            if update.enabled {
                let admissible = admissible_statuses(creation, before, read_after);
                assert!(
                    admissible.contains(&reported_status.as_str()),
                    "case {case:?} pass {pass}: expected one of {admissible:?}, got \
                     {reported_status:?}"
                );
            } else {
                assert_eq!(
                    reported_status, ANOMALY_DETECTOR_STATUS_PAUSED,
                    "case {case:?} pass {pass}: an update with enabled false pauses the \
                     detector, got {reported_status:?}"
                );
            }
        }
    }
}

/// `enabled` pauses a detector and restarts it, from any status of its lifecycle: a paused
/// detector stays paused however often it is read, a restarted one is back under the clock,
/// and a restart never carries a detector further through its lifecycle than the clock had.
#[tokio::test]
async fn enabled_pauses_and_restarts_a_detector_from_any_status() {
    let logs = Logs::new();
    let group = log_group_arn(REGION, GROUP, false);
    let now = now_ms();

    // (case, how long before now the detector was created) — one detector per status the
    // clock carries a detector through, seeded through the same record constructor the
    // create uses, since the rule is about the status the detector is in when the pause
    // arrives and not about how it got there.
    let ages: [(&str, i64); 3] = [
        ("paused while still initializing", 0),
        (
            "paused after training became due",
            ANOMALY_DETECTOR_TRAINING_DELAY_MS + 1,
        ),
        ("paused an hour after it was created", 60 * 60 * 1_000),
    ];
    let mut seeded: Vec<(&str, String)> = Vec::new();
    {
        let state = logs
            .engine()
            .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
        let mut store = state.lock();
        for (index, (case, age)) in ages.into_iter().enumerate() {
            let arn = anomaly_detector_arn(
                REGION,
                &format!("1000000{index}-0000-0000-0000-000000000000"),
            );
            let record = AnomalyDetectorRecord::new(&arn, vec![group.clone()], now - age);
            store.anomaly_detectors.insert(arn.clone(), record);
            seeded.push((case, arn));
        }
    }

    for (case, arn) in &seeded {
        // Where the clock had carried the detector when the pause arrived, which a restart
        // resumes from and never rewinds behind.
        let running = logs.ok(GET, &json!({"anomalyDetectorArn": arn})).await;
        let paused_from = status(&running, case);
        let paused_from_position = lifecycle_position(&paused_from).unwrap_or_else(|| {
            panic!("case {case:?}: a detector nothing paused is in its lifecycle, got {paused_from:?}")
        });

        // Pausing twice is the same pause: the second call finds the detector already there
        // and leaves it there.
        for pass in 0..2 {
            logs.ok(UPDATE, &json!({"anomalyDetectorArn": arn, "enabled": false}))
                .await;
            let paused = logs.ok(GET, &json!({"anomalyDetectorArn": arn})).await;
            let reported = status(&paused, case);
            assert_eq!(
                reported, ANOMALY_DETECTOR_STATUS_PAUSED,
                "case {case:?} pause {pass}: expected {ANOMALY_DETECTOR_STATUS_PAUSED}, got \
                 {reported:?}"
            );
        }

        // Reading a paused detector does not restart it: only a call does.
        let read_again = logs.ok(GET, &json!({"anomalyDetectorArn": arn})).await;
        let reported = status(&read_again, case);
        assert_eq!(
            reported, ANOMALY_DETECTOR_STATUS_PAUSED,
            "case {case:?}: reading a paused detector leaves it paused, got {reported:?}"
        );

        let restart_before = now_ms();
        logs.ok(UPDATE, &json!({"anomalyDetectorArn": arn, "enabled": true}))
            .await;
        let restart_after = now_ms();
        let restarted = logs.ok(GET, &json!({"anomalyDetectorArn": arn})).await;
        let reported = status(&restarted, case);
        assert_ne!(
            reported, ANOMALY_DETECTOR_STATUS_PAUSED,
            "case {case:?}: an update with enabled true restarts the detector, got \
             {reported:?}"
        );

        // A restart puts the detector back where the pause interrupted its lifecycle: never
        // behind the status it was paused in, and never past what the clock has made due.
        let position = lifecycle_position(&reported).unwrap_or_else(|| {
            panic!("case {case:?}: a restarted detector is back in its lifecycle, got {reported:?}")
        });
        assert!(
            position >= paused_from_position,
            "case {case:?}: a restart does not rewind the lifecycle behind {paused_from:?}, \
             got {reported:?}"
        );

        // What the clock still owed the detector when it was paused is re-armed by the
        // restart rather than dropped — so a detector paused mid-lifecycle still has its
        // next status due AFTER the restart, instead of having jumped to the end of the
        // lifecycle or having lost the transition altogether.
        let status_due_ms = {
            let state = logs
                .engine()
                .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
            let store = state.lock();
            store
                .anomaly_detectors
                .get(arn)
                .unwrap_or_else(|| panic!("case {case:?}: the detector is stored under {arn}"))
                .status_due_ms
        };
        match status_due_ms {
            Some(due) => {
                // Whatever was outstanding is at most one whole span of the lifecycle.
                let latest = restart_after
                    + ANOMALY_DETECTOR_TRAINING_DELAY_MS.max(ANOMALY_DETECTOR_ANALYSIS_DELAY_MS);
                assert!(
                    due > restart_before && due <= latest,
                    "case {case:?}: a restart re-arms what the clock still owed, so the next \
                     status is due in {restart_before}..={latest}, got {due}"
                );
            }
            None => assert_eq!(
                reported, ANOMALY_DETECTOR_STATUS_ANALYZING,
                "case {case:?}: a detector the clock owes nothing restarts in the status its \
                 lifecycle ended in, got {reported:?}"
            ),
        }
    }
}

/// An update of a detector the account does not hold is refused as a missing resource, and
/// creates nothing: the ARN is resolved against what the store holds, so an ARN nothing
/// minted, another Region's or another account's detector, an ARN naming another kind of
/// resource and a detector a delete already removed all name a resource that is not there.
#[tokio::test]
async fn an_update_of_a_detector_the_account_does_not_hold_is_refused() {
    let logs = Logs::new();
    let group = watched_group(&logs).await;
    let arn = created_detector(&logs, &group).await;
    let id = arn
        .rsplit(':')
        .next()
        .expect("a detector ARN ends in its id")
        .to_string();

    let deleted = created_detector(&logs, &group).await;
    logs.ok(
        "DeleteLogAnomalyDetector",
        &json!({"anomalyDetectorArn": deleted}),
    )
    .await;

    let cases: Vec<(&str, String)> = vec![
        (
            "an id no create minted",
            anomaly_detector_arn(REGION, "ffffffff-ffff-ffff-ffff-ffffffffffff"),
        ),
        (
            "this detector's id in another Region",
            anomaly_detector_arn("us-east-1", &id),
        ),
        (
            "this detector's id in another account",
            format!("arn:aws:logs:{REGION}:123456789012:anomaly-detector:{id}"),
        ),
        ("the watched log group's ARN", group.clone()),
        ("a detector a delete already removed", deleted.clone()),
    ];

    for (case, detector_arn) in cases {
        let (code, message) = logs
            .refusal(
                UPDATE,
                &json!({"anomalyDetectorArn": detector_arn, "enabled": true}),
            )
            .await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?}: expected {NOT_FOUND}, got {code} {message:?}"
        );
        assert_eq!(
            message,
            format!("The specified ARN does not exist or already deleted : {detector_arn}"),
            "case {case:?}: the refusal names the ARN that was not found, got {message:?}"
        );
    }

    // A refused update created nothing and left the one detector the account holds as it
    // was created.
    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let stored: Vec<String> = state.lock().anomaly_detectors.keys().cloned().collect();
    assert_eq!(
        stored,
        vec![arn.clone()],
        "a refused update creates no detector, expected only {arn:?}, got {stored:?}"
    );
    let reported = logs.ok(GET, &json!({"anomalyDetectorArn": arn})).await;
    assert_eq!(
        member(&reported, "evaluationFrequency"),
        Some(CREATED_FREQUENCY),
        "a refused update changes no other detector, got {reported}"
    );
}

/// A value the model's constraints or the filter-pattern syntax does not admit is refused
/// with the `InvalidParameterException` the operation models, and the detector keeps every
/// value it held.
#[tokio::test]
async fn a_value_the_model_or_the_pattern_syntax_rejects_is_refused() {
    let logs = Logs::new();
    let group = watched_group(&logs).await;
    let arn = created_detector(&logs, &group).await;

    // (case, the member and value, the refusal message when the service builds one)
    let cases: Vec<(&str, Value, Option<String>)> = vec![
        (
            "an anomaly visibility time one below the minimum of 7",
            json!({"anomalyVisibilityTime": 6}),
            Some(
                "1 validation error detected: Value at 'anomalyVisibilityTime' failed to \
                 satisfy constraint: Member must have value greater than or equal to 7"
                    .to_string(),
            ),
        ),
        (
            "an anomaly visibility time one above the maximum of 90",
            json!({"anomalyVisibilityTime": 91}),
            Some(
                "1 validation error detected: Value at 'anomalyVisibilityTime' failed to \
                 satisfy constraint: Member must have value less than or equal to 90"
                    .to_string(),
            ),
        ),
        (
            "an evaluation frequency outside the modelled enum",
            json!({"evaluationFrequency": "FIVE_MINUTES"}),
            None,
        ),
        (
            "a filter pattern that opens a bracketed form and never closes it",
            json!({"filterPattern": "{ $.level = \"ERROR\""}),
            Some("If a filter pattern starts with '{' it must end with '}'".to_string()),
        ),
        (
            "a filter pattern whose brackets do not balance",
            json!({"filterPattern": "[ip, [user, timestamp]"}),
            Some("Invalid filter pattern".to_string()),
        ),
    ];

    for (case, extra, expected_message) in cases {
        let mut body = Map::new();
        body.insert("anomalyDetectorArn".to_string(), json!(arn));
        body.insert("enabled".to_string(), json!(false));
        for (key, value) in extra.as_object().expect("each case names members") {
            body.insert(key.clone(), value.clone());
        }
        let (code, message) = logs.refusal(UPDATE, &Value::Object(body)).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {code} {message:?}"
        );
        if let Some(expected) = expected_message {
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }

        // A refused update writes nothing: neither the member it carried, nor the pause its
        // `enabled` asked for.
        let reported = logs.ok(GET, &json!({"anomalyDetectorArn": arn})).await;
        assert_eq!(
            member(&reported, "evaluationFrequency"),
            Some(CREATED_FREQUENCY),
            "case {case:?}: a refused update keeps the stored evaluationFrequency, got \
             {reported}"
        );
        assert_eq!(
            member(&reported, "filterPattern"),
            Some(CREATED_PATTERN),
            "case {case:?}: a refused update keeps the stored filterPattern, got {reported}"
        );
        assert_eq!(
            reported.get("anomalyVisibilityTime").and_then(Value::as_i64),
            Some(CREATED_VISIBILITY),
            "case {case:?}: a refused update keeps the stored anomalyVisibilityTime, got \
             {reported}"
        );
        assert_ne!(
            status(&reported, case),
            ANOMALY_DETECTOR_STATUS_PAUSED,
            "case {case:?}: a refused update does not pause the detector, got {reported}"
        );
    }
}
