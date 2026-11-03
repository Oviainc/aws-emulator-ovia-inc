// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteLogAnomalyDetector`, the operation that removes a log
//! anomaly detector.
//!
//! The rule under test: a delete removes exactly the ONE anomaly detector the account holds
//! under the ARN the request names — whatever configuration it carries, whatever status the
//! clock has it in, and whatever it has reported — so the detector stops being reported by
//! every reader of it, the findings it had reported and the suppressions placed on them go
//! with it, and it stops counting against the account's detector ceiling; every other
//! detector the account holds is left exactly as it was. An ARN no stored detector answers
//! under — an id nothing minted, another Region's or another account's detector, another
//! resource kind, a value that is not an ARN at all, or a detector this account already
//! deleted — is a missing resource and removes nothing.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The tables here span what the rule generalises over rather than one request: the seeded
//! detectors differ in which optional members the create named (none, each, all of them),
//! in `anomalyVisibilityTime` at both published bounds of its `@range(7..90)`, in the log
//! group they watch, in the status the clock has them in (one still initializing, one that
//! has reached ANALYZING), and in how many findings they have reported (none, one, several)
//! and whether one of those findings is suppressed; the deletes then run in an order that is
//! neither the creation order nor the ARN order, so the detector removed is a middle, a
//! first and a last one of the remaining enumeration in turn.

use serde_json::{json, Map, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    anomaly_detector_arn, anomaly_detector_not_found_message, log_group_arn,
    ComAmazonawsCloudwatchlogsLogs20140328State, AnomalyDetectorRecord,
    ANOMALY_DETECTOR_STATUS_ANALYZING, MAX_ANOMALY_DETECTORS, SUPPRESSION_TYPE_INFINITE,
};

use crate::harness::{now_ms, report_anomaly, Logs, REGION};

/// The operation under test, the one that puts a detector in the store, and the readers a
/// removal must take the detector out of.
const DELETE: &str = "DeleteLogAnomalyDetector";
const CREATE: &str = "CreateLogAnomalyDetector";
const GET: &str = "GetLogAnomalyDetector";
const LIST_ANOMALIES: &str = "ListAnomalies";

/// The code answered when the resource a request names is not there.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The code answered when a create would grow the account's store past its ceiling.
const LIMIT_EXCEEDED: &str = "LimitExceededException";

/// The two log groups the seeded detectors watch, so the corpus spans more than one watched
/// group and a delete can be held to leaving the group it watched alone.
const GROUPS: [&str; 2] = [
    "anomaly-detector-removal/orders",
    "anomaly-detector-removal/payments",
];

/// A KMS key ARN of the form `DetectorKmsKeyArn`'s `@pattern` admits, for the rows that
/// assign one.
const KMS_KEY: &str = "arn:aws:kms:eu-west-2:000000000000:key/aaaabbbb-cccc-dddd-eeee-ffff00001111";

/// How far back an "aged" detector is created, so the clock has already moved it through
/// TRAINING to ANALYZING by the time anything reads it.
const AN_HOUR_MS: i64 = 60 * 60 * 1_000;

/// One seeded detector: the configuration its create names, the group it watches, the
/// findings it has reported, and whether the clock has had time to finish training it.
struct Seed {
    /// What this row varies, for the assertion messages.
    case: &'static str,
    group: &'static str,
    detector_name: Option<&'static str>,
    evaluation_frequency: Option<&'static str>,
    filter_pattern: Option<&'static str>,
    kms_key_id: Option<&'static str>,
    anomaly_visibility_time: Option<i64>,
    tags: Option<&'static [(&'static str, &'static str)]>,
    /// How many anomalies this detector has reported.
    findings: usize,
    /// Whether the first of those findings carries a suppression.
    suppressed: bool,
    /// Whether the detector is backdated, so a read reports it ANALYZING rather than in a
    /// status the clock still owes a change after.
    aged: bool,
}

