// SPDX-License-Identifier: CC0-1.0

//! What an `AttachDisk` leaves the CLOCK owing the disk it attached, driven over
//! the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body, on
//! the sibling suites' harness ([`crate::instance_lifecycle::Emulator`]).
//!
//! The rule under test: an operation that attaches a block storage disk stores
//! the disk `in-use`, naming the instance it is attached to and the device path
//! it is exposed at, with nothing the clock still owes that would RELEASE that
//! attachment — a settle into `available`, the modelled `DiskState` value for a
//! disk attached to nothing, which is what completes a detachment. A settle the
//! attachment does not contradict is left where it is. So an attachment holds
//! for as long as nothing asks to release it, however the disk's clock stood at
//! the instant of the attach.
//!
//! An `available` disk carrying an outstanding settle is a fixture no current
//! operation produces — a settle to `available` is scheduled only on an
//! ATTACHED, `in-use` disk (`DetachDisk`) or on one still being provisioned
//! (`CreateDisk`) — so each row puts the settle on the stored record through the
//! same public state API a handler uses, after creating both resources through
//! the API, and then drives the attach over the wire.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state as lightsail_state;

use crate::disk_attachment::{create_available_disk, create_running_instance};
use crate::instance_lifecycle::Emulator;

/// The `DiskState` values a settle can be owed in, from the service's own
/// `src/state.rs` constants (`DISK_STATE_AVAILABLE`, `DISK_STATE_IN_USE`) — the
/// modelled `DiskState` members for a disk attached to nothing and for one an
/// instance is using.
const AVAILABLE: &str = lightsail_state::DISK_STATE_AVAILABLE;
/// See [`AVAILABLE`].
const IN_USE: &str = lightsail_state::DISK_STATE_IN_USE;

/// How long after the owed instant a case reads the disk back, so the settle it
/// carried has certainly been walked by the clock.
const PAST_THE_INSTANT_MILLIS: u64 = 750;

/// The stored disk `name`, as `GetDisk` reports it.
async fn disk(emulator: &Emulator, name: &str) -> Value {
    emulator.ok("GetDisk", json!({"diskName": name})).await["disk"].clone()
}

/// Every block storage disk name the instance `name` reports in its hardware —
/// its system disk excluded, since that one is not a `Disk` resource.
async fn attached_disk_names(emulator: &Emulator, name: &str) -> Vec<String> {
    emulator
        .ok("GetInstance", json!({"instanceName": name}))
        .await["instance"]["hardware"]["disks"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|disk| disk["isSystemDisk"].as_bool() != Some(true))
        .filter_map(|disk| disk["name"].as_str().map(str::to_string))
        .collect()
}

/// Put the settle `state`, due `offset_ms` from now, on the stored disk
/// `disk_name` — the clock standing where the row says it stands at the instant
/// of the attach.
fn owe_settle(emulator: &Emulator, disk_name: &str, state: &'static str, offset_ms: i64) {
    lightsail_state::with_state(emulator.engine(), |store| {
        let at_ms = lightsail_state::now_ms().saturating_add(offset_ms);
        let disk = store
            .disk_mut(disk_name)
            .expect("the case created this disk through CreateDisk");
        disk.due = Some(lightsail_state::DueNamedState { at_ms, state });
    })
    .expect("the suite's engine is the shared in-memory one");
}

/// The state the stored disk `disk_name` is owed, when it is owed one.
fn owed_settle(emulator: &Emulator, disk_name: &str) -> Option<&'static str> {
    lightsail_state::with_state(emulator.engine(), |store| {
        store
            .disk(disk_name)
            .expect("the case created this disk through CreateDisk")
            .due
            .map(|due| due.state)
    })
    .expect("the suite's engine is the shared in-memory one")
}

