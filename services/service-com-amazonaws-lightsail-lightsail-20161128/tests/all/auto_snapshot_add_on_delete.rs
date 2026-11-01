// SPDX-License-Identifier: CC0-1.0

//! `forceDeleteAddOns` on `DeleteInstance` and `DeleteDisk` — what a delete does
//! with the automatic snapshots the deleted resource's add-on had taken — driven
//! over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. Every resource a case deletes was created through the API and every
//! copy it asserts on was taken by the add-on's own schedule — no case reaches
//! into the store.
//!
//! The rule under test: the member is "a Boolean value to indicate whether to
//! delete all add-ons" for the resource the call deletes (the modelled
//! `DeleteInstanceRequest$forceDeleteAddOns` and
//! `DeleteDiskRequest$forceDeleteAddOns`), so it is what decides the fate of what
//! that resource's add-on produced: the automatic snapshots its schedule took of
//! the resource. A delete ASKED to delete the add-ons takes those copies with the
//! record, after which the name addresses nothing. A delete that was NOT asked —
//! the member absent, or `false` — keeps them: they stay reportable under the
//! name they were taken under, with the resource's own `ResourceType`, and
//! deletable one date at a time, located where the resource was; no further copy
//! is taken of a resource the account no longer holds; and a resource created
//! under that name afterwards takes the name over and starts with none —
//! whichever of the two kinds that resource is, since a Lightsail name is unique
//! per resource type. The flag decides nothing else: the delete is served either
//! way, answers the same one terminal entry, and removes the resource.
//!
//! The rows differ along what the rule generalises over: the KIND of resource the
//! add-on was enabled on, the value of the flag (absent, `false`, `true`), the
//! Availability Zone — and so the region — the resource was in, the daily hour
//! the add-on carries (named, and left to the service to choose), the size of the
//! disk, and the form of the name.

use std::time::Duration;

use serde_json::{json, Value};

// The automatic snapshot fixtures a sibling suite publishes, and that suite's
// own harness and error-code reader: this suite drives the service through the
// same front door and builds its copies the same way rather than keeping second
// copies of either.
use crate::auto_snapshots::{auto_snapshot_request, copies, wait_for_success, Kind, NO_COPY_WAIT};
use crate::instance_lifecycle::{error_code, Emulator};

/// A Linux/Unix blueprint and the smallest plan: neither selects anything about
/// the automatic snapshot add-on.
const BLUEPRINT: &str = "amazon_linux_2023";
/// See [`BLUEPRINT`].
const BUNDLE: &str = "nano_3_0";

/// The terminal `OperationStatus` an entry for finished work reports.
const SUCCEEDED: &str = "Succeeded";

/// One resource whose add-on has taken a copy, and the delete that is sent for
/// it.
struct Row {
    case: &'static str,
    kind: Kind,
    name: &'static str,
    /// The zone the resource is created in, which is where every entry logged
    /// for it — the delete's own, and a deletion of one of its kept copies — is
    /// located.
    zone: &'static str,
    /// The daily hour asked of the add-on, or `None` to leave the hour to the
    /// service.
    hour: Option<&'static str>,
    /// The size of the disk a disk row creates; ignored by an instance row.
    size_in_gb: i64,
    /// What the delete sends for `forceDeleteAddOns`: `None` leaves the member
    /// out of the request altogether.
    force_delete_add_ons: Option<bool>,
}

impl Row {
    /// Whether this row's delete keeps the copies the add-on took: it keeps them
    /// unless it was asked to delete the resource's add-ons.
    fn keeps_the_copies(&self) -> bool {
        !self.force_delete_add_ons.unwrap_or(false)
    }

    /// The delete operation and the member it names the resource by.
    fn delete(&self) -> (&'static str, &'static str) {
        match self.kind {
            Kind::Instance => ("DeleteInstance", "instanceName"),
            Kind::Disk => ("DeleteDisk", "diskName"),
        }
    }

    /// The read that answers the resource itself, and the member it names it by.
    fn read(&self) -> (&'static str, &'static str) {
        match self.kind {
            Kind::Instance => ("GetInstance", "instanceName"),
            Kind::Disk => ("GetDisk", "diskName"),
        }
    }
}