impl Seed {
    /// A row naming nothing but the required log group list, fresh and with no findings.
    const fn bare(case: &'static str, group: &'static str) -> Self {
        Self {
            case,
            group,
            detector_name: None,
            evaluation_frequency: None,
            filter_pattern: None,
            kms_key_id: None,
            anomaly_visibility_time: None,
            tags: None,
            findings: 0,
            suppressed: false,
            aged: false,
        }
    }
}

/// The seeded corpus, in the order the detectors are CREATED in.
fn seeds() -> Vec<Seed> {
    vec![
        Seed::bare("nothing but the log group it watches", GROUPS[0]),
        Seed {
            detector_name: Some("fully-configured"),
            evaluation_frequency: Some("FIFTEEN_MIN"),
            filter_pattern: Some("{ $.level = \"ERROR\" }"),
            kms_key_id: Some(KMS_KEY),
            anomaly_visibility_time: Some(30),
            tags: Some(&[("team", "observability")]),
            findings: 2,
            suppressed: true,
            aged: true,
            ..Seed::bare("every optional member, two findings, one suppressed", GROUPS[0])
        },
        Seed {
            filter_pattern: Some(""),
            tags: Some(&[("env", "test")]),
            findings: 1,
            ..Seed::bare(
                "the empty filter pattern, which FilterPattern's length 0.. admits",
                GROUPS[1],
            )
        },
        Seed {
            evaluation_frequency: Some("ONE_MIN"),
            kms_key_id: Some(KMS_KEY),
            aged: true,
            ..Seed::bare("a KMS key and the shortest evaluation frequency", GROUPS[1])
        },
        Seed {
            // `AnomalyVisibilityTime` is `@range(min: 7, max: 90)`: this row sits on the
            // published minimum, the one below on the maximum.
            anomaly_visibility_time: Some(7),
            findings: 3,
            suppressed: true,
            ..Seed::bare("the minimum anomaly visibility time and three findings", GROUPS[0])
        },
        Seed {
            detector_name: Some("long-visibility"),
            anomaly_visibility_time: Some(90),
            findings: 1,
            aged: true,
            ..Seed::bare("the maximum anomaly visibility time", GROUPS[1])
        },
    ]
}

/// The order the seeds are deleted in: neither the creation order nor the ascending ARN
/// order the store holds them in, so the detector removed is a middle, a first and a last
/// one of what remains in turn.
const DELETE_ORDER: [usize; 6] = [2, 0, 5, 1, 4, 3];

/// The create body for one seed: the group it watches plus only the members it names, so an
/// absent member is absent on the wire too.
fn create_request(seed: &Seed) -> Value {
    let mut request = Map::new();
    request.insert(
        "logGroupArnList".to_string(),
        json!([log_group_arn(REGION, seed.group, false)]),
    );
    if let Some(name) = seed.detector_name {
        request.insert("detectorName".to_string(), json!(name));
    }
    if let Some(frequency) = seed.evaluation_frequency {
        request.insert("evaluationFrequency".to_string(), json!(frequency));
    }
    if let Some(pattern) = seed.filter_pattern {
        request.insert("filterPattern".to_string(), json!(pattern));
    }
    if let Some(key) = seed.kms_key_id {
        request.insert("kmsKeyId".to_string(), json!(key));
    }
    if let Some(days) = seed.anomaly_visibility_time {
        request.insert("anomalyVisibilityTime".to_string(), json!(days));
    }
    if let Some(tags) = seed.tags {
        let mut map = Map::new();
        for (key, value) in tags {
            map.insert((*key).to_string(), json!(value));
        }
        request.insert("tags".to_string(), Value::Object(map));
    }
    Value::Object(request)
}

/// One string member of a response, or `None` when the response omits it.
fn member(body: &Value, key: &str) -> Option<String> {
    body.get(key).and_then(Value::as_str).map(str::to_string)
}

/// An `anomalyId` of the length the model fixes it at (`@length(36..36)`), carrying which
/// detector reported it so one enumeration row is told from another.
fn anomaly_id(detector: usize, finding: usize) -> String {
    padded(&format!("anomaly-{detector}-{finding}-"), 36)
}

