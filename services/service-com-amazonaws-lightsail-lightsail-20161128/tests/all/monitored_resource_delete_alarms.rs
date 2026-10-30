// SPDX-License-Identifier: CC0-1.0

//! What deleting a monitored resource does to the alarms that watched it, driven
//! over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness and the alarm fixtures are the sibling suites' own
//! ([`crate::instance_lifecycle::Emulator`],
//! [`crate::alarm_configuration::monitored_resource`]), and every alarm a case
//! releases was made with `PutAlarm` — no case reaches into the store.
//!
//! The rule under test: removing a resource releases every live reference
//! another stored record holds to it, so no read reports a record referring to a
//! resource the account no longer holds. An alarm's reference is the resource it
//! monitors — `monitoredResourceName` is required on the put, which refuses a
//! name that resolves to no resource of this account, and every read reports it
//! as `Alarm$monitoredResourceInfo` — and that reference cannot be left empty,
//! so the alarm goes with the resource it watched: the alarm is gone from
//! `GetAlarms` and from `DeleteAlarm`, its removal is logged as its own
//! `DeleteAlarm` entry beside the delete that caused it, the per-metric alarm
//! budget it spent is free again, and an alarm watching any OTHER resource is
//! left exactly as it was.
//!
//! The cases are a TABLE whose rows differ along everything the rule generalises
//! over: the KIND of monitored resource (so both delete operations that can
//! remove one), the Region, how many alarms watched the removed resource (none,
//! one, both of the two a single metric accepts, several metrics at once), which
//! metrics those were, whether another monitored resource of the same kind is
//! being watched at the same time, the lifecycle state the resource is removed
//! from, and — for a database — whether the delete also takes a final snapshot.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::alarm_configuration::{alarms, monitored_resource, names, Monitored};
use crate::instance_lifecycle::{error_code, is_uuid_shaped, wait_for_state, Emulator};

/// The wire error a Lightsail request for a resource the account does not hold is
/// refused with, and the `code` its body carries (pinned for every operation in
/// `instance_error_envelope.rs`).
const NOT_FOUND: &str = "NotFoundException";
/// See [`NOT_FOUND`].
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The most alarms one metric of one monitored resource carries, from the
/// modelled `PutAlarmRequest$metricName` documentation: "You can configure up
/// to two alarms per metric."
const ALARMS_PER_METRIC: usize = 2;

/// One monitored resource whose removal must release the alarms watching it.
struct Row {
    case: &'static str,
    /// The kind of resource the row deletes, which decides the delete operation
    /// and the metrics its alarms watch.
    kind: Monitored,
    /// The Region the monitored resource's Availability Zone belongs to — the
    /// Region its alarms live in, and so the Region each release is logged in.
    region: &'static str,
    zone: &'static str,
    /// The resource the case deletes.
    deleted: &'static str,
    /// The alarms watching it, as `(alarm name, metric)`.
    watching: &'static [(&'static str, &'static str)],
    /// Another monitored resource of the same kind, when the row has one: its
    /// alarms must survive the delete untouched.
    survivor: Option<&'static str>,
    /// The alarms watching `survivor`.
    survivor_watching: &'static [(&'static str, &'static str)],
    /// Whether the clock is allowed to settle the resource into its ready state
    /// before the delete. A delete is accepted either way, so the release cannot
    /// depend on it.
    settle: bool,
    /// The name of the final snapshot a database delete takes, or `None` for a
    /// delete that skips it. Ignored for an instance, which has no such member.
    final_snapshot: Option<&'static str>,
}