/// The table: every combination of the resource kind and the three values the
/// flag can arrive as, each in its own zone, under its own name and at its own
/// add-on hour.
const ROWS: &[Row] = &[
    Row {
        case: "an instance deleted with no forceDeleteAddOns member",
        kind: Kind::Instance,
        name: "keep-add-on-instance",
        zone: "us-east-1a",
        hour: Some("06:00"),
        size_in_gb: 0,
        force_delete_add_ons: None,
    },
    Row {
        case: "an instance deleted with forceDeleteAddOns false",
        kind: Kind::Instance,
        name: "keep_add_on_instance_2",
        zone: "eu-west-1c",
        hour: Some("23:00"),
        size_in_gb: 0,
        force_delete_add_ons: Some(false),
    },
    Row {
        case: "an instance deleted with forceDeleteAddOns true",
        kind: Kind::Instance,
        name: "force-add-on-instance",
        zone: "ap-southeast-2b",
        hour: None,
        size_in_gb: 0,
        force_delete_add_ons: Some(true),
    },
    Row {
        case: "a disk deleted with no forceDeleteAddOns member",
        kind: Kind::Disk,
        name: "keep-add-on-disk",
        zone: "us-west-2a",
        hour: Some("13:00"),
        size_in_gb: 8,
        force_delete_add_ons: None,
    },
    Row {
        case: "a large disk deleted with forceDeleteAddOns false",
        kind: Kind::Disk,
        name: "keep_add_on_disk_2048",
        zone: "ap-northeast-1c",
        hour: None,
        size_in_gb: 2048,
        force_delete_add_ons: Some(false),
    },
    Row {
        case: "a disk deleted with forceDeleteAddOns true",
        kind: Kind::Disk,
        name: "force-add-on-disk",
        zone: "eu-central-1b",
        hour: Some("00:00"),
        size_in_gb: 16,
        force_delete_add_ons: Some(true),
    },
];

/// Create the row's resource in its own zone with the automatic snapshot add-on
/// enabled, and wait until it is deletable.
///
/// The sibling suite's `create` builds the same request but in the one zone it
/// publishes, and the zone is what this suite's rows vary — so the body is built
/// here, from the same two members (`addOns` plus the resource's own create
/// members).
async fn create_with_the_add_on(emulator: &Emulator, row: &Row) {
    let add_ons = json!([auto_snapshot_request(row.hour)]);
    match row.kind {
        Kind::Instance => {
            emulator
                .ok(
                    "CreateInstances",
                    json!({
                        "instanceNames": [row.name],
                        "availabilityZone": row.zone,
                        "blueprintId": BLUEPRINT,
                        "bundleId": BUNDLE,
                        "addOns": add_ons,
                    }),
                )
                .await;
        }
        Kind::Disk => {
            emulator
                .ok(
                    "CreateDisk",
                    json!({
                        "diskName": row.name,
                        "availabilityZone": row.zone,
                        "sizeInGb": row.size_in_gb,
                        "addOns": add_ons,
                    }),
                )
                .await;
            // "The disk must be in the available state (not attached to a
            // Lightsail instance)" — the modelled `DeleteDisk` documentation —
            // so a disk is waited for before it is deleted. An instance is
            // deletable in any state.
            wait_for_disk_state(emulator, row, "available").await;
        }
    }
}

