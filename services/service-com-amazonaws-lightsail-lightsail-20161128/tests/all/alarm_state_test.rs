// SPDX-License-Identifier: CC0-1.0

//! `TestAlarm` — the Lightsail alarm driven into the state a test names, over
//! the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `TestAlarm` puts the ONE stored alarm it names into the
//! `AlarmState` the request carries — whichever state that is, however the alarm
//! is configured, and however many times in a row — leaving every other member
//! of that alarm, and every other alarm, exactly as it was, and appending one
//! terminal operation-log entry in the tested alarm's own region. A name no
//! alarm is stored under is refused with the service's `DoesNotExist` envelope
//! and changes nothing, and a state outside the modelled enum is refused by the
//! constraint walk before the handler runs.

use std::collections::BTreeSet;

use serde_json::{json, Value};

// The harness, the wire error code, the minted-id check and the alarm fixtures
// this test target already exposes: a sibling suite drives the service through
// the same front door rather than standing up a second one.
use crate::alarm_configuration::{alarm, alarms, monitored_resource, names, Monitored};
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// Every member of the modelled `AlarmState` enum
/// (`smithy_query(query="shape",shape="AlarmState")`) — the whole set of states
/// `TestAlarmRequest$state` accepts, and so the whole set this suite drives an
/// alarm through.
const ALARM_STATES: &[&str] = &["OK", "ALARM", "INSUFFICIENT_DATA"];

/// The state a new alarm is in ("When this action creates an alarm, the alarm
/// state is immediately set to INSUFFICIENT_DATA", `PutAlarm`), and so the state
/// an alarm nobody tested still reports.
const INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// The `operationType` the log entry of a test carries, the modelled
/// `OperationType` enum's own `TestAlarm` member.
const TEST_ALARM: &str = "TestAlarm";

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One alarm, and the states a case drives it through in order.
///
/// The rows differ along everything the rule generalises over: which state is
/// tested (all three modelled members appear, as a first test and as a later
/// one), how MANY tests one alarm takes and in what order (one, three
/// distinct, the same state twice, and a test into the state the alarm is
/// already in), the kind of monitored resource, the region, the metric, whether
/// the tested state is among the alarm's `notificationTriggers`, whether
/// notifications are enabled, disabled or left to default, whether the alarm
/// names contact protocols at all, whether the Region actually holds a contact
/// method for them, and the form of the alarm name.
struct Row {
    case: &'static str,
    alarm_name: &'static str,
    monitored: Monitored,
    monitored_name: &'static str,
    /// The region the monitored resource's Availability Zone belongs to, which
    /// is the region the alarm lives in and the one its operation-log entry
    /// reports.
    region: &'static str,
    zone: &'static str,
    metric_name: &'static str,
    contact_protocols: &'static [&'static str],
    notification_triggers: &'static [&'static str],
    notification_enabled: Option<bool>,
    /// The `ContactProtocol` this Region holds a contact method for before the
    /// alarm is tested, when it holds one at all.
    contact_method: Option<&'static str>,
    /// The states this case tests, in the order it tests them.
    tested: &'static [&'static str],
}

/// The table.
const ROWS: &[Row] = &[
    Row {
        case: "instance CPU tested into ALARM, the trigger it names, with a contact method",
        alarm_name: "cpu-high",
        monitored: Monitored::Instance,
        monitored_name: "box",
        region: "us-east-1",
        zone: "us-east-1a",
        metric_name: "CPUUtilization",
        contact_protocols: &["Email"],
        notification_triggers: &["ALARM"],
        notification_enabled: Some(true),
        contact_method: Some("Email"),
        tested: &["ALARM"],
    },
    Row {
        case: "instance egress with no triggers walked through every state in turn",
        alarm_name: "net_out_288",
        monitored: Monitored::Instance,
        monitored_name: "web",
        region: "us-west-2",
        zone: "us-west-2b",
        metric_name: "NetworkOut",
        contact_protocols: &[],
        notification_triggers: &[],
        notification_enabled: None,
        contact_method: None,
        tested: &["OK", "ALARM", "INSUFFICIENT_DATA"],
    },
    Row {
        case: "database connections with notifications off, tested into a state no trigger names",
        alarm_name: "db-connections-42",
        monitored: Monitored::RelationalDatabase,
        monitored_name: "appdb",
        region: "us-east-2",
        zone: "us-east-2a",
        metric_name: "DatabaseConnections",
        contact_protocols: &["Email", "SMS"],
        notification_triggers: &["ALARM", "OK"],
        notification_enabled: Some(false),
        contact_method: Some("Email"),
        tested: &["OK", "OK"],
    },
    Row {
        case: "instance status check tested into the state it is already in",
        alarm_name: "status_check_system",
        monitored: Monitored::Instance,
        monitored_name: "batch",
        region: "ap-northeast-1",
        zone: "ap-northeast-1c",
        metric_name: "StatusCheckFailed_System",
        contact_protocols: &["Email"],
        notification_triggers: &["ALARM", "INSUFFICIENT_DATA"],
        notification_enabled: None,
        contact_method: None,
        tested: &["INSUFFICIENT_DATA", "ALARM"],
    },
    Row {
        case: "database CPU in another region, SMS protocol the region holds no contact method for",
        alarm_name: "dbCpu2",
        monitored: Monitored::RelationalDatabase,
        monitored_name: "reports-db",
        region: "eu-central-1",
        zone: "eu-central-1a",
        metric_name: "CPUUtilization",
        contact_protocols: &["SMS"],
        notification_triggers: &["ALARM"],
        notification_enabled: Some(true),
        contact_method: None,
        tested: &["ALARM", "OK"],
    },
];

