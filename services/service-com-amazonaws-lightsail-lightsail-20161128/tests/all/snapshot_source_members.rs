// SPDX-License-Identifier: CC0-1.0

//! The refusal every Lightsail operation that can address an AUTOMATIC snapshot
//! answers a bad combination of its source members with, driven over the wire:
//! `POST /` with `X-Amz-Target: Lightsail_20161128.<Operation>` and an
//! `awsJson1_1` body, dispatched by the multi-service router, so the
//! modelled-constraint walk, the handler and the awsJson error envelope are all
//! exercised rather than bypassed. The harness is the sibling suite's
//! (`instance_lifecycle`), so this file stands up no second emulator of its own.
//!
//! The rule under test: a request names exactly one source — a MANUAL snapshot,
//! or the instance or disk an AUTOMATIC snapshot was taken of — and the two
//! members that address an automatic snapshot (`restoreDate`,
//! `useLatestRestorableAutoSnapshot`) belong only with the second, never with
//! each other. WHICH snapshot the members address is settled before the FORM of
//! any value they carry, so a request that sent a member it should not have sent
//! at all is told that, and the `YYYY-MM-DD` form of a restore date is read only
//! where the date is a member of a request the service would act on. Every
//! operation that takes such a source answers the same sentence, in its own
//! member names and its own verb.
//!
//! The rows differ along which of the four members the request carried, along the
//! FORM of the restore date (a date, a date with the wrong separators, one with
//! too few digits, a timestamp, nothing at all) and along the value of the
//! latest-restorable flag — the dimensions the rule generalises over. The served
//! control at the end restores and copies from a manual snapshot through all
//! three doors, to show the refusal keys on the COMBINATION of members rather
//! than on the automatic-snapshot members being modelled at all.
//!
//! The database-restore door takes the sibling pair (`restoreTime`,
//! `useLatestRestorableTime`), which addresses a point in a database's
//! restorable history rather than an automatic snapshot: its own sentences, and
//! the same ordering, are pinned in their own case below.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::instance_lifecycle::{error_code, Emulator};

/// The Region, Availability Zone, blueprint and plan every fixture here is
/// created in: nothing about the source rule turns on any of them.
const REGION: &str = "us-east-1";
/// See [`REGION`].
const ZONE: &str = "us-east-1a";
/// See [`REGION`].
const BLUEPRINT: &str = "amazon_linux_2023";
/// See [`REGION`].
const BUNDLE: &str = "micro_3_0";

/// How long a case waits for a transition the clock owes.
const WAIT: Duration = Duration::from_secs(5);

/// The names a refused row puts in the two source members. Nothing resolves
/// them: the combination of members is judged before any name is looked up, so
/// these rows need no resource to exist.
const PROBE_SNAPSHOT: &str = "probe-snapshot";
/// See [`PROBE_SNAPSHOT`].
const PROBE_SOURCE: &str = "probe-source";

/// One operation that takes such a source: its name, the two source members it
/// takes them under, the words its own modelled documentation describes them
/// with, and a body that is well formed apart from those members.
///
/// SOURCE: the modelled inputs carrying `restoreDate` and
/// `useLatestRestorableAutoSnapshot`, from
/// `smithy_query(query="shape",shape="CopySnapshotRequest,\
/// CreateDiskFromSnapshotRequest,CreateInstancesFromSnapshotRequest")` — the
/// three this service models, each pairing those two members with one manual
/// snapshot member and one automatic-source member.
struct Door {
    operation: &'static str,
    snapshot_member: &'static str,
    source_member: &'static str,
    /// The clause the automatic-snapshot members' documentation states their
    /// applicability with ("Define this parameter only when …").
    only_when: &'static str,
    /// What the operation does with the snapshot it is given.
    verb: &'static str,
    /// How a refusal describes the manual snapshot.
    snapshot_description: &'static str,
    /// How a refusal describes the resource the automatic source member names.
    source_description: &'static str,
    /// What naming one source or the other does.
    either_source: &'static str,
    /// The rest of a well-formed request, for a call that names `name` as the
    /// resource it creates.
    base: fn(&str) -> Value,
}

