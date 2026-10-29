// SPDX-License-Identifier: CC0-1.0

//! Restoring a Lightsail block storage disk from an AUTOMATIC snapshot of a
//! disk, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: a restore that addresses an automatic snapshot resolves
//! it among the copies the `AutoSnapshot` add-on has taken of the disk
//! `sourceDiskName` names — the copy dated `restoreDate`, or the newest copy the
//! service has FINISHED taking where `useLatestRestorableAutoSnapshot` asks for
//! the latest restorable one — and restores a new disk holding the volume that
//! copy was taken of, at the size the request asks for and never below that
//! volume's, leaving the copy and the disk it was taken of exactly as they were.
//! A request addressing a copy this account does not hold is refused with the
//! service's `DoesNotExist` envelope, one addressing a copy the service has not
//! finished taking with the modelled `OperationFailureException`, and neither
//! stores anything.
//!
//! The manual source of the same operation — `diskSnapshotName` — is pinned in
//! `disk_restore_from_snapshot.rs`, and the same automatic source under
//! `CopySnapshot`'s own member names in `snapshot_copy.rs`.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_lightsail_lightsail_20161128::state as lightsail_state;

// The harness, the wire error code and the minted-id check this test target
// already exposes, plus the automatic snapshot fixtures a sibling suite
// publishes: this suite drives the service through the same front door and
// builds its copies the same way rather than keeping second copies of either.
use crate::auto_snapshots::{auto_snapshot_request, copies, utc_date_of, wait_for_success};
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN and
/// support code carries.
const ACCOUNT: &str = "000000000000";

/// A day, in milliseconds: the add-on takes one copy a day, so two consecutive
/// copies of one disk are dated a day apart.
const DAY_MS: i64 = 24 * 60 * 60 * 1_000;

/// The modelled `AutoSnapshotStatus` values a copy reports: the one a copy the
/// service is still taking is in, and the one it settles into.
const IN_PROGRESS: &str = "InProgress";
/// See [`IN_PROGRESS`].
const SUCCESS: &str = "Success";

/// The published size range of a Lightsail block storage disk, in GB: 8 GB to
/// 16 TB. The modelled `CreateDiskFromSnapshot$sizeInGb` carries no `@range`, so
/// these are the bounds the service itself enforces.
const MIN_SIZE_IN_GB: i64 = 8;
/// See [`MIN_SIZE_IN_GB`].
const MAX_SIZE_IN_GB: i64 = 16 * 1024;

/// The IOPS baseline a Lightsail disk inherits from the EBS general purpose SSD
/// (`gp2`) volume behind it: 3 IOPS per GB, never below 100 and never above
/// 16,000.
fn want_iops(size_in_gb: i64) -> i64 {
    (size_in_gb * 3).clamp(100, 16_000)
}

/// The stored disk `name`, as `GetDisk` reports it.
async fn disk(emulator: &Emulator, name: &str) -> Value {
    emulator.ok("GetDisk", json!({"diskName": name})).await["disk"].clone()
}

