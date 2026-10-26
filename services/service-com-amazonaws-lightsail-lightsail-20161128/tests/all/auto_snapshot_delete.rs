// SPDX-License-Identifier: CC0-1.0

//! `DeleteAutoSnapshot` — deleting ONE automatic snapshot of a Lightsail
//! instance or disk, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: an automatic snapshot is addressed by the resource it
//! was taken of plus the UTC date it was taken on, so the call removes exactly
//! the one copy of that resource dated that way from the store every later read
//! answers from — leaving the resource's other copies in the order they were
//! taken, leaving the add-on that took them enabled and on the schedule it was
//! on, and appending one terminal operation-log entry against the resource the
//! copy was taken of. A name that is no instance or disk of this account, and a
//! resource holding no copy for the requested date (including one whose copy an
//! earlier call already deleted), are refused with the service's `DoesNotExist`
//! envelope and delete nothing.
//!
//! The rows of each table differ along what the rule generalises over: the KIND
//! of resource the copies were taken of, how many copies that resource holds
//! (from one up to the most this store keeps of a resource), and WHICH of them
//! the request names — the oldest, one in the middle, the newest.

use serde_json::{json, Value};

use service_com_amazonaws_lightsail_lightsail_20161128::state as lightsail_state;

// The harness, the wire error code and the minted-id check this test target
// already exposes, plus the automatic snapshot fixtures a sibling suite
// publishes: this suite drives the service through the same front door and
// builds its copies the same way rather than keeping second copies of either.
use crate::auto_snapshots::{
    auto_snapshot_request, copies, create, utc_date_of, wait_for_success, Kind, NO_COPY_WAIT,
};
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};
use crate::resource_add_ons::add_ons;

/// A day, in milliseconds — the interval between the dates of two copies the
/// add-on took on consecutive days, since it takes one a day.
const DAY_MS: i64 = 24 * 60 * 60 * 1_000;

/// The modelled `AddOnType` value the automatic snapshot add-on carries, which
/// must still be enabled after a copy it took is deleted.
const AUTO_SNAPSHOT_ADD_ON: &str = "AutoSnapshot";

/// The terminal `OperationStatus` an entry for finished work reports.
const SUCCEEDED: &str = "Succeeded";

/// The `ResourceType` key the store holds a resource's copies under.
fn resource_type_key(kind: Kind) -> &'static str {
    match kind {
        Kind::Instance => lightsail_state::RESOURCE_TYPE_INSTANCE,
        Kind::Disk => lightsail_state::RESOURCE_TYPE_DISK,
    }
}

/// Give the resource `name` of `kind` `count` further copies, each one day
/// earlier than the last, answering their dates oldest first.
///
/// The add-on takes one copy a day, so a resource holding copies for several
/// dates is a resource whose add-on has been enabled for several days — which
/// no sequence of API calls can produce inside one test. The copies are
/// therefore seeded where the store holds them, through the same public state
/// API a handler reads, so the rule is exercised against the ONE persisted
/// schedule rather than a copy of it. Each is written exactly as the add-on's
/// own taker writes a settled copy: dated by the UTC date of the instant it was
/// taken, `Success`, owed nothing further by the clock, and capturing the disks
/// attached to the resource (none, for these) when it was taken.
fn seed_earlier_copies(emulator: &Emulator, kind: Kind, name: &str, count: usize) -> Vec<String> {
    lightsail_state::with_state(emulator.engine(), |store| {
        let key = (resource_type_key(kind), name.to_string());
        let schedule = store
            .auto_snapshots
            .get_mut(&key)
            .expect("the suite enabled the add-on and waited for its first copy");
        let newest_ms = schedule
            .taken
            .last()
            .expect("the suite waited for the add-on's first copy")
            .created_at_ms;
        let mut dates = Vec::new();
        for day in 1..=count as i64 {
            let taken_at_ms = newest_ms - day * DAY_MS;
            let date = utc_date_of(taken_at_ms as f64 / 1_000.0);
            schedule.taken.insert(
                0,
                lightsail_state::StoredAutoSnapshot {
                    date: date.clone(),
                    created_at_ms: taken_at_ms,
                    status: lightsail_state::AUTO_SNAPSHOT_STATUS_SUCCESS,
                    due: None,
                    from_attached_disks: match kind {
                        Kind::Instance => Some(Vec::new()),
                        Kind::Disk => None,
                    },
                },
            );
            dates.insert(0, date);
        }
        dates
    })
    .expect("the suite's engine is the shared in-memory one")
}

/// Delete the copy of `name` dated `date`, requiring success.
async fn delete(emulator: &Emulator, name: &str, date: &str) -> Value {
    emulator
        .ok(
            "DeleteAutoSnapshot",
            json!({"resourceName": name, "date": date}),
        )
        .await
}