/// A `patternId` of the length the model fixes it at (`@length(32..32)`).
fn pattern_id(detector: usize, finding: usize) -> String {
    padded(&format!("pattern-{detector}-{finding}-"), 32)
}

/// `seed` padded out to `len` characters, so a seeded id has the length the model fixes.
fn padded(seed: &str, len: usize) -> String {
    let mut id = seed.to_string();
    while id.len() < len {
        id.push('0');
    }
    id
}

/// Record, in the store, that the detector under `arn` has REPORTED `findings` anomalies —
/// what a detection pass over its log group's events writes, and the set `ListAnomalies`
/// reports back — and suppress the first of them when `suppressed`.
///
/// No request reports an anomaly ("The anomaly detector uses pattern recognition to find
/// anomalies"), so these are seeded on the stored record; everything the cases then assert
/// about them goes over the wire like any client's call.
fn report_findings(logs: &Logs, index: usize, arn: &str, findings: usize, suppressed: bool) {
    for finding in 0..findings {
        report_anomaly(
            logs,
            arn,
            &anomaly_id(index, finding),
            &pattern_id(index, finding),
        );
    }
    if suppressed && findings > 0 {
        let state = logs
            .engine()
            .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
        let mut store = state.lock();
        let detector = store
            .anomaly_detectors
            .get_mut(arn)
            .unwrap_or_else(|| panic!("the detector under {arn} is stored"));
        detector.suppress_finding(
            &anomaly_id(index, 0),
            Some(SUPPRESSION_TYPE_INFINITE),
            None,
            false,
            now_ms(),
        );
    }
}

/// Move the detector under `arn` back in time by `by_ms`, so the status changes the clock
/// owed it are already due and a read reports it ANALYZING. Only the clock can do this, and
/// a test cannot wait an hour, so the instants the create stored are shifted instead.
fn backdate(logs: &Logs, arn: &str, by_ms: i64) {
    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let mut store = state.lock();
    let detector = store
        .anomaly_detectors
        .get_mut(arn)
        .unwrap_or_else(|| panic!("the detector under {arn} is stored"));
    detector.creation_time_ms -= by_ms;
    detector.last_modified_time_ms -= by_ms;
    detector.status_due_ms = detector.status_due_ms.map(|due| due - by_ms);
}

/// The ARNs the store holds a detector under, in the order it holds them.
fn stored_arns(logs: &Logs) -> Vec<String> {
    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let arns = state.lock().anomaly_detectors.keys().cloned().collect();
    arns
}

/// The `anomalyId`s `ListAnomalies` reports for one request, in the order it listed them.
/// The seeded corpus is far inside one page, so a single call sees all of it.
async fn listed_anomaly_ids(logs: &Logs, request: &Value) -> Vec<String> {
    let page = logs.ok(LIST_ANOMALIES, request).await;
    assert!(
        page.get("nextToken").is_none(),
        "the seeded findings fit in one page, got {page}"
    );
    page.get("anomalies")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{LIST_ANOMALIES} must return anomalies, got {page}"))
        .iter()
        .map(|anomaly| {
            member(anomaly, "anomalyId")
                .unwrap_or_else(|| panic!("a reported anomaly carries its id, got {anomaly}"))
        })
        .collect()
}

/// Create the log groups the seeded detectors watch, then create one detector per seed,
/// seeding the findings it has reported and backdating the aged ones. Answers each seed's
/// ARN, in seed order.
async fn seeded_detectors(logs: &Logs, seeds: &[Seed]) -> Vec<String> {
    for group in GROUPS {
        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
    }
    let mut arns = Vec::new();
    for (index, seed) in seeds.iter().enumerate() {
        let created = logs.ok(CREATE, &create_request(seed)).await;
        let arn = member(&created, "anomalyDetectorArn").unwrap_or_else(|| {
            panic!(
                "case {:?}: a create answers anomalyDetectorArn, got {created}",
                seed.case
            )
        });
        report_findings(logs, index, &arn, seed.findings, seed.suppressed);
        if seed.aged {
            backdate(logs, &arn, AN_HOUR_MS);
        }
        arns.push(arn);
    }
    arns
}