/// The table for the release. The metrics are rows of the crate's published
/// alarm catalog for the resource type (`crate::state::ALARM_METRICS`, which
/// quotes the modelled `PutAlarmRequest$metricName` list).
const ROWS: &[Row] = &[
    Row {
        case: "an instance with one alarm, deleted before the create has settled",
        kind: Monitored::Instance,
        region: "us-east-1",
        zone: "us-east-1a",
        deleted: "watched-box",
        watching: &[("watched-box-cpu", "CPUUtilization")],
        survivor: None,
        survivor_watching: &[],
        settle: false,
        final_snapshot: None,
    },
    Row {
        case: "an instance with both alarms one metric accepts plus one on another metric, \
               beside another watched instance",
        kind: Monitored::Instance,
        region: "us-west-2",
        zone: "us-west-2a",
        deleted: "metrics_host_1",
        watching: &[
            ("metrics-host-cpu-a", "CPUUtilization"),
            ("metrics-host-cpu-b", "CPUUtilization"),
            ("metrics-host-net", "NetworkOut"),
        ],
        survivor: Some("kept-host"),
        survivor_watching: &[("kept-host-cpu", "CPUUtilization")],
        settle: true,
        final_snapshot: None,
    },
    Row {
        case: "an instance no alarm watches, beside a watched one",
        kind: Monitored::Instance,
        region: "eu-central-1",
        zone: "eu-central-1b",
        deleted: "bare-host",
        watching: &[],
        survivor: Some("observed_host_1"),
        survivor_watching: &[("observed-host-queue", "DiskQueueLength")],
        settle: false,
        final_snapshot: None,
    },
    Row {
        case: "a database with three alarms, deleted with a final snapshot, beside another \
               watched database",
        kind: Monitored::RelationalDatabase,
        region: "us-east-2",
        zone: "us-east-2a",
        deleted: "orders-db",
        watching: &[
            ("orders-db-conn-a", "DatabaseConnections"),
            ("orders-db-conn-b", "DatabaseConnections"),
            ("orders-db-cpu", "CPUUtilization"),
        ],
        survivor: Some("reports_db_1"),
        survivor_watching: &[("reports-db-space", "FreeStorageSpace")],
        settle: false,
        final_snapshot: Some("orders-db-final"),
    },
    Row {
        case: "a database with one alarm, deleted skipping the final snapshot",
        kind: Monitored::RelationalDatabase,
        region: "ap-southeast-2",
        zone: "ap-southeast-2a",
        deleted: "Cache-DB-2",
        watching: &[("cache-db-queue", "DiskQueueDepth")],
        survivor: None,
        survivor_watching: &[],
        settle: false,
        final_snapshot: None,
    },
];

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// Put the alarm `name` on `metric` of the monitored resource `monitored`,
/// requiring it to be accepted.
async fn put_alarm(emulator: &Emulator, case: &str, name: &str, monitored: &str, metric: &str) {
    let (status, body) = try_put_alarm(emulator, name, monitored, metric).await;
    assert_eq!(
        status, 200,
        "case {case}: expected the alarm {name} on {metric} of {monitored} to be accepted, \
         got {status} {body:?}"
    );
}

/// Put the alarm `name` on `metric` of `monitored`, answering what the service
/// said — for a case that is about whether the put is accepted at all.
async fn try_put_alarm(
    emulator: &Emulator,
    name: &str,
    monitored: &str,
    metric: &str,
) -> (u16, Value) {
    emulator
        .call(
            "PutAlarm",
            json!({
                "alarmName": name,
                "monitoredResourceName": monitored,
                "metricName": metric,
                "comparisonOperator": "GreaterThanThreshold",
                "threshold": 1.0,
                "evaluationPeriods": 1,
            }),
        )
        .await
}

/// Create the monitored resource `name` of `kind` in `zone` and, when `settle`,
/// wait for the clock to put it into the state a ready resource of that kind
/// reports.
async fn create_monitored(
    emulator: &Emulator,
    case: &str,
    kind: Monitored,
    name: &str,
    zone: &str,
    settle: bool,
) {
    monitored_resource(emulator, kind, name, zone, case).await;
    if settle && kind == Monitored::Instance {
        wait_for_state(emulator, name, "running").await;
    }
}

/// Delete the monitored resource `name` of `kind`, requiring success and
/// answering the `operations` the call logged.
async fn delete_monitored(
    emulator: &Emulator,
    case: &str,
    kind: Monitored,
    name: &str,
    final_snapshot: Option<&str>,
) -> Vec<Value> {
    let (operation, body) = match kind {
        Monitored::Instance => ("DeleteInstance", json!({"instanceName": name})),
        Monitored::RelationalDatabase => {
            let mut body = json!({"relationalDatabaseName": name});
            match final_snapshot {
                Some(snapshot) => {
                    body["finalRelationalDatabaseSnapshotName"] = json!(snapshot);
                }
                None => body["skipFinalSnapshot"] = json!(true),
            }
            ("DeleteRelationalDatabase", body)
        }
    };
    let answered = emulator.ok(operation, body).await;
    answered["operations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {answered:?}"))
        .clone()
}

/// How many operation-log entries a delete of `kind` logs for its OWN work,
/// before the releases: the removal itself, plus the final snapshot a database
/// delete was asked for.
fn own_entries(kind: Monitored, final_snapshot: Option<&str>) -> usize {
    match kind {
        Monitored::Instance => 1,
        Monitored::RelationalDatabase => 1 + usize::from(final_snapshot.is_some()),
    }
}