/// The `PutAlarm` body `row` describes.
fn put_body(row: &Row) -> Value {
    let mut body = json!({
        "alarmName": row.alarm_name,
        "monitoredResourceName": row.monitored_name,
        "metricName": row.metric_name,
        "comparisonOperator": "GreaterThanThreshold",
        "threshold": 70.5,
        "evaluationPeriods": 2,
    });
    if !row.contact_protocols.is_empty() {
        body["contactProtocols"] = json!(row.contact_protocols);
    }
    if !row.notification_triggers.is_empty() {
        body["notificationTriggers"] = json!(row.notification_triggers);
    }
    if let Some(enabled) = row.notification_enabled {
        body["notificationEnabled"] = json!(enabled);
    }
    body
}

/// `found` without its `state` member, so two reads of one alarm can be
/// compared on everything a test must NOT change.
fn without_state(found: &Value, case: &str) -> Value {
    let mut copy = found.clone();
    copy.as_object_mut()
        .unwrap_or_else(|| panic!("case {case}: expected an alarm object, got {found:?}"))
        .remove("state")
        .unwrap_or_else(|| panic!("case {case}: expected the alarm to report a state, got {found:?}"));
    copy
}

/// The one operation entry `body` answers, checked to be the terminal
/// `TestAlarm` entry of `alarm_name` in `region`, answering its id.
fn tested_operation(body: &Value, alarm_name: &str, region: &str, case: &str) -> String {
    let operations = body["operations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {body:?}"));
    assert_eq!(
        operations.len(),
        1,
        "case {case}: expected exactly one operation, got {body:?}"
    );
    let operation = &operations[0];
    for (member, want) in [
        ("operationType", TEST_ALARM),
        ("resourceType", "Alarm"),
        ("resourceName", alarm_name),
        ("status", "Succeeded"),
    ] {
        assert_eq!(
            operation[member].as_str(),
            Some(want),
            "case {case}: expected {member} {want}, got {operation:?}"
        );
    }
    assert_eq!(
        operation["isTerminal"].as_bool(),
        Some(true),
        "case {case}: the alarm is in the tested state when the call answers, got {operation:?}"
    );
    assert_eq!(
        operation["location"]["regionName"].as_str(),
        Some(region),
        "case {case}: expected the alarm's own region, got {operation:?}"
    );
    let id = operation["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected the operation to carry an id, got {operation:?}"));
    assert!(
        is_uuid_shaped(id),
        "case {case}: expected a minted uuid-shaped operation id, got {id}"
    );
    id.to_string()
}

/// Every operation-log entry `GetOperationsForResource` reports for `name`,
/// walking every page.
async fn log_entries(emulator: &Emulator, name: &str, case: &str) -> Vec<Value> {
    let mut entries: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({"resourceName": name});
        if let Some(token) = &token {
            body["pageToken"] = json!(token);
        }
        let answered = emulator.ok("GetOperationsForResource", body).await;
        let page = answered["operations"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case}: expected an operations array for {name}, got {answered:?}")
            })
            .clone();
        entries.extend(page);
        match answered["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
        assert!(
            entries.len() < 1_000,
            "case {case}: the walk over {name} did not terminate"
        );
    }
    entries
}

// ---------------------------------------------------------------------------
// The tested state
// ---------------------------------------------------------------------------