/// Every seeded detector, deleted one at a time: the delete answers an empty Unit output,
/// the deleted detector is gone from the read of one detector and from the anomaly
/// enumeration, a second delete of the same ARN is refused, and every surviving detector
/// still reports exactly what its create stored, findings and all.
#[tokio::test]
async fn each_deleted_detector_is_gone_from_every_read() {
    let logs = Logs::new();
    let seeds = seeds();
    let arns = seeded_detectors(&logs, &seeds).await;

    // What the store and the anomaly enumeration hold while nothing has been deleted yet:
    // the baseline each removal is measured against.
    let mut remaining: Vec<usize> = (0..seeds.len()).collect();
    let expected_ids = |remaining: &[usize]| -> Vec<String> {
        let mut ids: Vec<String> = remaining
            .iter()
            .flat_map(|index| (0..seeds[*index].findings).map(move |finding| (*index, finding)))
            .map(|(index, finding)| anomaly_id(index, finding))
            .collect();
        ids.sort();
        ids
    };
    let mut listed = listed_anomaly_ids(&logs, &json!({})).await;
    listed.sort();
    assert_eq!(
        listed,
        expected_ids(&remaining),
        "case \"the seeded corpus\": every reported finding is listed before any detector is \
         deleted"
    );

    for index in DELETE_ORDER {
        let seed = &seeds[index];
        let case = seed.case;
        let arn = &arns[index];

        // An aged detector has reached its terminal status and a fresh one has not, so the
        // rows span both: either way the delete removes it.
        let before = logs.ok(GET, &json!({"anomalyDetectorArn": arn})).await;
        if seed.aged {
            assert_eq!(
                member(&before, "anomalyDetectorStatus").as_deref(),
                Some(ANOMALY_DETECTOR_STATUS_ANALYZING),
                "case {case:?}: a detector created an hour ago has finished training, got \
                 {before}"
            );
        }

        let body = logs.ok(DELETE, &json!({"anomalyDetectorArn": arn})).await;
        let members = match &body {
            Value::Null => 0,
            Value::Object(map) => map.len(),
            other => panic!("case {case:?}: a Unit output is an empty body, got {other}"),
        };
        assert_eq!(
            members, 0,
            "case {case:?}: a Unit output carries no members, got {body}"
        );

        // The detector is gone from the read of one detector, and a second delete of the
        // same ARN names a resource that is no longer there rather than succeeding again.
        for operation in [GET, DELETE] {
            let (code, message) = logs
                .refusal(operation, &json!({"anomalyDetectorArn": arn}))
                .await;
            assert_eq!(
                code, NOT_FOUND,
                "case {case:?}: {operation} on the deleted detector expected {NOT_FOUND}, got \
                 {code} {message:?}"
            );
            assert_eq!(
                message,
                anomaly_detector_not_found_message(arn),
                "case {case:?}: {operation} names the ARN that is not there, got {message:?}"
            );
        }

        // The findings the deleted detector had reported went with it, and only those: the
        // rest of the enumeration is untouched. Selecting the deleted detector by ARN names
        // a resource that is not there rather than selecting nothing.
        remaining.retain(|held| *held != index);
        let mut listed = listed_anomaly_ids(&logs, &json!({})).await;
        listed.sort();
        assert_eq!(
            listed,
            expected_ids(&remaining),
            "case {case:?}: only the deleted detector's findings are gone from the enumeration"
        );
        let (code, message) = logs
            .refusal(LIST_ANOMALIES, &json!({"anomalyDetectorArn": arn}))
            .await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?}: listing the deleted detector's anomalies expected {NOT_FOUND}, got \
             {code} {message:?}"
        );

        // Every surviving detector still reports its own configuration, so the delete removed
        // one record and touched no other.
        for other in &remaining {
            let seed = &seeds[*other];
            let read = logs
                .ok(GET, &json!({"anomalyDetectorArn": arns[*other]}))
                .await;
            assert_eq!(
                read.get("logGroupArnList"),
                Some(&json!([log_group_arn(REGION, seed.group, false)])),
                "case {case:?}: the surviving detector {:?} still watches its log group, got \
                 {read}",
                seed.case
            );
            for (key, named) in [
                ("detectorName", seed.detector_name),
                ("evaluationFrequency", seed.evaluation_frequency),
                ("filterPattern", seed.filter_pattern),
                ("kmsKeyId", seed.kms_key_id),
            ] {
                assert_eq!(
                    member(&read, key).as_deref(),
                    named,
                    "case {case:?}: the surviving detector {:?} keeps {key} {named:?}, got \
                     {read}",
                    seed.case
                );
            }
            assert_eq!(
                read.get("anomalyVisibilityTime").and_then(Value::as_i64),
                seed.anomaly_visibility_time,
                "case {case:?}: the surviving detector {:?} keeps anomalyVisibilityTime {:?}, \
                 got {read}",
                seed.case,
                seed.anomaly_visibility_time
            );
        }

        let held = stored_arns(&logs);
        let mut expected: Vec<String> = remaining.iter().map(|held| arns[*held].clone()).collect();
        expected.sort();
        assert_eq!(
            held, expected,
            "case {case:?}: the store holds exactly the detectors not yet deleted, expected \
             {expected:?}, got {held:?}"
        );
    }

    assert!(
        stored_arns(&logs).is_empty(),
        "case \"every detector deleted\": the account holds no anomaly detector, got {:?}",
        stored_arns(&logs)
    );
    assert!(
        listed_anomaly_ids(&logs, &json!({})).await.is_empty(),
        "case \"every detector deleted\": no finding is reported any more"
    );

    // A detector watches log groups it does not own: deleting it leaves every one of them.
    for group in GROUPS {
        assert!(
            logs.describe_group(group).await.is_some(),
            "case \"every detector deleted\": the watched log group {group:?} is untouched"
        );
    }
}