const DOORS: &[Door] = &[
    Door {
        operation: "CopySnapshot",
        snapshot_member: "sourceSnapshotName",
        source_member: "sourceResourceName",
        only_when: "copying an automatic snapshot as a manual snapshot",
        verb: "copy",
        snapshot_description: "the manual snapshot",
        source_description: "the source instance or disk",
        either_source: "a manual snapshot is copied as another manual snapshot, or an automatic \
                        snapshot of an instance or disk is copied as a manual one",
        base: copy_snapshot_body,
    },
    Door {
        operation: "CreateDiskFromSnapshot",
        snapshot_member: "diskSnapshotName",
        source_member: "sourceDiskName",
        only_when: "creating a new disk from an automatic snapshot",
        verb: "restore",
        snapshot_description: "the disk snapshot",
        source_description: "the source disk",
        either_source: "a disk is created from a manual snapshot of a disk, or from an automatic \
                        snapshot of one",
        base: create_disk_from_snapshot_body,
    },
    Door {
        operation: "CreateInstancesFromSnapshot",
        snapshot_member: "instanceSnapshotName",
        source_member: "sourceInstanceName",
        only_when: "creating a new instance from an automatic snapshot",
        verb: "restore",
        snapshot_description: "the instance snapshot",
        source_description: "the source instance",
        either_source: "an instance is created from a manual snapshot of an instance, or from an \
                        automatic snapshot of one",
        base: create_instances_from_snapshot_body,
    },
];

fn copy_snapshot_body(name: &str) -> Value {
    json!({"targetSnapshotName": name, "sourceRegion": REGION})
}

fn create_disk_from_snapshot_body(name: &str) -> Value {
    json!({"diskName": name, "availabilityZone": ZONE, "sizeInGb": 32})
}

fn create_instances_from_snapshot_body(name: &str) -> Value {
    json!({"instanceNames": [name], "availabilityZone": ZONE, "bundleId": BUNDLE})
}