/// Assert every alarm the account holds watches a resource the account still
/// holds — the state `PutAlarm` is the only door into, and the one it refuses to
/// create.
async fn assert_every_alarm_watches_a_live_resource(emulator: &Emulator, case: &str) {
    let (held, _) = alarms(emulator, json!({})).await;
    for alarm in &held {
        let monitored = alarm["monitoredResourceInfo"].clone();
        let name = monitored["name"].as_str().unwrap_or_else(|| {
            panic!("case {case}: expected every alarm to name its monitored resource, got {alarm:?}")
        });
        let (read, (status, answered)) = match monitored["resourceType"].as_str() {
            Some("Instance") => (
                "GetInstance",
                emulator
                    .call("GetInstance", json!({"instanceName": name}))
                    .await,
            ),
            Some("RelationalDatabase") => (
                "GetRelationalDatabase",
                emulator
                    .call(
                        "GetRelationalDatabase",
                        json!({"relationalDatabaseName": name}),
                    )
                    .await,
            ),
            other => panic!(
                "case {case}: an alarm reported the unexpected monitored resource type \
                 {other:?} in {alarm:?}"
            ),
        };
        assert_eq!(
            status, 200,
            "case {case}: the alarm {:?} watches {name}, so {read} must still answer it, \
             got {status} {answered:?}",
            alarm["name"]
        );
    }
}

// ---------------------------------------------------------------------------
// The release
// ---------------------------------------------------------------------------