/// RULE: `TestAlarm` puts the alarm it names into the `AlarmState` the request
/// carries, leaves every other member of that alarm as it was, and appends one
/// terminal operation-log entry per test — so the state a later `GetAlarms`
/// reports is the last state tested, whatever the alarm's notification
/// configuration is.
#[tokio::test]
async fn testing_an_alarm_puts_it_into_the_state_the_request_names() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        monitored_resource(
            &emulator,
            row.monitored,
            row.monitored_name,
            row.zone,
            case,
        )
        .await;
        if let Some(protocol) = row.contact_method {
            emulator
                .ok_at(
                    None,
                    Some(row.region),
                    "CreateContactMethod",
                    json!({"protocol": protocol, "contactEndpoint": "ops@example.com"}),
                )
                .await;
        }
        emulator.ok("PutAlarm", put_body(row)).await;

        let created = alarm(&emulator, row.alarm_name, case).await;
        assert_eq!(
            created["state"].as_str(),
            Some(INSUFFICIENT_DATA),
            "case {case}: expected a new alarm to start in {INSUFFICIENT_DATA}, got {created:?}"
        );
        let configuration = without_state(&created, case);

        let mut operation_ids: BTreeSet<String> = BTreeSet::new();
        for (index, state) in row.tested.iter().enumerate() {
            let answered = emulator
                .ok(
                    "TestAlarm",
                    json!({"alarmName": row.alarm_name, "state": state}),
                )
                .await;
            let case = format!("{case} (test {index} into {state})");
            let id = tested_operation(&answered, row.alarm_name, row.region, &case);
            assert!(
                operation_ids.insert(id.clone()),
                "case {case}: expected a fresh operation id per test, saw {id} twice"
            );

            let after = alarm(&emulator, row.alarm_name, &case).await;
            assert_eq!(
                after["state"].as_str(),
                Some(*state),
                "case {case}: expected the tested state, got {after:?}"
            );
            assert_eq!(
                without_state(&after, &case),
                configuration,
                "case {case}: expected a test to change nothing but the state, got {after:?}"
            );
        }

        // The log holds the put and one entry per test, and nothing else.
        let entries = log_entries(&emulator, row.alarm_name, case).await;
        let logged: Vec<&str> = entries
            .iter()
            .map(|entry| entry["operationType"].as_str().unwrap_or("<none>"))
            .collect();
        let mut want = vec!["PutAlarm"];
        want.extend(row.tested.iter().map(|_| TEST_ALARM));
        assert_eq!(
            logged, want,
            "case {case}: expected the put and one entry per test, got {entries:?}"
        );
    }
}