/// Poll `GetDisk` until it reports `want`, answering the disk.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_disk_state(emulator: &Emulator, case: &str, name: &str, want: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let disk = disk(emulator, name).await;
        if disk["state"].as_str() == Some(want) {
            return disk;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: expected {name} to reach '{want}' within 5s, got {:?}",
            disk["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The dates of `reported`, in the order the service reports them.
fn dates(reported: &[Value]) -> Vec<String> {
    reported
        .iter()
        .map(|copy| copy["date"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The statuses of `reported`, in the same order.
fn statuses(reported: &[Value]) -> Vec<String> {
    reported
        .iter()
        .map(|copy| copy["status"].as_str().unwrap_or_default().to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// The restore table
// ---------------------------------------------------------------------------

/// How a row's request says WHICH automatic snapshot it restores: the two
/// mutually exclusive members the modelled input admits for that.
#[derive(Debug, Clone, Copy)]
enum Addressed {
    /// `restoreDate`: the UTC date the copy was taken on.
    ByDate,
    /// `useLatestRestorableAutoSnapshot`: the latest copy the service has
    /// finished taking.
    Latest,
}

/// One restore from an automatic snapshot the service must serve.
///
/// The rows differ along everything the rule generalises over: WHICH member
/// addresses the copy, the size of the volume the copy was taken of (the
/// smallest disk Lightsail creates, a middling one, a large one), how much
/// bigger than that volume the new disk is (its own size, larger, up to the
/// published maximum), the Region and zone the source lives in and whether the
/// disk is restored into that zone or another of the same Region, the hour the
/// add-on carries, the number of tags, whether an add-on is asked for, and the
/// form of the new disk's name.
#[derive(Debug)]
struct Row {
    case: &'static str,
    source_disk: &'static str,
    source_zone: &'static str,
    /// The zone the new disk is restored into — in the source disk's own Region.
    restore_zone: &'static str,
    region: &'static str,
    restored_name: &'static str,
    source_size_in_gb: i64,
    /// GB on top of the volume the copy was taken of: 0 restores at exactly its
    /// size.
    extra_gb: i64,
    addressed: Addressed,
    /// The `snapshotTimeOfDay` the source disk's add-on carries, or `None` to
    /// let the service pick the hour.
    hour: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
    /// Whether the RESTORED disk is asked to carry the automatic snapshot add-on
    /// itself.
    with_add_on: bool,
}

/// The table.
const ROWS: &[Row] = &[
    Row {
        case: "a copy of the smallest disk, restored at its own size by date",
        source_disk: "auto-source-8",
        source_zone: "us-east-1a",
        restore_zone: "us-east-1a",
        region: "us-east-1",
        restored_name: "my-disk-from-auto",
        source_size_in_gb: MIN_SIZE_IN_GB,
        extra_gb: 0,
        addressed: Addressed::ByDate,
        hour: None,
        tags: &[],
        with_add_on: false,
    },
    Row {
        case: "the latest restorable copy, restored larger into another zone of the Region, \
               tagged, with an add-on",
        source_disk: "auto_source_32",
        source_zone: "eu-west-1c",
        restore_zone: "eu-west-1a",
        region: "eu-west-1",
        restored_name: "restored_disk_1",
        source_size_in_gb: 32,
        extra_gb: 32,
        addressed: Addressed::Latest,
        hour: Some("06:00"),
        tags: &[("tier", "edge")],
        with_add_on: true,
    },
    Row {
        case: "a copy of a large disk, doubled, two tags, by date",
        source_disk: "auto-source-4096",
        source_zone: "ap-southeast-2b",
        restore_zone: "ap-southeast-2b",
        region: "ap-southeast-2",
        restored_name: "data-disk-8192",
        source_size_in_gb: 4096,
        extra_gb: 4096,
        addressed: Addressed::ByDate,
        hour: Some("23:00"),
        tags: &[("stack", "prod"), ("owner", "storage-team")],
        with_add_on: false,
    },
    Row {
        case: "the latest restorable copy, restored at the published maximum into another zone",
        source_disk: "auto-source-max",
        source_zone: "us-west-2a",
        restore_zone: "us-west-2b",
        region: "us-west-2",
        restored_name: "max_disk_16384",
        source_size_in_gb: MIN_SIZE_IN_GB,
        extra_gb: MAX_SIZE_IN_GB - MIN_SIZE_IN_GB,
        addressed: Addressed::Latest,
        hour: None,
        tags: &[("keep", "yes")],
        with_add_on: false,
    },
];

/// Create `row`'s source disk with the add-on that takes the copies and wait for
/// the copy the CLOCK then owes it, answering that copy's date.
async fn with_one_copy(emulator: &Emulator, row: &Row) -> String {
    emulator
        .ok(
            "CreateDisk",
            json!({
                "diskName": row.source_disk,
                "availabilityZone": row.source_zone,
                "sizeInGb": row.source_size_in_gb,
                "addOns": [auto_snapshot_request(row.hour)],
            }),
        )
        .await;
    let copy = wait_for_success(emulator, row.case, row.source_disk).await;
    copy["date"]
        .as_str()
        .unwrap_or_else(|| panic!("case {}: expected the copy's date, got {copy:?}", row.case))
        .to_string()
}

/// The `CreateDiskFromSnapshot` body for `row`, restoring the copy dated `date`
/// at `size_in_gb`.
fn restore_body(row: &Row, date: &str, size_in_gb: i64) -> Value {
    let mut body = json!({
        "diskName": row.restored_name,
        "availabilityZone": row.restore_zone,
        "sizeInGb": size_in_gb,
        "sourceDiskName": row.source_disk,
    });
    match row.addressed {
        Addressed::ByDate => body["restoreDate"] = json!(date),
        Addressed::Latest => body["useLatestRestorableAutoSnapshot"] = json!(true),
    }
    if !row.tags.is_empty() {
        body["tags"] = Value::Array(
            row.tags
                .iter()
                .map(|(key, value)| json!({"key": key, "value": value}))
                .collect(),
        );
    }
    if row.with_add_on {
        body["addOns"] = json!([auto_snapshot_request(None)]);
    }
    body
}

/// RULE: a disk restored from an automatic snapshot of a disk holds the volume
/// that disk carries, at the size the request asks for, in the zone it names;
/// every member `GetDisk` reports of it comes from the ONE record the restore
/// stored, and the copy it was restored from — and the disk that copy was taken
/// of — are left exactly as they were.
#[tokio::test]
async fn a_disk_restored_from_an_automatic_snapshot_copies_the_volume_it_was_taken_of() {
    let emulator = Emulator::new();
    let mut arns: BTreeSet<String> = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        let name = row.restored_name;
        let date = with_one_copy(&emulator, row).await;

        // What the restore must leave untouched, read from the service's own
        // records before it runs.
        wait_for_disk_state(&emulator, case, row.source_disk, "available").await;
        let source_before = disk(&emulator, row.source_disk).await;
        let copies_before = copies(&emulator, row.source_disk).await;

        let want_size = row.source_size_in_gb + row.extra_gb;
        let answered = emulator
            .ok("CreateDiskFromSnapshot", restore_body(row, &date, want_size))
            .await;

        // The restore answers ONE operation-log entry — it creates one disk —
        // left non-terminal until the volume it owes the disk is restored.
        let operations = answered["operations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {answered:?}"))
            .clone();
        assert_eq!(
            operations.len(),
            1,
            "case {case}: expected exactly one operation, got {operations:?}"
        );
        let operation = operations[0].clone();
        assert_eq!(
            operation["operationType"].as_str(),
            Some("CreateDiskFromSnapshot"),
            "case {case}: expected the CreateDiskFromSnapshot operation type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceName"].as_str(),
            Some(name),
            "case {case}: expected the operation to name the new disk, got {operation:?}"
        );
        assert_eq!(
            operation["isTerminal"].as_bool(),
            Some(false),
            "case {case}: expected a non-terminal operation, got {operation:?}"
        );

        // The stored record, read back through the implemented reader.
        let restored = disk(&emulator, name).await;
        assert_eq!(
            restored["name"].as_str(),
            Some(name),
            "case {case}: expected the disk name the request carried, got {restored:?}"
        );
        assert_eq!(
            restored["sizeInGb"].as_i64(),
            Some(want_size),
            "case {case}: expected the size the request asked for, got {restored:?}"
        );
        assert_eq!(
            restored["iops"].as_i64(),
            Some(want_iops(want_size)),
            "case {case}: expected the IOPS of a {want_size} GB disk, got {:?}",
            restored["iops"]
        );
        assert_eq!(
            restored["location"]["availabilityZone"].as_str(),
            Some(row.restore_zone),
            "case {case}: expected the zone the request named, got {:?}",
            restored["location"]
        );
        assert_eq!(
            restored["location"]["regionName"].as_str(),
            Some(row.region),
            "case {case}: expected the Region of that zone, got {:?}",
            restored["location"]
        );
        assert_eq!(
            restored["state"].as_str(),
            Some("pending"),
            "case {case}: expected a disk still being restored, got {restored:?}"
        );
        assert_eq!(
            restored["isAttached"].as_bool(),
            Some(false),
            "case {case}: a restored disk is attached to nothing yet, got {restored:?}"
        );

        // The tags the request carried, verbatim and in order.
        let reported_tags: Vec<(String, String)> = restored["tags"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a tags array, got {restored:?}"))
            .iter()
            .map(|tag| {
                (
                    tag["key"].as_str().unwrap_or_default().to_string(),
                    tag["value"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let want_tags: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            reported_tags, want_tags,
            "case {case}: expected the tags the request carried, got {:?}",
            restored["tags"]
        );

        // The add-ons the request asked for, and none it did not.
        let add_ons = restored["addOns"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an addOns array, got {restored:?}"))
            .clone();
        if row.with_add_on {
            assert_eq!(
                add_ons.len(),
                1,
                "case {case}: expected the one add-on the request asked for, got {add_ons:?}"
            );
            assert_eq!(
                (
                    add_ons[0]["name"].as_str(),
                    add_ons[0]["status"].as_str()
                ),
                (Some("AutoSnapshot"), Some("Enabled")),
                "case {case}: expected the AutoSnapshot add-on enabled, got {add_ons:?}"
            );
        } else {
            assert!(
                add_ons.is_empty(),
                "case {case}: expected no add-ons, got {add_ons:?}"
            );
        }

        // The ARN carries the restore zone's Region and the canonical emulator
        // account, and ends in a uuid no other resource is using; the support
        // code names the same id.
        let arn = restored["arn"].as_str().unwrap_or_default().to_string();
        let prefix = format!("arn:aws:lightsail:{}:{ACCOUNT}:Disk/", row.region);
        let id = arn
            .strip_prefix(&prefix)
            .unwrap_or_else(|| panic!("case {case}: expected an ARN starting '{prefix}', got {arn:?}"));
        assert!(
            is_uuid_shaped(id),
            "case {case}: expected a uuid-shaped ARN id, got {id:?}"
        );
        assert_eq!(
            restored["supportCode"].as_str(),
            Some(format!("{ACCOUNT}/{id}").as_str()),
            "case {case}: expected the account/resource-id support code, got {restored:?}"
        );
        assert!(
            arns.insert(arn.clone()),
            "case {case}: expected a fresh ARN, got the already-used {arn:?}"
        );

        // The clock settles the restored disk, and nothing the restore read has
        // moved: a restore READS the copy and the disk it was taken of.
        wait_for_disk_state(&emulator, case, name, "available").await;
        let copies_after = copies(&emulator, row.source_disk).await;
        assert_eq!(
            copies_after, copies_before,
            "case {case}: expected the copies of {} to be untouched, got {copies_after:?}",
            row.source_disk
        );
        let source_after = disk(&emulator, row.source_disk).await;
        assert_eq!(
            source_after, source_before,
            "case {case}: expected the source disk to be untouched, got {source_after:?}"
        );
    }
}

/// Give the disk `name` two further copies: one taken the day BEFORE its newest
/// copy and finished, and one taken the day AFTER it that the service has not
/// finished taking — answering their dates, oldest first.
///
/// A disk holding copies for several dates is one whose add-on has been enabled
/// for several days, and a copy read mid-capture is one read inside the interval
/// the service takes to finish taking it: neither is something a sequence of API
/// calls can produce inside one test. Both are therefore seeded where the store
/// holds them, through the same public state API a handler reads, so the rule is
/// exercised against the ONE persisted schedule — each written exactly as the
/// add-on's own taker writes it, dated by the UTC date of the instant it was
/// taken, and capturing no attached disk, a disk being one volume.
fn seed_neighbouring_copies(emulator: &Emulator, name: &str) -> (String, String) {
    lightsail_state::with_state(emulator.engine(), |store| {
        let schedule = store
            .auto_snapshots
            .get_mut(&(lightsail_state::RESOURCE_TYPE_DISK, name.to_string()))
            .expect("the suite enabled the add-on and waited for its first copy");
        let newest_ms = schedule
            .taken
            .last()
            .expect("the suite waited for the add-on's first copy")
            .created_at_ms;
        let earlier_ms = newest_ms - DAY_MS;
        let later_ms = newest_ms + DAY_MS;
        let earlier = utc_date_of(earlier_ms as f64 / 1_000.0);
        let later = utc_date_of(later_ms as f64 / 1_000.0);
        schedule.taken.insert(
            0,
            lightsail_state::StoredAutoSnapshot {
                date: earlier.clone(),
                created_at_ms: earlier_ms,
                status: lightsail_state::AUTO_SNAPSHOT_STATUS_SUCCESS,
                due: None,
                from_attached_disks: None,
            },
        );
        schedule.taken.push(lightsail_state::StoredAutoSnapshot {
            date: later.clone(),
            created_at_ms: later_ms,
            status: lightsail_state::AUTO_SNAPSHOT_STATUS_IN_PROGRESS,
            due: None,
            from_attached_disks: None,
        });
        (earlier, later)
    })
    .expect("the suite's engine is the shared in-memory one")
}

/// RULE: a date addresses exactly one of a disk's copies and the latest
/// restorable copy is the newest one the service has FINISHED taking, so a disk
/// holding several copies restores the one the request addresses — an older
/// finished copy by its date, the newest finished one for
/// `useLatestRestorableAutoSnapshot` — while a copy still being taken is refused
/// with the modelled `OperationFailureException` and a date no copy was taken on
/// with the `DoesNotExist` envelope.
#[tokio::test]
async fn the_copy_a_restore_uses_is_the_one_the_request_addresses() {
    let emulator = Emulator::new();
    let case = "a disk holding three copies";
    let source = "staged-disk";
    let source_size_in_gb = 16;
    emulator
        .ok(
            "CreateDisk",
            json!({
                "diskName": source,
                "availabilityZone": "us-east-1a",
                "sizeInGb": source_size_in_gb,
                "addOns": [auto_snapshot_request(Some("06:00"))],
            }),
        )
        .await;
    let finished = wait_for_success(&emulator, case, source).await;
    let finished_date = finished["date"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected the copy's date, got {finished:?}"))
        .to_string();
    let (earlier_date, unfinished_date) = seed_neighbouring_copies(&emulator, source);

    // The precondition the cases are about, read from the service's own record
    // rather than assumed: three copies, oldest first, the newest unfinished.
    let reported = copies(&emulator, source).await;
    assert_eq!(
        dates(&reported),
        vec![
            earlier_date.clone(),
            finished_date.clone(),
            unfinished_date.clone()
        ],
        "case {case}: expected the three copies oldest first, got {reported:?}"
    );
    assert_eq!(
        statuses(&reported),
        vec![SUCCESS.to_string(), SUCCESS.to_string(), IN_PROGRESS.to_string()],
        "case {case}: expected only the newest copy to be unfinished, got {reported:?}"
    );

    // The copy still being taken holds no complete volume to restore from.
    let unfinished_body = json!({
        "diskName": "from-unfinished",
        "availabilityZone": "us-east-1a",
        "sizeInGb": source_size_in_gb,
        "sourceDiskName": source,
        "restoreDate": unfinished_date,
    });
    let (status, answered) = emulator
        .call("CreateDiskFromSnapshot", unfinished_body)
        .await;
    assert_eq!(
        status, 400,
        "case {case} restoring the unfinished copy: expected a 400 refusal, got {status} \
         {answered:?}"
    );
    assert_eq!(
        error_code(&answered),
        "OperationFailureException",
        "case {case} restoring the unfinished copy: expected the modelled \
         OperationFailureException, got {answered:?}"
    );
    let message = answered["message"].as_str().unwrap_or_default().to_string();
    assert!(
        message.contains(source)
            && message.contains(&unfinished_date)
            && message.contains(IN_PROGRESS)
            && message.contains(SUCCESS),
        "case {case} restoring the unfinished copy: expected the refusal to name the disk, the \
         copy's date and the state it is in, got {message:?}"
    );
    let (read_status, read) = emulator
        .call("GetDisk", json!({"diskName": "from-unfinished"}))
        .await;
    assert_eq!(
        read_status, 400,
        "case {case} restoring the unfinished copy: a refused restore stores nothing, got \
         {read_status} {read:?}"
    );

    // A date no copy of this disk was taken on addresses no copy at all.
    let absent_date = "2020-01-01";
    let (status, answered) = emulator
        .call(
            "CreateDiskFromSnapshot",
            json!({
                "diskName": "from-absent-date",
                "availabilityZone": "us-east-1a",
                "sizeInGb": source_size_in_gb,
                "sourceDiskName": source,
                "restoreDate": absent_date,
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&answered).as_str(), answered["code"].as_str()),
        (400, "NotFoundException", Some("DoesNotExist")),
        "case {case} restoring a date no copy was taken on: expected the DoesNotExist envelope, \
         got {status} {answered:?}"
    );
    assert_eq!(
        answered["message"].as_str(),
        Some(
            format!("The automatic snapshot of the Disk {source} does not exist: {absent_date}")
                .as_str()
        ),
        "case {case} restoring a date no copy was taken on: expected the resource/date message, \
         got {answered:?}"
    );

    // Each copy the request CAN address is restored: the older finished one by
    // its date, and the newest finished one for the latest restorable.
    let served: &[(&str, &str, Value)] = &[
        (
            "the older finished copy, by its date",
            "from-earlier",
            json!({"restoreDate": earlier_date}),
        ),
        (
            "the newest finished copy, as the latest restorable one",
            "from-latest",
            json!({"useLatestRestorableAutoSnapshot": true}),
        ),
    ];
    for (served_case, restored_name, addressed) in served {
        let mut body = json!({
            "diskName": restored_name,
            "availabilityZone": "us-east-1a",
            "sizeInGb": source_size_in_gb,
            "sourceDiskName": source,
        });
        for (member, value) in addressed.as_object().expect("the source names members") {
            body[member] = value.clone();
        }
        emulator.ok("CreateDiskFromSnapshot", body).await;
        let restored = disk(&emulator, restored_name).await;
        assert_eq!(
            restored["sizeInGb"].as_i64(),
            Some(source_size_in_gb),
            "case {served_case}: expected the volume the copy was taken of, got {restored:?}"
        );
    }
}

/// What one refused row must be answered with.
#[derive(Debug)]
enum Refusal {
    /// Lightsail's `DoesNotExist`: HTTP 400, the modelled `NotFoundException`,
    /// body `code` `DoesNotExist` and exactly this message.
    DoesNotExist(&'static str),
    /// The modelled `InvalidInputException`, whose message names this member.
    Invalid(&'static str),
}

/// RULE: a restore is served only from a copy this account holds, of a disk of
/// the Region the new disk is created in, into a disk at least as large as the
/// volume that copy was taken of; a request outside that is refused and stores
/// nothing.
#[tokio::test]
async fn a_restore_the_account_cannot_serve_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    // One disk whose add-on has taken a copy, and one that carries no add-on at
    // all, so a request addressing a copy of it addresses one of a disk that has
    // never been copied.
    emulator
        .ok(
            "CreateDisk",
            json!({
                "diskName": "copied-disk",
                "availabilityZone": "us-east-1a",
                "sizeInGb": 32,
                "addOns": [auto_snapshot_request(None)],
            }),
        )
        .await;
    emulator
        .ok(
            "CreateDisk",
            json!({"diskName": "plain-disk", "availabilityZone": "us-east-1a", "sizeInGb": 16}),
        )
        .await;
    let copy = wait_for_success(&emulator, "the fixture", "copied-disk").await;
    let date = copy["date"]
        .as_str()
        .expect("the copy carries a date")
        .to_string();

    // (case, the new disk's name, the zone, the size, the source members, the refusal)
    let cases: &[(&str, &str, &str, i64, Value, Refusal)] = &[
        (
            "the latest restorable copy of a disk whose add-on has never been enabled",
            "latest-of-plain",
            "us-east-1a",
            16,
            json!({"sourceDiskName": "plain-disk", "useLatestRestorableAutoSnapshot": true}),
            Refusal::DoesNotExist(
                "The Disk plain-disk has no restorable automatic snapshot to restore",
            ),
        ),
        (
            "a dated copy of a disk whose add-on has never been enabled",
            "dated-of-plain",
            "us-east-1a",
            16,
            json!({"sourceDiskName": "plain-disk", "restoreDate": "2026-10-22"}),
            Refusal::DoesNotExist(
                "The automatic snapshot of the Disk plain-disk does not exist: 2026-10-22",
            ),
        ),
        (
            "a disk this account does not hold",
            "latest-of-absent",
            "us-east-1a",
            16,
            json!({"sourceDiskName": "absent-disk", "useLatestRestorableAutoSnapshot": true}),
            Refusal::DoesNotExist("The Disk does not exist: absent-disk"),
        ),
        (
            "one GB below the volume the copy was taken of",
            "too-small",
            "us-east-1a",
            31,
            json!({"sourceDiskName": "copied-disk", "useLatestRestorableAutoSnapshot": true}),
            Refusal::Invalid("sizeInGb"),
        ),
        (
            "well below the volume the copy was taken of, but a size Lightsail creates",
            "far-too-small",
            "us-east-1a",
            MIN_SIZE_IN_GB,
            json!({"sourceDiskName": "copied-disk", "useLatestRestorableAutoSnapshot": true}),
            Refusal::Invalid("sizeInGb"),
        ),
        (
            "a zone of another Region",
            "cross-region",
            "eu-west-1a",
            32,
            json!({"sourceDiskName": "copied-disk", "useLatestRestorableAutoSnapshot": true}),
            Refusal::Invalid("sourceDiskName"),
        ),
        (
            "a zone of no Lightsail Region",
            "no-such-region",
            "xx-middle-9z",
            32,
            json!({"sourceDiskName": "copied-disk", "useLatestRestorableAutoSnapshot": true}),
            Refusal::Invalid("availabilityZone"),
        ),
        (
            "one GB past the published maximum",
            "too-large",
            "us-east-1a",
            MAX_SIZE_IN_GB + 1,
            json!({"sourceDiskName": "copied-disk", "useLatestRestorableAutoSnapshot": true}),
            Refusal::Invalid("sizeInGb"),
        ),
    ];

    for (case, restored_name, zone, size, source, refusal) in cases {
        let mut body = json!({
            "diskName": restored_name,
            "availabilityZone": zone,
            "sizeInGb": size,
        });
        for (member, value) in source.as_object().expect("the source names members") {
            body[member] = value.clone();
        }
        let (status, answered) = emulator.call("CreateDiskFromSnapshot", body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400 refusal, got {status} {answered:?}"
        );
        match refusal {
            Refusal::DoesNotExist(message) => {
                assert_eq!(
                    (error_code(&answered).as_str(), answered["code"].as_str()),
                    ("NotFoundException", Some("DoesNotExist")),
                    "case {case}: expected the DoesNotExist envelope, got {answered:?}"
                );
                assert_eq!(
                    answered["message"].as_str(),
                    Some(*message),
                    "case {case}: expected the refusal to name what is missing, got {answered:?}"
                );
            }
            Refusal::Invalid(member) => {
                assert_eq!(
                    error_code(&answered),
                    "InvalidInputException",
                    "case {case}: expected the modelled InvalidInputException, got {answered:?}"
                );
                let message = answered["message"].as_str().unwrap_or_default().to_string();
                assert!(
                    message.contains(member),
                    "case {case}: expected the refusal to name {member}, got {message:?}"
                );
            }
        }
        let (read_status, read) = emulator
            .call("GetDisk", json!({"diskName": restored_name}))
            .await;
        assert_eq!(
            read_status, 400,
            "case {case}: a refused restore stores nothing, got {read_status} {read:?}"
        );
    }

    // Only those requests were refused: the copy is restorable, which the same
    // request made rightly proves — and the copy and its disk are untouched.
    let copies_before = copies(&emulator, "copied-disk").await;
    emulator
        .ok(
            "CreateDiskFromSnapshot",
            json!({
                "diskName": "served-restore",
                "availabilityZone": "us-east-1a",
                "sizeInGb": 32,
                "sourceDiskName": "copied-disk",
                "restoreDate": date,
            }),
        )
        .await;
    let restored = disk(&emulator, "served-restore").await;
    assert_eq!(
        restored["sizeInGb"].as_i64(),
        Some(32),
        "case the served restore: expected the volume the copy was taken of, got {restored:?}"
    );
    let copies_after = copies(&emulator, "copied-disk").await;
    assert_eq!(
        copies_after, copies_before,
        "case the served restore: expected the copies to be untouched, got {copies_after:?}"
    );
}
