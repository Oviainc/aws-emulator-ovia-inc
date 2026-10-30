// SPDX-License-Identifier: CC0-1.0

//! The snapshot NAMESPACE this service's two kinds of snapshot share, driven
//! over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: a snapshot name addresses ONE snapshot of the account, so
//! every operation that mints a snapshot name refuses a name any snapshot of the
//! account already holds, whichever kind holds it and whichever kind is being
//! minted — and the refusal leaves what was there untouched. That is what keeps
//! the service's UNQUALIFIED snapshot-name resolution single-valued: a snapshot
//! name is taken unqualified wherever this service takes one (`ExportSnapshot`'s
//! `sourceSnapshotName`, the `resourceName` a tagging call resolves), so each
//! such name resolves to the one kind of snapshot that holds it. A name the
//! account no longer holds is free again, for either kind.
//!
//! The sibling suites cover one minting operation each — `snapshot_copy.rs` a
//! copy's own duplicate-target refusal, `disk_snapshot.rs` and
//! `instance_snapshot_create.rs` the two creates — and
//! `export_snapshot_source_kinds.rs` covers an export of each kind separately.
//! This one covers the namespace they share, across every pair of minting
//! operations.

use serde_json::{json, Value};

// The harness, the wire error code, the snapshot reader and the settled-state
// wait this test target already exposes: a sibling suite drives the service
// through the same front door, stands a source resource up and reads a snapshot
// back the same way rather than writing any of it a second time.
use crate::disk_attachment::{create_available_disk, create_running_instance};
use crate::instance_lifecycle::{error_code, Emulator};
use crate::snapshot_copy::{snapshot, wait_for_snapshot_state, Kind};

/// The Availability Zone every resource of this suite lives in, and the Region
/// it is therefore in — the Region an unsigned call is made in
/// (`crate::state::DEFAULT_REGION`), which is what a copy names as its
/// `sourceRegion`.
const ZONE: &str = "us-east-1a";
/// See [`ZONE`].
const REGION: &str = "us-east-1";

/// The instance every instance snapshot, and every disk snapshot of a system
/// volume, is taken of.
const INSTANCE: &str = "namespace-instance";

/// The block storage disk every disk snapshot of a disk is taken of.
const DISK: &str = "namespace-disk";

/// The settled disk snapshot a copying row copies.
const SOURCE_DISK_SNAPSHOT: &str = "namespace-source-disk-snapshot";

/// The settled instance snapshot a copying row copies.
const SOURCE_INSTANCE_SNAPSHOT: &str = "namespace-source-instance-snapshot";

/// The size of the block storage disk this suite snapshots, in GB.
const DISK_SIZE_IN_GB: i64 = 8;

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One way to mint a snapshot name: an operation of this service that persists a
/// new snapshot under a name its request chose.
///
/// The rows are every such operation, over everything the rule generalises
/// over: which operation mints the name, which KIND of snapshot it mints, and —
/// for the create that takes either — which source it is taken of. A rename is
/// not one of them: no operation renames a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mint {
    /// `CreateDiskSnapshot` of a block storage disk.
    DiskSnapshotOfADisk,
    /// `CreateDiskSnapshot` of an instance's system volume — the other source
    /// the same operation accepts.
    DiskSnapshotOfASystemVolume,
    /// `CreateInstanceSnapshot` of an instance.
    InstanceSnapshotOfAnInstance,
    /// `CopySnapshot` of a settled disk snapshot.
    CopyOfADiskSnapshot,
    /// `CopySnapshot` of a settled instance snapshot.
    CopyOfAnInstanceSnapshot,
}

/// Every minting operation of this service.
const MINTS: &[Mint] = &[
    Mint::DiskSnapshotOfADisk,
    Mint::DiskSnapshotOfASystemVolume,
    Mint::InstanceSnapshotOfAnInstance,
    Mint::CopyOfADiskSnapshot,
    Mint::CopyOfAnInstanceSnapshot,
];

impl Mint {
    /// The kind of snapshot this mint persists.
    fn kind(self) -> Kind {
        match self {
            Self::DiskSnapshotOfADisk
            | Self::DiskSnapshotOfASystemVolume
            | Self::CopyOfADiskSnapshot => Kind::DiskSnapshot,
            Self::InstanceSnapshotOfAnInstance | Self::CopyOfAnInstanceSnapshot => {
                Kind::InstanceSnapshot
            }
        }
    }

    /// The call that mints the snapshot name `name`: the operation and its body.
    fn request(self, name: &str) -> (&'static str, Value) {
        match self {
            Self::DiskSnapshotOfADisk => (
                "CreateDiskSnapshot",
                json!({"diskName": DISK, "diskSnapshotName": name}),
            ),
            Self::DiskSnapshotOfASystemVolume => (
                "CreateDiskSnapshot",
                json!({"instanceName": INSTANCE, "diskSnapshotName": name}),
            ),
            Self::InstanceSnapshotOfAnInstance => (
                "CreateInstanceSnapshot",
                json!({"instanceName": INSTANCE, "instanceSnapshotName": name}),
            ),
            Self::CopyOfADiskSnapshot => (
                "CopySnapshot",
                json!({
                    "sourceSnapshotName": SOURCE_DISK_SNAPSHOT,
                    "sourceRegion": REGION,
                    "targetSnapshotName": name,
                }),
            ),
            Self::CopyOfAnInstanceSnapshot => (
                "CopySnapshot",
                json!({
                    "sourceSnapshotName": SOURCE_INSTANCE_SNAPSHOT,
                    "sourceRegion": REGION,
                    "targetSnapshotName": name,
                }),
            ),
        }
    }
}