/// RULE: deleting a monitored resource releases every alarm the store holds that
/// watches it — the alarm is gone from `GetAlarms` and from `DeleteAlarm`, one
/// `DeleteAlarm` entry per released alarm is logged beside the delete's own
/// entries, and an alarm watching another resource is untouched.
#[tokio::test]
async fn deleting_a_monitored_resource_releases_every_alarm_watching_it() {
    let emulator = Emulator::new();
    // Every alarm the account is expected to still hold, grown and shrunk row by
    // row: the store is shared across the rows, so each row's assertion is also
    // a check that no earlier row's alarms were disturbed.
    let mut expected: BTreeSet<String> = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        create_monitored(&emulator, case, row.kind, row.deleted, row.zone, row.settle).await;
        for (name, metric) in row.watching {
            put_alarm(&emulator, case, name, row.deleted, metric).await;
            expected.insert((*name).to_string());
        }
        if let Some(survivor) = row.survivor {
            create_monitored(&emulator, case, row.kind, survivor, row.zone, false).await;
            for (name, metric) in row.survivor_watching {
                put_alarm(&emulator, case, name, survivor, metric).await;
                expected.insert((*name).to_string());
            }
        }

        // What a client sees before the delete: every alarm of the row watches
        // the resource the row is about to delete.
        let (before, _) = alarms(&emulator, json!({"monitoredResourceName": row.deleted})).await;
        let watching: Vec<String> = row
            .watching
            .iter()
            .map(|(name, _)| (*name).to_string())
            .collect();
        assert_eq!(
            names(&before),
            watching,
            "case {case}: expected the alarms of the row to start out watching {}",
            row.deleted
        );

        let entries =
            delete_monitored(&emulator, case, row.kind, row.deleted, row.final_snapshot).await;

        // The log: the delete's own entries, then one release per alarm — each
        // already done when the call returned, so each is terminal, and each
        // located where the alarm was rather than where the removed resource
        // was zoned.
        let own = own_entries(row.kind, row.final_snapshot);
        assert_eq!(
            entries.len(),
            own + row.watching.len(),
            "case {case}: expected {own} entries for the delete and one per released alarm, \
             got {entries:?}"
        );
        let mut released: Vec<String> = Vec::new();
        for entry in &entries[own..] {
            assert_eq!(
                (
                    entry["operationType"].as_str(),
                    entry["resourceType"].as_str(),
                    entry["status"].as_str(),
                    entry["isTerminal"].as_bool(),
                    entry["location"]["regionName"].as_str(),
                    entry["location"]["availabilityZone"].as_str(),
                ),
                (
                    Some("DeleteAlarm"),
                    Some("Alarm"),
                    Some("Succeeded"),
                    Some(true),
                    Some(row.region),
                    None,
                ),
                "case {case}: expected a succeeded DeleteAlarm entry located where the alarm \
                 was, got {entry:?}"
            );
            let id = entry["id"].as_str().unwrap_or_default();
            assert!(
                is_uuid_shaped(id),
                "case {case}: expected a uuid-shaped operation id, got {id:?}"
            );
            released.push(
                entry["resourceName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        released.sort();
        let mut expected_released = watching.clone();
        expected_released.sort();
        assert_eq!(
            released, expected_released,
            "case {case}: expected one release logged per alarm that watched the resource"
        );

        // The alarms are gone from every read...
        for name in &watching {
            expected.remove(name);
        }
        let (gone, _) = alarms(&emulator, json!({"monitoredResourceName": row.deleted})).await;
        assert!(
            gone.is_empty(),
            "case {case}: expected no alarm left watching {}, got {gone:?}",
            row.deleted
        );
        let (held, _) = alarms(&emulator, json!({})).await;
        assert_eq!(
            names(&held),
            expected.iter().cloned().collect::<Vec<String>>(),
            "case {case}: expected only the alarms watching a resource the account still \
             holds, got {held:?}"
        );

        // ...and really removed rather than hidden: the name no alarm is stored
        // under is refused the way any other missing alarm is.
        for name in &watching {
            let (status, body) = emulator
                .call("DeleteAlarm", json!({"alarmName": name}))
                .await;
            assert_eq!(
                (status, error_code(&body).as_str(), body["code"].as_str()),
                (400, NOT_FOUND, Some(DOES_NOT_EXIST)),
                "case {case}: expected deleting the released alarm {name} to be refused, \
                 got {status} {body:?}"
            );
        }

        assert_every_alarm_watches_a_live_resource(&emulator, case).await;
    }
}

// ---------------------------------------------------------------------------
// The budget the released alarms spent
// ---------------------------------------------------------------------------

/// One monitored resource whose alarms spend its whole per-metric budget before
/// it is deleted, and the metric they spend it on.
struct Budget {
    case: &'static str,
    kind: Monitored,
    zone: &'static str,
    name: &'static str,
    /// A metric the resource type publishes (`crate::state::ALARM_METRICS`).
    metric: &'static str,
}

/// RULE (the same one, read through the budget the released alarms spent): the
/// alarms are really out of the store, so a NEW resource created under the
/// deleted one's name accepts the full per-metric alarm budget again — the
/// budget `PutAlarm` enforces counts only alarms watching a resource the
/// account holds.
#[tokio::test]
async fn a_new_resource_reusing_the_name_gets_the_whole_alarm_budget_back() {
    let emulator = Emulator::new();
    let cases: &[Budget] = &[
        Budget {
            case: "an instance name reused after the delete",
            kind: Monitored::Instance,
            zone: "us-east-1a",
            name: "recycled_box_1",
            metric: "CPUUtilization",
        },
        Budget {
            case: "a database name reused after the delete",
            kind: Monitored::RelationalDatabase,
            zone: "eu-west-1b",
            name: "recycled-db",
            metric: "DatabaseConnections",
        },
    ];

    for Budget {
        case,
        kind,
        zone,
        name,
        metric,
    } in cases
    {
        create_monitored(&emulator, case, *kind, name, zone, false).await;
        // Spend the whole budget of the metric, then watch the service refuse
        // one more.
        for index in 0..ALARMS_PER_METRIC {
            put_alarm(&emulator, case, &format!("{name}-first-{index}"), name, metric).await;
        }
        let (status, body) =
            try_put_alarm(&emulator, &format!("{name}-first-spare"), name, metric).await;
        assert_eq!(
            status, 400,
            "case {case}: expected the metric's budget to be spent, got {status} {body:?}"
        );

        delete_monitored(&emulator, case, *kind, name, None).await;
        create_monitored(&emulator, case, *kind, name, zone, false).await;

        // The successor holds the same name and its own, unspent budget.
        for index in 0..ALARMS_PER_METRIC {
            put_alarm(
                &emulator,
                case,
                &format!("{name}-second-{index}"),
                name,
                metric,
            )
            .await;
        }
        let (found, _) = alarms(&emulator, json!({"monitoredResourceName": name})).await;
        let expected: Vec<String> = (0..ALARMS_PER_METRIC)
            .map(|index| format!("{name}-second-{index}"))
            .collect();
        assert_eq!(
            names(&found),
            expected,
            "case {case}: expected the successor to hold only its own alarms, got {found:?}"
        );
        let (status, body) =
            try_put_alarm(&emulator, &format!("{name}-second-spare"), name, metric).await;
        assert_eq!(
            status, 400,
            "case {case}: expected the successor's own budget to be spent by its own alarms, \
             got {status} {body:?}"
        );
    }
}