/// RULE: a test names ONE alarm, so no other stored alarm moves — whether it
/// watches the same resource or a different one.
#[tokio::test]
async fn testing_one_alarm_leaves_every_other_alarm_alone() {
    let case = "two alarms on an instance and one on a database";
    let emulator = Emulator::new();
    monitored_resource(&emulator, Monitored::Instance, "box", "eu-west-1a", case).await;
    monitored_resource(
        &emulator,
        Monitored::RelationalDatabase,
        "appdb",
        "eu-west-1a",
        case,
    )
    .await;
    for (name, monitored, metric) in [
        ("cpu", "box", "CPUUtilization"),
        ("net", "box", "NetworkIn"),
        ("conns", "appdb", "DatabaseConnections"),
    ] {
        emulator
            .ok(
                "PutAlarm",
                json!({
                    "alarmName": name,
                    "monitoredResourceName": monitored,
                    "metricName": metric,
                    "comparisonOperator": "LessThanThreshold",
                    "threshold": 1.0,
                    "evaluationPeriods": 1,
                }),
            )
            .await;
    }

    // Each step tests one alarm; every other alarm keeps the state it had.
    let mut expected = [
        ("conns", INSUFFICIENT_DATA.to_string()),
        ("cpu", INSUFFICIENT_DATA.to_string()),
        ("net", INSUFFICIENT_DATA.to_string()),
    ];
    for (tested, state) in [("cpu", "ALARM"), ("conns", "OK"), ("cpu", "OK")] {
        emulator
            .ok("TestAlarm", json!({"alarmName": tested, "state": state}))
            .await;
        for entry in expected.iter_mut() {
            if entry.0 == tested {
                entry.1 = state.to_string();
            }
        }
        let (found, _) = alarms(&emulator, json!({})).await;
        assert_eq!(
            names(&found),
            vec!["conns".to_string(), "cpu".to_string(), "net".to_string()],
            "case {case}: expected a test to add or remove no alarm, got {found:?}"
        );
        let states: Vec<(&str, String)> = found
            .iter()
            .map(|found| {
                (
                    found["name"].as_str().unwrap_or_default(),
                    found["state"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let want: Vec<(&str, String)> = expected
            .iter()
            .map(|(name, state)| (*name, state.clone()))
            .collect();
        assert_eq!(
            states, want,
            "case {case}: after testing {tested} into {state}, expected {want:?}, got {states:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: a request naming no stored alarm is refused with the service's
/// `DoesNotExist` envelope and changes nothing — an alarm is addressed by its
/// own name, so neither a name nothing was ever stored under, nor a deleted
/// alarm's name, nor the name of the resource the alarm watches, names one.
#[tokio::test]
async fn testing_an_alarm_that_is_not_stored_is_refused_and_changes_nothing() {
    let case = "one alarm, and four names that are not it";
    let emulator = Emulator::new();
    monitored_resource(&emulator, Monitored::Instance, "box", "us-east-1a", case).await;
    for name in ["cpu", "gone"] {
        emulator
            .ok(
                "PutAlarm",
                json!({
                    "alarmName": name,
                    "monitoredResourceName": "box",
                    "metricName": if name == "cpu" { "CPUUtilization" } else { "NetworkIn" },
                    "comparisonOperator": "GreaterThanThreshold",
                    "threshold": 5.0,
                    "evaluationPeriods": 1,
                }),
            )
            .await;
    }
    emulator.ok("DeleteAlarm", json!({"alarmName": "gone"})).await;

    for (name, state) in [
        // Never stored.
        ("never-existed", "ALARM"),
        // Stored, then deleted.
        ("gone", "OK"),
        // The monitored resource's own name is not the alarm's.
        ("box", "INSUFFICIENT_DATA"),
        // A name that differs from a stored alarm only in case.
        ("CPU", "ALARM"),
    ] {
        let (status, body) = emulator
            .call("TestAlarm", json!({"alarmName": name, "state": state}))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected testing {name} to be refused, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case}: expected NotFoundException for {name}, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected the DoesNotExist envelope for {name}, got {body:?}"
        );
    }

    let stored = alarm(&emulator, "cpu", case).await;
    assert_eq!(
        stored["state"].as_str(),
        Some(INSUFFICIENT_DATA),
        "case {case}: expected the refusals to have moved no alarm, got {stored:?}"
    );
    let entries = log_entries(&emulator, "cpu", case).await;
    let logged: Vec<&str> = entries
        .iter()
        .map(|entry| entry["operationType"].as_str().unwrap_or("<none>"))
        .collect();
    assert_eq!(
        logged,
        vec!["PutAlarm"],
        "case {case}: expected a refused test to append no log entry, got {logged:?}"
    );
}

/// RULE: `state` targets the modelled `AlarmState` enum and is `@required`, so a
/// value outside that enum — or none at all — is refused by the constraint walk
/// with its own 400 `ValidationException`, before the handler runs, and moves no
/// alarm.
#[tokio::test]
async fn a_state_outside_the_modelled_enum_is_refused_before_the_handler() {
    let case = "states the model does not declare";
    let emulator = Emulator::new();
    monitored_resource(&emulator, Monitored::Instance, "box", "us-east-1a", case).await;
    emulator
        .ok(
            "PutAlarm",
            json!({
                "alarmName": "cpu",
                "monitoredResourceName": "box",
                "metricName": "CPUUtilization",
                "comparisonOperator": "GreaterThanThreshold",
                "threshold": 5.0,
                "evaluationPeriods": 1,
            }),
        )
        .await;

    for body in [
        // A state of a different service's alarm vocabulary.
        json!({"alarmName": "cpu", "state": "PENDING"}),
        // The right member in the wrong case.
        json!({"alarmName": "cpu", "state": "ok"}),
        // Empty.
        json!({"alarmName": "cpu", "state": ""}),
        // The `@required` member omitted altogether.
        json!({"alarmName": "cpu"}),
    ] {
        let (status, answered) = emulator.call("TestAlarm", body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case}: expected {body} to be refused, got {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case}: expected the boundary walk's ValidationException for {body}, got \
             {answered:?}"
        );
    }

    // Every modelled member, by contrast, is served.
    for state in ALARM_STATES {
        emulator
            .ok("TestAlarm", json!({"alarmName": "cpu", "state": state}))
            .await;
        let stored = alarm(&emulator, "cpu", case).await;
        assert_eq!(
            stored["state"].as_str(),
            Some(*state),
            "case {case}: expected {state} to be served, got {stored:?}"
        );
    }
}