/// The name one case of the table works with, in the form that case's index
/// calls for: hyphens, underscores, mixed case and the shortest single word —
/// every one inside the modelled `ResourceName` pattern `^\w[\w\-]*\w$`
/// (`smithy_query(query="shape",shape="ResourceName")`).
fn name_at(at: usize) -> String {
    match at % 4 {
        0 => format!("taken-name-{at}"),
        1 => format!("taken_name_{at}"),
        2 => format!("TakenName{at}"),
        _ => format!("t{at}"),
    }
}

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// Stand up what every minting operation needs: the instance and the disk the
/// creates snapshot, and one settled snapshot of each kind for the copies.
///
/// A copy's source is served only once the service has finished taking it, so
/// each source is waited into its settled state here — the transition the clock
/// owes it, not one a reader forces.
async fn fixture(emulator: &Emulator) {
    create_running_instance(emulator, INSTANCE, ZONE).await;
    create_available_disk(emulator, DISK, ZONE, DISK_SIZE_IN_GB).await;
    emulator
        .ok(
            "CreateDiskSnapshot",
            json!({"diskName": DISK, "diskSnapshotName": SOURCE_DISK_SNAPSHOT}),
        )
        .await;
    emulator
        .ok(
            "CreateInstanceSnapshot",
            json!({"instanceName": INSTANCE, "instanceSnapshotName": SOURCE_INSTANCE_SNAPSHOT}),
        )
        .await;
    for (kind, name) in [
        (Kind::DiskSnapshot, SOURCE_DISK_SNAPSHOT),
        (Kind::InstanceSnapshot, SOURCE_INSTANCE_SNAPSHOT),
    ] {
        wait_for_snapshot_state(emulator, kind, name, kind.settled()).await;
    }
}