/// Poll `GetDisk` until the row's disk reports `want`.
async fn wait_for_disk_state(emulator: &Emulator, row: &Row, want: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let state = emulator
            .ok("GetDisk", json!({"diskName": row.name}))
            .await["disk"]["state"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if state == want {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {:?}: expected the disk {} to be {want} within 5s, got {state:?}",
            row.case,
            row.name
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The `message` of a failure body.
fn message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or("<no message>")
        .to_string()
}

/// Assert that `answered` carries exactly one terminal entry of
/// `operation_type` against `name`, located in `zone`.
fn assert_one_terminal_operation(
    case: &str,
    answered: &Value,
    operation_type: &str,
    resource_type: &str,
    name: &str,
    zone: &str,
) {
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
        (
            entry["operationType"].as_str(),
            entry["resourceType"].as_str(),
            entry["resourceName"].as_str(),
            entry["status"].as_str(),
            entry["isTerminal"].as_bool(),
            entry["location"]["availabilityZone"].as_str(),
        ),
        (
            Some(operation_type),
            Some(resource_type),
            Some(name),
            Some(SUCCEEDED),
            Some(true),
            Some(zone),
        ),
        "case {case:?}: expected a finished {operation_type} entry for {name} in {zone}, got \
         {entry:?}"
    );
}

/// Assert that the name `name` addresses no automatic snapshots of this account:
/// the service's `DoesNotExist` envelope from both doors that address a
/// resource's copies, each deleting and reporting nothing.
async fn assert_no_copies_are_addressed(emulator: &Emulator, case: &str, name: &str, date: &str) {
    for (door, request) in [
        ("GetAutoSnapshots", json!({"resourceName": name})),
        (
            "DeleteAutoSnapshot",
            json!({"resourceName": name, "date": date}),
        ),
    ] {
        let (status, body) = emulator.call(door, request).await;
        assert_eq!(
            (status, error_code(&body).as_str(), body["code"].as_str()),
            (400, "NotFoundException", Some("DoesNotExist")),
            "case {case:?}: expected {door} to refuse {name}, got {body:?}"
        );
        assert_eq!(
            message(&body),
            format!(
                "resourceName '{name}' does not name a resource of this account that carries \
                 add-ons"
            ),
            "case {case:?}: expected {door}'s shared add-on resolution sentence, got {body:?}"
        );
    }
}

/// RULE: `forceDeleteAddOns` decides whether the copies the deleted resource's
/// add-on took go with it. A delete asked for the add-ons leaves nothing the name
/// addresses; a delete that was not asked keeps the copies — reportable with the
/// resource's own type, unchanged by the clock now that no resource is there to
/// copy, and deletable by date, located where the resource was. Either way the
/// delete is served, answers one terminal entry and removes the resource.
#[tokio::test]
async fn a_delete_keeps_the_add_ons_copies_unless_it_is_asked_to_delete_the_add_ons() {
    let emulator = Emulator::new();

    for row in ROWS {
        let case = row.case;
        create_with_the_add_on(&emulator, row).await;
        // The add-on's own schedule takes the copy; the case only waits for it.
        let taken = wait_for_success(&emulator, case, row.name).await;
        let date = taken["date"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected the copy to carry a date"))
            .to_string();

        let (delete, member) = row.delete();
        let mut request = json!({member: row.name});
        if let Some(force) = row.force_delete_add_ons {
            request["forceDeleteAddOns"] = json!(force);
        }
        let answered = emulator.ok(delete, request).await;
        assert_one_terminal_operation(
            case,
            &answered,
            delete,
            row.kind.resource_type(),
            row.name,
            row.zone,
        );

        // The resource is gone whatever the flag said: the member is about the
        // add-ons, not about whether the delete is served.
        let (read, read_member) = row.read();
        let (status, body) = emulator.call(read, json!({read_member: row.name})).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "NotFoundException"),
            "case {case:?}: expected the deleted resource to be gone, got {body:?}"
        );

        if !row.keeps_the_copies() {
            assert_no_copies_are_addressed(&emulator, case, row.name, &date).await;
            continue;
        }

        // KEPT: the same copies, under the same name, with the resource's own
        // `ResourceType`.
        let reported = emulator
            .ok("GetAutoSnapshots", json!({"resourceName": row.name}))
            .await;
        assert_eq!(
            (
                reported["resourceName"].as_str(),
                reported["resourceType"].as_str(),
                reported["autoSnapshots"].clone(),
            ),
            (
                Some(row.name),
                Some(row.kind.resource_type()),
                json!([taken.clone()]),
            ),
            "case {case:?}: expected the add-on's copy kept under its own name, got {reported:?}"
        );

        // And no further copy is taken: the schedule stopped with the resource,
        // so what the clock owes a resource the account no longer holds is
        // nothing.
        tokio::time::sleep(NO_COPY_WAIT).await;
        let after = copies(&emulator, row.name).await;
        assert_eq!(
            after,
            vec![taken.clone()],
            "case {case:?}: expected the kept copies to stay exactly {taken:?}, got {after:?}"
        );

        // A kept copy is deletable by its date, and the entry that records it is
        // located where the resource was.
        let deleted = emulator
            .ok(
                "DeleteAutoSnapshot",
                json!({"resourceName": row.name, "date": date}),
            )
            .await;
        assert_one_terminal_operation(
            case,
            &deleted,
            "DeleteAutoSnapshot",
            row.kind.resource_type(),
            row.name,
            row.zone,
        );

        // That was the resource's last copy, so the name now addresses nothing.
        assert_no_copies_are_addressed(&emulator, case, row.name, &date).await;
    }
}