/// Assert that `answered` reports exactly one terminal operation against the
/// resource `name` of `kind`, answering the id it minted for it.
///
/// A deletion is complete when the call returns — the next read answers a
/// resource whose copy is gone — so the entry is terminal and `Succeeded`. It
/// reports no `operationType`: the modelled `OperationType` enum declares no
/// value for this call, and an absent optional member is the model's answer to
/// that, never an invented value.
fn assert_one_terminal_operation(case: &str, answered: &Value, kind: Kind, name: &str) -> String {
    let logged = answered["operations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        logged.len(),
        1,
        "case {case:?}: expected the one entry this call appends, got {answered:?}"
    );
    let entry = &logged[0];
    assert_eq!(
        entry["resourceName"].as_str(),
        Some(name),
        "case {case:?}: expected the entry against {name}, got {entry:?}"
    );
    assert_eq!(
        entry["resourceType"].as_str(),
        Some(kind.resource_type()),
        "case {case:?}: expected the resource's own type, got {entry:?}"
    );
    assert_eq!(
        entry["isTerminal"].as_bool(),
        Some(true),
        "case {case:?}: the copy is gone as the call returns, got {entry:?}"
    );
    assert_eq!(
        entry["status"].as_str(),
        Some(SUCCEEDED),
        "case {case:?}: expected the terminal status, got {entry:?}"
    );
    assert!(
        entry["operationType"].is_null(),
        "case {case:?}: the model declares no operationType for this call, got {entry:?}"
    );
    assert!(
        entry["createdAt"].as_f64().is_some(),
        "case {case:?}: expected the instant the entry was appended, got {entry:?}"
    );
    let id = entry["id"].as_str().unwrap_or_default().to_string();
    assert!(
        is_uuid_shaped(&id),
        "case {case:?}: expected a minted operation id, got {entry:?}"
    );
    id
}

// ---------------------------------------------------------------------------
// The deletion table
// ---------------------------------------------------------------------------

/// One resource whose copies are deleted, and which of them the request names.
struct Row {
    case: &'static str,
    kind: Kind,
    /// Distinct per row, so no row can see another's resource.
    name: &'static str,
    size_in_gb: i64,
    /// The daily hour the add-on is enabled at, where the request names one.
    hour: Option<&'static str>,
    /// Further copies the resource holds, on the days before the one its add-on
    /// took through the clock.
    earlier_copies: usize,
    /// Which copy the delete names, as an index into the resource's copies
    /// oldest first.
    delete_at: usize,
}

/// The table. `MAX_AUTO_SNAPSHOTS_PER_RESOURCE` copies is the most this store
/// keeps of one resource, so the last row names a resource holding exactly that
/// many.
const ROWS: &[Row] = &[
    Row {
        case: "an instance holding the one copy its add-on took",
        kind: Kind::Instance,
        name: "del-auto-instance-one-copy",
        size_in_gb: 0,
        hour: Some("06:00"),
        earlier_copies: 0,
        delete_at: 0,
    },
    Row {
        case: "an instance holding three copies, the oldest deleted",
        kind: Kind::Instance,
        name: "del_auto_instance_oldest",
        size_in_gb: 0,
        hour: None,
        earlier_copies: 2,
        delete_at: 0,
    },
    Row {
        case: "an instance holding four copies, one in the middle deleted",
        kind: Kind::Instance,
        name: "del-auto-instance-middle",
        size_in_gb: 0,
        hour: Some("00:00"),
        earlier_copies: 3,
        delete_at: 2,
    },
    Row {
        case: "a disk holding two copies, the newest deleted",
        kind: Kind::Disk,
        name: "del-auto-disk-newest",
        size_in_gb: 8,
        hour: Some("23:00"),
        earlier_copies: 1,
        delete_at: 1,
    },
    Row {
        case: "a large disk holding as many copies as the store keeps, one in the middle deleted",
        kind: Kind::Disk,
        name: "del-auto-disk-full",
        size_in_gb: 256,
        hour: Some("13:00"),
        earlier_copies: lightsail_state::MAX_AUTO_SNAPSHOTS_PER_RESOURCE - 1,
        delete_at: 3,
    },
];