/// Deleting an ARN no detector is stored under is a missing resource, not a silent success,
/// and removes nothing — including an ARN this service's own builder produces for an id
/// nothing minted, the same detector in another Region or another account, another resource
/// kind, and a value that is not an ARN at all.
#[tokio::test]
async fn deleting_a_detector_the_account_does_not_hold_is_refused() {
    let logs = Logs::new();
    let seeds = vec![
        Seed {
            detector_name: Some("held"),
            findings: 2,
            suppressed: true,
            ..Seed::bare("the detector the account holds", GROUPS[0])
        },
        Seed {
            findings: 1,
            aged: true,
            ..Seed::bare("a second detector the account holds", GROUPS[1])
        },
    ];
    let arns = seeded_detectors(&logs, &seeds).await;
    let held = arns[0].clone();
    let prefix = anomaly_detector_arn(REGION, "");
    let id = held[prefix.len()..].to_string();

    // One detector deleted up front, so "a detector this account already deleted" is a real
    // row rather than an id nothing ever minted.
    let deleted = arns[1].clone();
    logs.ok(DELETE, &json!({"anomalyDetectorArn": deleted}))
        .await;

    // (case, the ARN deleted)
    let cases: Vec<(&str, String)> = vec![
        (
            "an anomaly-detector ARN for an id nothing minted",
            anomaly_detector_arn(REGION, "ffffffff-ffff-ffff-ffff-ffffffffffff"),
        ),
        (
            "this detector's id in ANOTHER Region, which this account does not hold here",
            anomaly_detector_arn("us-east-1", &id),
        ),
        (
            "this detector's id in ANOTHER account",
            format!("arn:aws:logs:{REGION}:111122223333:anomaly-detector:{id}"),
        ),
        (
            "a well-formed ARN of another resource kind",
            log_group_arn(REGION, GROUPS[0], false),
        ),
        (
            "an ARN of another service entirely",
            "arn:aws:s3:::an-anomaly-bucket".to_string(),
        ),
        ("a value that is not an ARN at all", "detector-1".to_string()),
        ("a detector this account already deleted", deleted.clone()),
    ];

    for (case, arn) in cases {
        let (code, message) = logs
            .refusal(DELETE, &json!({"anomalyDetectorArn": arn}))
            .await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?}: expected {NOT_FOUND}, got {code} {message:?}"
        );
        assert_eq!(
            message,
            anomaly_detector_not_found_message(&arn),
            "case {case:?}: expected the refusal to name the ARN that is not there, got {message:?}"
        );
    }

    // The detector the account DOES hold is untouched, findings and all: no refused delete
    // removed anything.
    let read = logs.ok(GET, &json!({"anomalyDetectorArn": held})).await;
    assert_eq!(
        member(&read, "detectorName").as_deref(),
        Some("held"),
        "a refused delete removes nothing, got {read}"
    );
    let mut listed = listed_anomaly_ids(&logs, &json!({})).await;
    listed.sort();
    assert_eq!(
        listed,
        vec![anomaly_id(0, 0), anomaly_id(0, 1)],
        "a refused delete leaves the held detector's findings as they were"
    );
    assert_eq!(
        stored_arns(&logs),
        vec![held],
        "a refused delete leaves the account's detectors as they were"
    );
}