/// One clock the disk's record can stand at when the attach is made.
///
/// The rows differ along everything the rule generalises over: whether a settle
/// is owed at all, the state it is owed in, whether its instant is already past
/// or still ahead, how far ahead it is, and — so no row is a rename of another —
/// the Availability Zone, the disk's size, the device path and the form of the
/// two names.
struct Row {
    case: &'static str,
    disk_name: &'static str,
    instance_name: &'static str,
    availability_zone: &'static str,
    size_in_gb: i64,
    disk_path: &'static str,
    /// The settle the disk carries when the attach is made: the state it is owed
    /// and how far from the seeding instant it falls (negative: already due).
    owed: Option<(&'static str, i64)>,
    /// The settle the store must still hold once the attach has been made: the
    /// one the disk carried, unless it was a settle into `available` — which
    /// would release the attachment the attach just made — or one the clock had
    /// already walked.
    owed_after: Option<&'static str>,
}

/// The table.
const ROWS: &[Row] = &[
    Row {
        case: "no settle owed, as the API itself leaves an available disk",
        disk_name: "my-disk",
        instance_name: "my-instance",
        availability_zone: "us-east-1a",
        size_in_gb: 8,
        disk_path: "/dev/xvdf",
        owed: None,
        owed_after: None,
    },
    Row {
        case: "a settle into available still ahead by a transition span",
        disk_name: "data_disk_512",
        instance_name: "data-host-2",
        availability_zone: "eu-west-1c",
        size_in_gb: 512,
        disk_path: "/dev/xvdg",
        owed: Some((AVAILABLE, 400)),
        owed_after: None,
    },
    Row {
        case: "a settle into available still well ahead",
        disk_name: "disk3",
        instance_name: "Instance-Ohio-3",
        availability_zone: "us-east-2b",
        size_in_gb: 16384,
        disk_path: "/dev/sdf",
        owed: Some((AVAILABLE, 800)),
        owed_after: None,
    },
    Row {
        case: "a settle into available whose instant is already past",
        disk_name: "late-disk-4",
        instance_name: "late_host_4",
        availability_zone: "ap-southeast-2a",
        size_in_gb: 64,
        disk_path: "/dev/xvdh",
        owed: Some((AVAILABLE, -1_000)),
        owed_after: None,
    },
    Row {
        case: "a settle into in-use, which the attachment does not contradict",
        disk_name: "in-use-settle-5",
        instance_name: "host5",
        availability_zone: "eu-central-1a",
        size_in_gb: 128,
        disk_path: "/dev/xvdi",
        owed: Some((IN_USE, 1_200)),
        owed_after: Some(IN_USE),
    },
];

/// RULE: the disk an `AttachDisk` attached is `in-use`, names the instance and
/// the device path, and stays that way past every instant the clock owed it —
/// the attach drops a settle into `available`, which would release the
/// attachment, and leaves any other settle alone.
#[tokio::test]
async fn an_attached_disk_keeps_its_attachment_past_the_settle_it_carried() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        create_running_instance(&emulator, row.instance_name, row.availability_zone).await;
        create_available_disk(
            &emulator,
            row.disk_name,
            row.availability_zone,
            row.size_in_gb,
        )
        .await;
        if let Some((state, offset_ms)) = row.owed {
            owe_settle(&emulator, row.disk_name, state, offset_ms);
        }

        let attached = emulator
            .ok(
                "AttachDisk",
                json!({
                    "diskName": row.disk_name,
                    "instanceName": row.instance_name,
                    "diskPath": row.disk_path,
                }),
            )
            .await;
        assert_eq!(
            attached["operations"].as_array().map(Vec::len),
            Some(1),
            "case {case}: the attach answers the one operation-log entry it \
             appended, got {attached:?}"
        );

        // What the attach left the clock owing, read from the ONE stored record
        // the handler wrote.
        let owed = owed_settle(&emulator, row.disk_name);
        assert_eq!(
            owed, row.owed_after,
            "case {case}: expected the attached disk to be owed {:?}, got {owed:?}",
            row.owed_after
        );

        // The attachment, as every reader reports it the instant it was made.
        let made = disk(&emulator, row.disk_name).await;
        assert_eq!(
            made["state"].as_str(),
            Some(IN_USE),
            "case {case}: expected an attached disk to be {IN_USE}, got {made:?}"
        );
        assert_eq!(
            made["attachedTo"].as_str(),
            Some(row.instance_name),
            "case {case}: expected the disk to name the instance it is attached \
             to, got {made:?}"
        );

        // Past every instant the disk's clock stood at: an attachment nobody
        // asked to release is still in place, on the disk AND in the holding
        // instance's reported hardware.
        let ahead_ms = row.owed.map_or(0, |(_, offset_ms)| offset_ms.max(0));
        let wait_ms = u64::try_from(ahead_ms).unwrap_or(0) + PAST_THE_INSTANT_MILLIS;
        tokio::time::sleep(Duration::from_millis(wait_ms)).await;

        let settled = disk(&emulator, row.disk_name).await;
        for (member, want) in [
            ("state", Value::from(IN_USE)),
            ("attachedTo", Value::from(row.instance_name)),
            ("path", Value::from(row.disk_path)),
            ("isAttached", Value::from(true)),
            ("attachmentState", Value::from("attached")),
        ] {
            assert_eq!(
                settled[member], want,
                "case {case}: {wait_ms}ms after the attach, expected {member} \
                 {want:?}, got {:?}",
                settled[member]
            );
        }
        assert_eq!(
            owed_settle(&emulator, row.disk_name),
            None,
            "case {case}: every settle the disk carried is walked by now, got {:?}",
            owed_settle(&emulator, row.disk_name)
        );
        let names = attached_disk_names(&emulator, row.instance_name).await;
        assert_eq!(
            names,
            vec![row.disk_name.to_string()],
            "case {case}: the holding instance still reports the attached disk in \
             its hardware, got {names:?}"
        );
    }
}