/// Every row: the request's date names one of the resource's copies, that copy
/// is gone from what every later read reports, every other copy survives
/// unchanged and in order, the add-on stays enabled, and the call answers the
/// one terminal operation-log entry it appended — which the log itself then
/// reports.
#[tokio::test]
async fn deleting_a_dated_copy_removes_that_copy_and_keeps_every_other() {
    let emulator = Emulator::new();
    for row in ROWS {
        let case = row.case;
        create(
            &emulator,
            row.kind,
            row.name,
            &[auto_snapshot_request(row.hour)],
            row.size_in_gb,
        )
        .await;
        // The copy the clock owes, settled, so every later read of it reports
        // the same values and a comparison is about the deletion alone.
        wait_for_success(&emulator, case, row.name).await;
        seed_earlier_copies(&emulator, row.kind, row.name, row.earlier_copies);

        let held = copies(&emulator, row.name).await;
        assert_eq!(
            held.len(),
            row.earlier_copies + 1,
            "case {case:?}: expected the copies the resource holds, got {held:?}"
        );
        let target = held[row.delete_at]["date"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: every copy carries a date, got {held:?}"))
            .to_string();
        let expected: Vec<Value> = held
            .iter()
            .enumerate()
            .filter(|(at, _)| *at != row.delete_at)
            .map(|(_, copy)| copy.clone())
            .collect();

        let answered = delete(&emulator, row.name, &target).await;
        let id = assert_one_terminal_operation(case, &answered, row.kind, row.name);

        // What every later read answers: the copies that were not named, in the
        // order they were taken.
        let left = copies(&emulator, row.name).await;
        assert_eq!(
            left, expected,
            "case {case:?}: expected the other copies untouched, got {left:?}"
        );
        assert!(
            left.iter().all(|copy| copy["date"].as_str() != Some(&target)),
            "case {case:?}: expected the copy dated {target} to be gone, got {left:?}"
        );

        // The add-on that took the copies is untouched: deleting one of its
        // copies neither disables it nor changes the hour it runs at.
        let enabled = add_ons(&emulator, row.kind, row.name).await;
        let add_on = enabled
            .iter()
            .find(|add_on| add_on["name"].as_str() == Some(AUTO_SNAPSHOT_ADD_ON));
        assert!(
            add_on.is_some(),
            "case {case:?}: expected the add-on still enabled, got {enabled:?}"
        );
        if let Some(hour) = row.hour {
            assert_eq!(
                add_on.and_then(|add_on| add_on["snapshotTimeOfDay"].as_str()),
                Some(hour),
                "case {case:?}: expected the add-on still on its own hour, got {enabled:?}"
            );
        }

        // The entry the call answered is the entry the log holds: the deletion
        // was written to the one store every read answers from.
        let logged = emulator
            .ok("GetOperation", json!({"operationId": id}))
            .await["operation"]
            .clone();
        assert_eq!(
            logged, answered["operations"][0],
            "case {case:?}: expected the log to hold the entry the call answered, got {logged:?}"
        );

        // Nothing takes the deleted copy again: the add-on takes one copy a day
        // and this day's is accounted for.
        tokio::time::sleep(NO_COPY_WAIT).await;
        let still = copies(&emulator, row.name).await;
        assert_eq!(
            still, expected,
            "case {case:?}: expected no replacement copy, got {still:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// Assert that `name`/`date` is refused with the service's `DoesNotExist`
/// envelope, answering the message the refusal carried.
async fn assert_does_not_exist(
    emulator: &Emulator,
    case: &str,
    name: &str,
    date: &str,
) -> String {
    let (status, answered) = emulator
        .call(
            "DeleteAutoSnapshot",
            json!({"resourceName": name, "date": date}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the DoesNotExist status, got {answered:?}"
    );
    assert_eq!(
        error_code(&answered),
        "NotFoundException",
        "case {case:?}: expected the modelled NotFoundException, got {answered:?}"
    );
    assert_eq!(
        answered["code"].as_str(),
        Some("DoesNotExist"),
        "case {case:?}: expected the DoesNotExist code, got {answered:?}"
    );
    let message = answered["message"].as_str().unwrap_or_default().to_string();
    assert!(
        message.contains(name),
        "case {case:?}: expected the refusal to name {name}, got {answered:?}"
    );
    message
}

/// A request that names no copy this account holds deletes nothing and is
/// refused with the service's `DoesNotExist` envelope: a name that is no
/// instance or disk of the account, and a resource that holds no copy for the
/// date — one its add-on has taken nothing on, whether earlier, later or no day
/// of the calendar at all, and one whose copy a previous call already deleted.
#[tokio::test]
async fn a_request_naming_no_copy_this_account_holds_is_refused() {
    let emulator = Emulator::new();

    // A resource of a kind no automatic snapshot is taken of.
    emulator
        .ok("AllocateStaticIp", json!({"staticIpName": "del-auto-ip"}))
        .await;
    // A resource that existed and does not any more.
    create(&emulator, Kind::Instance, "del-auto-gone", &[], 0).await;
    emulator
        .ok("DeleteInstance", json!({"instanceName": "del-auto-gone"}))
        .await;
    // A resource whose add-on was never enabled, so it holds no copy at all.
    create(&emulator, Kind::Disk, "del-auto-no-add-on", &[], 8).await;

    // A resource holding exactly one copy, which stays held: every refusal here
    // is about a date that copy is not dated by.
    let case = "a disk holding one copy";
    create(
        &emulator,
        Kind::Disk,
        "del-auto-held",
        &[auto_snapshot_request(Some("08:00"))],
        16,
    )
    .await;
    let settled = wait_for_success(&emulator, case, "del-auto-held").await;
    let held = vec![settled];
    let held_date = held[0]["date"]
        .as_str()
        .expect("every copy carries a date")
        .to_string();
    let held_ms = held[0]["createdAt"].as_f64().unwrap_or_default() * 1_000.0;
    let the_day_before = utc_date_of((held_ms - DAY_MS as f64) / 1_000.0);
    let the_day_after = utc_date_of((held_ms + DAY_MS as f64) / 1_000.0);

    // (case, resourceName, date)
    let rows: &[(&str, &str, &str)] = &[
        (
            "a name nothing was created under",
            "del-auto-no-such-resource",
            "2026-10-24",
        ),
        (
            "a static IP, which no copy is taken of",
            "del-auto-ip",
            "2026-10-24",
        ),
        (
            "an instance that has been deleted",
            "del-auto-gone",
            "2026-10-24",
        ),
        (
            "a resource whose add-on was never enabled",
            "del-auto-no-add-on",
            "2026-10-24",
        ),
    ];
    for (case, name, date) in rows {
        assert_does_not_exist(&emulator, case, name, date).await;
    }
    for (case, date) in [
        ("the day before the resource's own copy", &the_day_before),
        ("the day after the resource's own copy", &the_day_after),
        // The modelled `AutoSnapshotDate` pattern admits any four digits, two
        // digits and two digits, so a value that is no day of the calendar
        // reaches the handler and names no copy either.
        (
            "a date that is no day of the calendar",
            &"2026-13-45".to_string(),
        ),
    ] {
        // A refusal about a DATE names the date as well as the resource: the two
        // together are what identifies an automatic snapshot.
        let message = assert_does_not_exist(&emulator, case, "del-auto-held", date).await;
        assert!(
            message.contains(date.as_str()),
            "case {case:?}: expected the refusal to name the date {date}, got {message:?}"
        );
    }

    // None of those refusals deleted anything.
    let still = copies(&emulator, "del-auto-held").await;
    assert_eq!(
        still, held,
        "case {case:?}: expected the refusals to delete nothing, got {still:?}"
    );

    // Deleting the copy once succeeds; deleting it again is refused, so a
    // deletion is never reported twice.
    delete(&emulator, "del-auto-held", &held_date).await;
    let left = copies(&emulator, "del-auto-held").await;
    assert!(
        left.is_empty(),
        "case {case:?}: expected the one copy deleted, got {left:?}"
    );
    let already_deleted = assert_does_not_exist(
        &emulator,
        "a date whose copy was already deleted",
        "del-auto-held",
        &held_date,
    )
    .await;
    assert!(
        already_deleted.contains(&held_date),
        "case {case:?}: expected the refusal to name the date {held_date}, got \
         {already_deleted:?}"
    );
}

/// A `date` outside the modelled `AutoSnapshotDate` pattern
/// `^[0-9]{4}-[0-9]{2}-[0-9]{2}$`, and a `resourceName` outside the modelled
/// `ResourceName` pattern `^\w[\w\-]*\w$`, are refused by the centralized
/// constraint walk with its own 400 `ValidationException`, before the handler
/// runs — and delete nothing.
#[tokio::test]
async fn a_request_outside_the_modelled_patterns_is_refused_by_the_walk() {
    let emulator = Emulator::new();
    let case = "an instance holding one copy";
    create(
        &emulator,
        Kind::Instance,
        "del-auto-pattern",
        &[auto_snapshot_request(Some("11:00"))],
        0,
    )
    .await;
    let held = vec![wait_for_success(&emulator, case, "del-auto-pattern").await];

    // (case, resourceName, date)
    let rows: &[(&str, &str, &str)] = &[
        (
            "a date whose month and day are not two digits",
            "del-auto-pattern",
            "2026-1-1",
        ),
        (
            "a date with no separators",
            "del-auto-pattern",
            "20261024",
        ),
        ("a date that is not digits", "del-auto-pattern", "yesterday"),
        ("an empty date", "del-auto-pattern", ""),
        ("a name outside the name pattern", "-nope-", "2026-10-24"),
    ];
    for (case, name, date) in rows {
        let (status, answered) = emulator
            .call(
                "DeleteAutoSnapshot",
                json!({"resourceName": name, "date": date}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the walk's refusal, got {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: expected the boundary walk's ValidationException, got {answered:?}"
        );
    }

    let still = copies(&emulator, "del-auto-pattern").await;
    assert_eq!(
        still, held,
        "case {case:?}: expected the refused requests to delete nothing, got {still:?}"
    );
}