/// Assert `name` is held by a snapshot of `kind` and by NOTHING else: the other
/// kind's reader does not serve it, and the unqualified `resourceName` a tagging
/// call resolves reaches that kind.
async fn assert_one_snapshot_holds(emulator: &Emulator, case: &str, kind: Kind, name: &str) {
    let other = match kind {
        Kind::DiskSnapshot => Kind::InstanceSnapshot,
        Kind::InstanceSnapshot => Kind::DiskSnapshot,
    };
    let (operation, member) = other.reader();
    let (status, answered) = emulator.call(operation, json!({member: name})).await;
    assert_eq!(
        status, 400,
        "case {case}: expected no {} of this name, got {status} {answered:?}",
        other.resource_type()
    );

    let tagged = emulator
        .ok(
            "TagResource",
            json!({"resourceName": name, "tags": [{"key": "namespace", "value": case}]}),
        )
        .await;
    let operations = tagged["operations"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        operations.len(),
        1,
        "case {case}: one tagging call changes one resource, got {operations:?}"
    );
    assert_eq!(
        operations[0]["resourceType"].as_str(),
        Some(kind.resource_type()),
        "case {case}: expected the tagging call to resolve the {} that holds the name, got {:?}",
        kind.resource_type(),
        operations[0]
    );
    let held = snapshot(emulator, kind, name).await;
    let tags = held["tags"].as_array().cloned().unwrap_or_default();
    assert!(
        tags.iter()
            .any(|tag| tag["key"].as_str() == Some("namespace")),
        "case {case}: expected the tag to reach the snapshot that holds the name, got {tags:?}"
    );
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: a snapshot name addresses one snapshot, so a name any snapshot of the
/// account already holds is refused by every operation that mints a snapshot
/// name — across the two kinds as much as within one — with the modelled
/// `InvalidInputException`, and the refusal leaves the snapshot that holds the
/// name exactly as it was.
#[tokio::test]
async fn a_name_a_snapshot_already_holds_cannot_be_minted_again() {
    let emulator = Emulator::new();
    fixture(&emulator).await;

    let mut at = 0usize;
    for holder in MINTS {
        for taker in MINTS {
            let name = name_at(at);
            at += 1;
            let case =
                format!("{holder:?} holds '{name}', {taker:?} tries to mint the same name again");

            // The name is taken by the holder, and reads back as the kind the
            // holder mints.
            let (operation, body) = holder.request(&name);
            emulator.ok(operation, body).await;
            let held = snapshot(&emulator, holder.kind(), &name).await;
            let held_arn = held["arn"].as_str().unwrap_or_default().to_string();
            assert!(
                !held_arn.is_empty(),
                "case {case}: expected the held snapshot to report its ARN, got {held:?}"
            );

            // The second mint of that very name is refused.
            let (operation, body) = taker.request(&name);
            let (status, answered) = emulator.call(operation, body).await;
            assert_eq!(
                status, 400,
                "case {case}: expected a 400 refusal, got {status} {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                "InvalidInputException",
                "case {case}: expected the modelled InvalidInputException, got {answered:?}"
            );
            let message = answered["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(&name),
                "case {case}: expected a message naming the snapshot name, got {message:?}"
            );

            // The refusal stored nothing: the name is still held by the one
            // snapshot that held it, identity and size unchanged.
            let after = snapshot(&emulator, holder.kind(), &name).await;
            assert_eq!(
                after["arn"].as_str(),
                Some(held_arn.as_str()),
                "case {case}: expected the held snapshot's own ARN, got {after:?}"
            );
            assert_eq!(
                after["sizeInGb"], held["sizeInGb"],
                "case {case}: expected the held snapshot's own size, got {after:?}"
            );
            assert_eq!(
                after["createdAt"], held["createdAt"],
                "case {case}: expected the held snapshot's own creation time, got {after:?}"
            );
            assert_one_snapshot_holds(&emulator, &case, holder.kind(), &name).await;
        }
    }
}

/// RULE: a snapshot name is taken unqualified wherever this service takes one,
/// so it resolves to the one kind of snapshot that holds it — which is what the
/// namespace being single-valued buys: an export of that name exports the
/// snapshot that holds it, whichever kind that is.
#[tokio::test]
async fn an_unqualified_snapshot_name_exports_the_snapshot_that_holds_it() {
    let emulator = Emulator::new();
    fixture(&emulator).await;

    for (at, mint) in MINTS.iter().enumerate() {
        let name = format!("exported-{at}");
        let case = format!("{mint:?} holds '{name}'");
        let (operation, body) = mint.request(&name);
        emulator.ok(operation, body).await;
        // An export reads the complete copy the snapshot holds, so it is served
        // only once the service has finished taking it — the transition the
        // clock owes it (`export_snapshot_completeness.rs`).
        let kind = mint.kind();
        wait_for_snapshot_state(&emulator, kind, &name, kind.settled()).await;
        emulator
            .ok("ExportSnapshot", json!({"sourceSnapshotName": name}))
            .await;
        let records = emulator.ok("GetExportSnapshotRecords", json!({})).await["exportSnapshotRecords"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let record = records
            .iter()
            .find(|record| record["sourceInfo"]["name"].as_str() == Some(name.as_str()))
            .unwrap_or_else(|| {
                panic!("case {case}: expected an export record of '{name}', got {records:?}")
            });
        assert_eq!(
            record["sourceInfo"]["resourceType"].as_str(),
            Some(mint.kind().resource_type()),
            "case {case}: expected the export to describe the kind that holds the name, got {:?}",
            record["sourceInfo"]
        );
    }
}

/// RULE: the name is reserved by the snapshot that holds it and by nothing else,
/// so a name the account no longer holds is free again — for the OTHER kind as
/// much as for the one that held it — while two snapshots of different names
/// coexist whatever kinds they are.
#[tokio::test]
async fn a_name_a_deleted_snapshot_held_is_free_for_either_kind() {
    let emulator = Emulator::new();
    fixture(&emulator).await;
    let case = "a disk snapshot's name handed over to an instance snapshot";
    let name = "handed-over-name";

    // The two fixture snapshots, of different names and different kinds, coexist
    // — the namespace refuses a repeated NAME, not a second snapshot.
    for (kind, fixture_name) in [
        (Kind::DiskSnapshot, SOURCE_DISK_SNAPSHOT),
        (Kind::InstanceSnapshot, SOURCE_INSTANCE_SNAPSHOT),
    ] {
        let reported = snapshot(&emulator, kind, fixture_name).await;
        assert_eq!(
            reported["name"].as_str(),
            Some(fixture_name),
            "case {case}: expected the {} of its own name, got {reported:?}",
            kind.resource_type()
        );
    }

    emulator
        .ok(
            "CreateDiskSnapshot",
            json!({"diskName": DISK, "diskSnapshotName": name}),
        )
        .await;
    let (status, answered) = emulator
        .call(
            "CreateInstanceSnapshot",
            json!({"instanceName": INSTANCE, "instanceSnapshotName": name}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case}: expected the held name to be refused, got {status} {answered:?}"
    );

    emulator
        .ok("DeleteDiskSnapshot", json!({"diskSnapshotName": name}))
        .await;
    emulator
        .ok(
            "CreateInstanceSnapshot",
            json!({"instanceName": INSTANCE, "instanceSnapshotName": name}),
        )
        .await;
    let held = snapshot(&emulator, Kind::InstanceSnapshot, name).await;
    assert_eq!(
        held["fromInstanceName"].as_str(),
        Some(INSTANCE),
        "case {case}: expected a snapshot of the instance under the freed name, got {held:?}"
    );
    assert_one_snapshot_holds(&emulator, case, Kind::InstanceSnapshot, name).await;
}