/// A deleted detector stops counting against the account's detector ceiling: a create
/// refused at the ceiling is accepted once one detector is deleted, and refused again once
/// the store is back at the ceiling.
#[tokio::test]
async fn a_deleted_detector_frees_its_slot_against_the_account_ceiling() {
    let logs = Logs::new();
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUPS[0]}))
        .await;
    let group = log_group_arn(REGION, GROUPS[0], false);
    let create = json!({"logGroupArnList": [group]});

    // The store filled to exactly its ceiling, through the same record constructor the create
    // uses: the rule is about the COUNT the store holds, so how each detector got there does
    // not matter.
    let mut filler = Vec::new();
    {
        let state = logs
            .engine()
            .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
        let mut store = state.lock();
        for index in 0..MAX_ANOMALY_DETECTORS {
            let arn = anomaly_detector_arn(REGION, &format!("ceiling-{index}"));
            let record = AnomalyDetectorRecord::new(&arn, vec![group.clone()], now_ms());
            store.anomaly_detectors.insert(arn.clone(), record);
            filler.push(arn);
        }
    }

    let code = logs.err(CREATE, &create).await;
    assert_eq!(
        code, LIMIT_EXCEEDED,
        "case \"a create at the ceiling\": expected {LIMIT_EXCEEDED}, got {code}"
    );

    // One delete, one accepted create: the slot the deleted detector held is free.
    logs.ok(DELETE, &json!({"anomalyDetectorArn": filler[0]}))
        .await;
    let created = logs.ok(CREATE, &create).await;
    let minted = member(&created, "anomalyDetectorArn")
        .unwrap_or_else(|| panic!("case \"a create after a delete\": expected an ARN, got {created}"));
    assert_ne!(
        minted, filler[0],
        "case \"a create after a delete\": a create mints a fresh ARN rather than reusing the \
         deleted detector's"
    );
    let read = logs
        .ok(GET, &json!({"anomalyDetectorArn": minted}))
        .await;
    assert_eq!(
        read.get("logGroupArnList"),
        Some(&json!([group])),
        "case \"a create after a delete\": the new detector watches the group it named, got {read}"
    );

    // The store is back at the ceiling, so the next create is refused again.
    let code = logs.err(CREATE, &create).await;
    assert_eq!(
        code, LIMIT_EXCEEDED,
        "case \"a create back at the ceiling\": expected {LIMIT_EXCEEDED}, got {code}"
    );
    let held = stored_arns(&logs).len();
    assert_eq!(
        held, MAX_ANOMALY_DETECTORS,
        "case \"a create back at the ceiling\": expected {MAX_ANOMALY_DETECTORS} detectors, got \
         {held}"
    );
}