/// The refusal a row earns, as the rule words it at one door.
enum Expected {
    /// The automatic-snapshot members were sent with a MANUAL source, which they
    /// do not belong with.
    AutomaticMembersWithManualSource,
    /// Both automatic-snapshot members were sent at once, and they are mutually
    /// exclusive.
    AutomaticMembersTogether,
    /// A restore date that is not a `YYYY-MM-DD` date, sent where a date does
    /// belong — named back as the request wrote it.
    MalformedDate(&'static str),
    /// An automatic source with neither member saying WHICH automatic snapshot.
    AutomaticSourceWithoutDate,
    /// Both sources at once.
    BothSources,
    /// Neither source.
    NoSource,
}

impl Expected {
    /// The whole sentence this refusal carries at `door`.
    fn message(&self, door: &Door) -> String {
        let Door {
            snapshot_member,
            source_member,
            only_when,
            verb,
            snapshot_description,
            source_description,
            either_source,
            ..
        } = door;
        match self {
            Self::AutomaticMembersWithManualSource => format!(
                "restoreDate and useLatestRestorableAutoSnapshot are to be defined only when \
                 {only_when}, which {source_member} names the source of, not together with \
                 {snapshot_member}"
            ),
            Self::AutomaticMembersTogether => format!(
                "restoreDate and useLatestRestorableAutoSnapshot are mutually exclusive: name \
                 the date of the automatic snapshot to {verb}, or ask for the latest restorable \
                 one, but not both"
            ),
            Self::MalformedDate(date) => {
                format!("restoreDate '{date}' must be specified in YYYY-MM-DD format")
            }
            Self::AutomaticSourceWithoutDate => format!(
                "{source_member} names {source_description} of an automatic snapshot, so one of \
                 restoreDate or useLatestRestorableAutoSnapshot must be defined with it"
            ),
            Self::BothSources => format!(
                "{snapshot_member} and {source_member} are mutually exclusive: name \
                 {snapshot_description} to {verb}, or {source_description} of the automatic \
                 snapshot to {verb}, but not both"
            ),
            Self::NoSource => format!(
                "one of {snapshot_member} or {source_member} must be defined: {either_source}"
            ),
        }
    }
}

/// One combination of source members, and the refusal it earns at every door.
struct Row {
    case: &'static str,
    /// Whether the request names the MANUAL snapshot member.
    names_snapshot: bool,
    /// Whether the request names the automatic-source member.
    names_source: bool,
    restore_date: Option<&'static str>,
    use_latest: Option<bool>,
    expected: Expected,
}

/// A date in the `YYYY-MM-DD` form the modelled `AutoSnapshotDate` pattern
/// `^[0-9]{4}-[0-9]{2}-[0-9]{2}$` admits, for the rows that are wrong about
/// something other than the date's form.
const WELL_FORMED_DATE: &str = "2026-10-24";

/// Every combination the rule refuses, over the whole table of ways a request can
/// carry these four members wrongly: the automatic-snapshot members with a manual
/// source (with a date of each form, with the flag, with both), both
/// automatic-snapshot members at once (again with a date of each form), a
/// malformed date where a date does belong, an automatic source that says which
/// snapshot neither way, both sources, and no source at all.
const ROWS: &[Row] = &[
    Row {
        case: "a malformed restore date beside a manual snapshot",
        names_snapshot: true,
        names_source: false,
        restore_date: Some("24-10-2026"),
        use_latest: None,
        expected: Expected::AutomaticMembersWithManualSource,
    },
    Row {
        case: "a well-formed restore date beside a manual snapshot",
        names_snapshot: true,
        names_source: false,
        restore_date: Some(WELL_FORMED_DATE),
        use_latest: None,
        expected: Expected::AutomaticMembersWithManualSource,
    },
    Row {
        case: "the latest restorable copy asked for beside a manual snapshot",
        names_snapshot: true,
        names_source: false,
        restore_date: None,
        use_latest: Some(true),
        expected: Expected::AutomaticMembersWithManualSource,
    },
    Row {
        case: "both automatic-snapshot members beside a manual snapshot, the date malformed",
        names_snapshot: true,
        names_source: false,
        restore_date: Some("24-10-2026"),
        use_latest: Some(true),
        expected: Expected::AutomaticMembersWithManualSource,
    },
    Row {
        case: "an empty restore date beside a manual snapshot",
        names_snapshot: true,
        names_source: false,
        restore_date: Some(""),
        use_latest: None,
        expected: Expected::AutomaticMembersWithManualSource,
    },
    Row {
        case: "both automatic-snapshot members with an automatic source, the date well formed",
        names_snapshot: false,
        names_source: true,
        restore_date: Some(WELL_FORMED_DATE),
        use_latest: Some(true),
        expected: Expected::AutomaticMembersTogether,
    },
    Row {
        case: "both automatic-snapshot members with an automatic source, the date malformed",
        names_snapshot: false,
        names_source: true,
        restore_date: Some("24-10-2026"),
        use_latest: Some(true),
        expected: Expected::AutomaticMembersTogether,
    },
    Row {
        case: "a restore date written the other way round, with an automatic source",
        names_snapshot: false,
        names_source: true,
        restore_date: Some("24-10-2026"),
        use_latest: None,
        expected: Expected::MalformedDate("24-10-2026"),
    },
    Row {
        case: "a restore date separated by slashes, with an automatic source",
        names_snapshot: false,
        names_source: true,
        restore_date: Some("2026/10/24"),
        use_latest: None,
        expected: Expected::MalformedDate("2026/10/24"),
    },
    Row {
        case: "a restore date missing a digit, with an automatic source",
        names_snapshot: false,
        names_source: true,
        restore_date: Some("2026-1-24"),
        use_latest: None,
        expected: Expected::MalformedDate("2026-1-24"),
    },
    Row {
        case: "a restore date carrying a time of day, with an automatic source",
        names_snapshot: false,
        names_source: true,
        restore_date: Some("2026-10-24T00:00:00Z"),
        use_latest: None,
        expected: Expected::MalformedDate("2026-10-24T00:00:00Z"),
    },
    Row {
        case: "an empty restore date with an automatic source",
        names_snapshot: false,
        names_source: true,
        restore_date: Some(""),
        use_latest: None,
        expected: Expected::MalformedDate(""),
    },
    Row {
        case: "an automatic source saying which snapshot neither way",
        names_snapshot: false,
        names_source: true,
        restore_date: None,
        use_latest: None,
        expected: Expected::AutomaticSourceWithoutDate,
    },
    Row {
        case: "an automatic source with the latest restorable copy declined",
        names_snapshot: false,
        names_source: true,
        restore_date: None,
        use_latest: Some(false),
        expected: Expected::AutomaticSourceWithoutDate,
    },
    Row {
        case: "both sources at once",
        names_snapshot: true,
        names_source: true,
        restore_date: None,
        use_latest: None,
        expected: Expected::BothSources,
    },
    Row {
        case: "both sources at once, with a malformed restore date",
        names_snapshot: true,
        names_source: true,
        restore_date: Some("24-10-2026"),
        use_latest: None,
        expected: Expected::BothSources,
    },
    Row {
        case: "both sources at once, with both automatic-snapshot members",
        names_snapshot: true,
        names_source: true,
        restore_date: Some(WELL_FORMED_DATE),
        use_latest: Some(true),
        expected: Expected::BothSources,
    },
    Row {
        case: "no source at all",
        names_snapshot: false,
        names_source: false,
        restore_date: None,
        use_latest: None,
        expected: Expected::NoSource,
    },
    Row {
        case: "no source at all, with a malformed restore date",
        names_snapshot: false,
        names_source: false,
        restore_date: Some("24-10-2026"),
        use_latest: None,
        expected: Expected::NoSource,
    },
    Row {
        case: "no source at all, with both automatic-snapshot members",
        names_snapshot: false,
        names_source: false,
        restore_date: Some(WELL_FORMED_DATE),
        use_latest: Some(true),
        expected: Expected::NoSource,
    },
];

/// `row`'s request at `door`, naming `name` as the resource the call would
/// create.
fn request(door: &Door, row: &Row, name: &str) -> Value {
    let mut body = (door.base)(name);
    if row.names_snapshot {
        body[door.snapshot_member] = json!(PROBE_SNAPSHOT);
    }
    if row.names_source {
        body[door.source_member] = json!(PROBE_SOURCE);
    }
    if let Some(date) = row.restore_date {
        body["restoreDate"] = json!(date);
    }
    if let Some(flag) = row.use_latest {
        body["useLatestRestorableAutoSnapshot"] = json!(flag);
    }
    body
}

// ---------------------------------------------------------------------------
// The refusal, wherever this service takes such a source
// ---------------------------------------------------------------------------

/// RULE: a request that does not name exactly one source, or that carries the
/// automatic-snapshot members anywhere but with the automatic source, is refused
/// with the modelled 400 `InvalidInputException` naming the members that are
/// wrong — the same sentence at every door that takes such a source, and the
/// combination of members is judged before the form of any value they carry.
#[tokio::test]
async fn one_source_rule_answers_every_door_the_same_way() {
    let emulator = Emulator::new();

    for door in DOORS {
        for (index, row) in ROWS.iter().enumerate() {
            let case = format!("{} with {}", door.operation, row.case);
            let name = format!("probe-{index}");
            let (status, refused) = emulator
                .call(door.operation, request(door, row, &name))
                .await;
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400, got {status} {refused:?}"
            );
            assert_eq!(
                error_code(&refused),
                "InvalidInputException",
                "case {case}: expected the wire error InvalidInputException, got {refused:?}"
            );
            assert_eq!(
                refused["message"].as_str(),
                Some(row.expected.message(door).as_str()),
                "case {case}: expected the refusal the rule words for this combination, got \
                 {refused:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The same ordering, where the pair addresses a point in time
// ---------------------------------------------------------------------------

/// The database-restore door's own pair of members (`restoreTime`,
/// `useLatestRestorableTime`) addresses a point in a database's restorable
/// history rather than an automatic snapshot, so it carries its own sentences —
/// but it is the same rule about WHICH source the request named, so the same
/// ordering holds.
///
/// SOURCE: `smithy_query(query="operation",\
/// operation="CreateRelationalDatabaseFromSnapshot")` — `restoreTime` is a
/// modelled `timestamp`, which this protocol carries as seconds since the epoch.
const POINT_IN_TIME_WITH_A_SNAPSHOT_SOURCE: &str =
    "restoreTime and useLatestRestorableTime address a point in the restorable history of the \
     database sourceRelationalDatabaseName names, so neither is defined together with \
     relationalDatabaseSnapshotName, which names the snapshot to restore";

/// See [`POINT_IN_TIME_WITH_A_SNAPSHOT_SOURCE`].
const POINT_IN_TIME_MEMBERS_TOGETHER: &str =
    "restoreTime and useLatestRestorableTime are mutually exclusive: name the point in time to \
     restore the database from, or ask for the latest restorable one, but not both";

/// See [`POINT_IN_TIME_WITH_A_SNAPSHOT_SOURCE`].
const POINT_IN_TIME_SOURCE_WITHOUT_A_TIME: &str =
    "sourceRelationalDatabaseName names the database to restore a point in time of, so one of \
     restoreTime or useLatestRestorableTime must be defined with it";

/// RULE: which source the request named is settled before any constraint on the
/// members that address a point in time, so a request carrying them with a
/// SNAPSHOT source is told they do not belong there rather than told that two
/// members it should not have sent at all conflict with each other.
#[tokio::test]
async fn the_point_in_time_members_are_judged_against_the_source_first() {
    let emulator = Emulator::new();

    // An instant inside the restorable history a database would have, in the
    // seconds-since-the-epoch form the modelled `timestamp` is carried as.
    let instant = 1_538_424_000_i64;

    // The rows differ along which of the four members the request carried and
    // along whether the pair that addresses a point in time is complete.
    let cases: &[(&str, Value, &str)] = &[
        (
            "both point-in-time members beside a snapshot source",
            json!({
                "relationalDatabaseName": "point-probe-1",
                "relationalDatabaseSnapshotName": PROBE_SNAPSHOT,
                "restoreTime": instant,
                "useLatestRestorableTime": true,
            }),
            POINT_IN_TIME_WITH_A_SNAPSHOT_SOURCE,
        ),
        (
            "a restore time beside a snapshot source",
            json!({
                "relationalDatabaseName": "point-probe-2",
                "relationalDatabaseSnapshotName": PROBE_SNAPSHOT,
                "restoreTime": instant,
            }),
            POINT_IN_TIME_WITH_A_SNAPSHOT_SOURCE,
        ),
        (
            "the latest restorable time asked for beside a snapshot source",
            json!({
                "relationalDatabaseName": "point-probe-3",
                "relationalDatabaseSnapshotName": PROBE_SNAPSHOT,
                "useLatestRestorableTime": true,
            }),
            POINT_IN_TIME_WITH_A_SNAPSHOT_SOURCE,
        ),
        (
            "both point-in-time members with a database source",
            json!({
                "relationalDatabaseName": "point-probe-4",
                "sourceRelationalDatabaseName": PROBE_SOURCE,
                "restoreTime": instant,
                "useLatestRestorableTime": true,
            }),
            POINT_IN_TIME_MEMBERS_TOGETHER,
        ),
        (
            "a database source saying which point in time neither way",
            json!({
                "relationalDatabaseName": "point-probe-5",
                "sourceRelationalDatabaseName": PROBE_SOURCE,
            }),
            POINT_IN_TIME_SOURCE_WITHOUT_A_TIME,
        ),
        (
            "a database source with the latest restorable time declined",
            json!({
                "relationalDatabaseName": "point-probe-6",
                "sourceRelationalDatabaseName": PROBE_SOURCE,
                "useLatestRestorableTime": false,
            }),
            POINT_IN_TIME_SOURCE_WITHOUT_A_TIME,
        ),
        (
            "both sources at once, with both point-in-time members",
            json!({
                "relationalDatabaseName": "point-probe-7",
                "relationalDatabaseSnapshotName": PROBE_SNAPSHOT,
                "sourceRelationalDatabaseName": PROBE_SOURCE,
                "restoreTime": instant,
                "useLatestRestorableTime": true,
            }),
            "relationalDatabaseSnapshotName and sourceRelationalDatabaseName are mutually \
             exclusive: name the database snapshot to restore, or the database to restore a \
             point in time of, but not both",
        ),
        (
            "no source at all, with both point-in-time members",
            json!({
                "relationalDatabaseName": "point-probe-8",
                "restoreTime": instant,
                "useLatestRestorableTime": true,
            }),
            "one of relationalDatabaseSnapshotName or sourceRelationalDatabaseName must be \
             defined: a database is restored from a snapshot of a database, or from a point in \
             the restorable history of one",
        ),
    ];

    for (case, body, expected) in cases {
        let (status, refused) = emulator
            .call("CreateRelationalDatabaseFromSnapshot", body.clone())
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidInputException",
            "case {case}: expected the wire error InvalidInputException, got {refused:?}"
        );
        assert_eq!(
            refused["message"].as_str(),
            Some(*expected),
            "case {case}: expected the refusal the rule words for this combination, got \
             {refused:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A manual source without the automatic-snapshot members is served
// ---------------------------------------------------------------------------

/// RULE: the refusal keys on the COMBINATION of source members, so a request
/// naming a manual snapshot and no automatic-snapshot member — the flag declined
/// included — is served, at every door that takes such a source.
#[tokio::test]
async fn a_manual_source_alone_is_served_at_every_door() {
    let emulator = Emulator::new();

    // One completed disk snapshot and one available instance snapshot, each
    // taken through the API and left to the clock to settle.
    emulator
        .ok(
            "CreateDisk",
            json!({"diskName": "served-disk", "availabilityZone": ZONE, "sizeInGb": 8}),
        )
        .await;
    emulator
        .ok(
            "CreateDiskSnapshot",
            json!({"diskSnapshotName": "served-disk-snapshot", "diskName": "served-disk"}),
        )
        .await;
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["served-instance"],
                "availabilityZone": ZONE,
                "blueprintId": BLUEPRINT,
                "bundleId": BUNDLE,
            }),
        )
        .await;
    emulator
        .ok(
            "CreateInstanceSnapshot",
            json!({
                "instanceSnapshotName": "served-instance-snapshot",
                "instanceName": "served-instance",
            }),
        )
        .await;
    wait_for_snapshot(
        &emulator,
        "GetDiskSnapshot",
        "diskSnapshotName",
        "diskSnapshot",
        "served-disk-snapshot",
        "completed",
    )
    .await;
    wait_for_snapshot(
        &emulator,
        "GetInstanceSnapshot",
        "instanceSnapshotName",
        "instanceSnapshot",
        "served-instance-snapshot",
        "available",
    )
    .await;

    // Each door's request, and the read that proves what it stored.
    let served: &[(&str, Value, &str, Value, &str)] = &[
        (
            "CopySnapshot from a manual snapshot",
            json!({
                "sourceRegion": REGION,
                "sourceSnapshotName": "served-disk-snapshot",
                "targetSnapshotName": "served-copy",
                "useLatestRestorableAutoSnapshot": false,
            }),
            "GetDiskSnapshot",
            json!({"diskSnapshotName": "served-copy"}),
            "diskSnapshot",
        ),
        (
            "CreateDiskFromSnapshot from a manual snapshot",
            json!({
                "diskName": "served-restored-disk",
                "availabilityZone": ZONE,
                "sizeInGb": 32,
                "diskSnapshotName": "served-disk-snapshot",
                "useLatestRestorableAutoSnapshot": false,
            }),
            "GetDisk",
            json!({"diskName": "served-restored-disk"}),
            "disk",
        ),
        (
            "CreateInstancesFromSnapshot from a manual snapshot",
            json!({
                "instanceNames": ["served-restored-instance"],
                "availabilityZone": ZONE,
                "bundleId": BUNDLE,
                "instanceSnapshotName": "served-instance-snapshot",
                "useLatestRestorableAutoSnapshot": false,
            }),
            "GetInstance",
            json!({"instanceName": "served-restored-instance"}),
            "instance",
        ),
    ];

    for (case, body, reader, read_body, member) in served {
        let operation = case.split(' ').next().unwrap_or(case);
        let (status, answered) = emulator.call(operation, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} {answered:?}"
        );
        let (status, read) = emulator.call(reader, read_body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case}: the resource the call created must read back, got {status} {read:?}"
        );
        assert!(
            read[member]["name"].is_string(),
            "case {case}: expected {reader} to report the resource it created, got {read:?}"
        );
    }
}

/// Poll `reader` until the snapshot `name` reports `want`.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_snapshot(
    emulator: &Emulator,
    reader: &str,
    request_member: &str,
    view_member: &str,
    name: &str,
    want: &str,
) {
    let deadline = Instant::now() + WAIT;
    loop {
        let body = emulator.ok(reader, json!({request_member: name})).await;
        let state = body[view_member]["state"].as_str().map(str::to_string);
        if state.as_deref() == Some(want) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for '{want}' on {name}: expected it within {WAIT:?}, got {state:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