/// RULE: a copy is addressed by the name of the resource it was taken of, so a
/// resource created under a name whose copies a delete kept takes that name over:
/// it starts with the copies its OWN add-on has taken — none — and the kept
/// copies are not reported as its.
#[tokio::test]
async fn a_resource_created_under_a_kept_name_starts_with_no_copies() {
    let emulator = Emulator::new();

    let rows: &[&Row] = &[&ROWS[0], &ROWS[3]];
    for row in rows {
        let case = row.case;
        create_with_the_add_on(&emulator, row).await;
        let taken = wait_for_success(&emulator, case, row.name).await;

        let (delete, member) = row.delete();
        emulator.ok(delete, json!({member: row.name})).await;
        let kept = copies(&emulator, row.name).await;
        assert_eq!(
            kept,
            vec![taken],
            "case {case:?}: expected the copy kept by a delete that was not asked to delete the \
             add-ons, got {kept:?}"
        );

        // The same name again, this time with NO add-on: the new resource holds
        // no copy, and none is owed it.
        match row.kind {
            Kind::Instance => {
                emulator
                    .ok(
                        "CreateInstances",
                        json!({
                            "instanceNames": [row.name],
                            "availabilityZone": row.zone,
                            "blueprintId": BLUEPRINT,
                            "bundleId": BUNDLE,
                        }),
                    )
                    .await;
            }
            Kind::Disk => {
                emulator
                    .ok(
                        "CreateDisk",
                        json!({
                            "diskName": row.name,
                            "availabilityZone": row.zone,
                            "sizeInGb": row.size_in_gb,
                        }),
                    )
                    .await;
            }
        }
        tokio::time::sleep(NO_COPY_WAIT).await;
        let reported = copies(&emulator, row.name).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: expected the resource created under the name to start with no copy, \
             got {reported:?}"
        );
    }
}

/// RULE (the same takeover, across the two kinds a copy can be taken of): a
/// Lightsail name is unique per resource TYPE, so a disk may be created under the
/// name an instance held. That create takes the name over too: the copies kept of
/// the instance are not the disk's, and deleting the disk — asked for its add-ons
/// or not — brings none of them back.
#[tokio::test]
async fn a_name_taken_over_by_the_other_kind_discards_the_kept_copies() {
    let emulator = Emulator::new();
    let case = "an instance's kept copies, under a name a disk then takes over";
    let instance = &ROWS[0];

    create_with_the_add_on(&emulator, instance).await;
    let taken = wait_for_success(&emulator, case, instance.name).await;
    emulator
        .ok("DeleteInstance", json!({"instanceName": instance.name}))
        .await;
    let kept = copies(&emulator, instance.name).await;
    assert_eq!(
        kept,
        vec![taken],
        "case {case:?}: expected the instance's copy kept by a delete that was not asked to \
         delete the add-ons, got {kept:?}"
    );

    // A DISK under the same name, with no add-on of its own.
    emulator
        .ok(
            "CreateDisk",
            json!({
                "diskName": instance.name,
                "availabilityZone": instance.zone,
                "sizeInGb": 8,
            }),
        )
        .await;
    let reported = emulator
        .ok("GetAutoSnapshots", json!({"resourceName": instance.name}))
        .await;
    assert_eq!(
        (
            reported["resourceType"].as_str(),
            reported["autoSnapshots"].clone(),
        ),
        (Some(Kind::Disk.resource_type()), json!([])),
        "case {case:?}: expected the disk to hold the name and no copy, got {reported:?}"
    );

    // And once that disk goes, the instance's copies do not come back.
    wait_for_disk_state(&emulator, instance, "available").await;
    emulator
        .ok(
            "DeleteDisk",
            json!({"diskName": instance.name, "forceDeleteAddOns": true}),
        )
        .await;
    let date = kept[0]["date"].as_str().unwrap_or_default().to_string();
    assert_no_copies_are_addressed(&emulator, case, instance.name, &date).await;
}
