// SPDX-License-Identifier: CC0-1.0

//! Block storage disks and the point-in-time disk snapshots taken of them.

use super::*;

/// The smallest block storage disk this service creates, in GB.
///
/// The size range Lightsail publishes for an additional block storage disk is
/// 8 GB to 16 TB; the modelled `CreateDisk$sizeInGb` carries no `@range`, so the
/// bound is enforced here rather than by the modelled-constraint walk.
pub const MIN_DISK_SIZE_IN_GB: i32 = 8;

/// The largest block storage disk this service creates, in GB: 16 TB, the upper
/// end of the same published range as [`MIN_DISK_SIZE_IN_GB`].
pub const MAX_DISK_SIZE_IN_GB: i32 = 16 * 1024;

/// IOPS per GB of disk, and the floor and ceiling that bracket the figure.
///
/// A Lightsail block storage disk is an EBS general purpose SSD (`gp2`) volume,
/// whose published baseline performance is 3 IOPS per GB, never below 100 IOPS
/// and never above 16,000.
pub(super) const DISK_IOPS_PER_GB: i32 = 3;
/// See [`DISK_IOPS_PER_GB`].
pub(super) const DISK_IOPS_FLOOR: i32 = 100;
/// See [`DISK_IOPS_PER_GB`].
pub(super) const DISK_IOPS_CEILING: i32 = 16_000;

/// The IOPS a block storage disk of `size_in_gb` provides, from the baseline in
/// [`DISK_IOPS_PER_GB`].
///
/// DERIVED from the disk's own size, so every read of one disk reports the same
/// figure and a larger disk reports more of them.
pub fn disk_iops(size_in_gb: i32) -> i32 {
    size_in_gb
        .saturating_mul(DISK_IOPS_PER_GB)
        .clamp(DISK_IOPS_FLOOR, DISK_IOPS_CEILING)
}

/// One stored block storage disk: every member `GetDisk` reports, plus the
/// clock-owed transition its state is waiting on.
#[derive(Debug, Clone)]
pub struct StoredDisk {
    /// The disk name the request named, kept verbatim — it is this record's
    /// store key and the `name` every read reports.
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    pub location: StoredLocation,
    pub tags: Vec<Tag>,
    pub add_ons: Vec<AddOn>,
    pub size_in_gb: i32,
    /// The current `DiskState` value: one of [`DISK_STATE_PENDING`],
    /// [`DISK_STATE_AVAILABLE`] or [`DISK_STATE_IN_USE`].
    pub state: &'static str,
    /// The state the clock owes this disk, while one is outstanding.
    ///
    /// A disk the clock owes [`DISK_STATE_AVAILABLE`] while it is still attached
    /// is one `DetachDisk` has started detaching: settling into `available` is
    /// what releases the attachment, because a disk attached to nothing is what
    /// `available` means (see [`ComAmazonawsLightsailLightsail20161128State::advance_clock`]).
    pub due: Option<DueNamedState>,
    /// The instance this disk is attached to, while one is. `AttachDisk` is what
    /// sets it; a newly created disk is attached to nothing.
    pub attached_to: Option<String>,
    /// The device path the disk is mounted at on the instance it is attached to.
    /// `AttachDisk` carries it (`diskPath`), so an unattached disk has none.
    pub path: Option<String>,
}

impl StoredDisk {
    /// This record as the modelled `Disk`.
    ///
    /// `next_auto_snapshot_time_of_day` is when the next automatic copy of this
    /// disk is due. It lives on the SCHEDULE the disk's add-on runs on rather
    /// than on this record, so the store passes it in
    /// ([`ComAmazonawsLightsailLightsail20161128State::next_auto_snapshot_time_of_day`]).
    pub fn view(&self, next_auto_snapshot_time_of_day: Option<String>) -> crate::ops::types::Disk {
        let attached = self.attached_to.is_some();
        crate::ops::types::Disk {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            support_code: Some(self.support_code.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_DISK.to_string()),
            tags: Some(self.tags.clone()),
            add_ons: Some(add_ons_view(&self.add_ons, next_auto_snapshot_time_of_day)),
            size_in_gb: Some(self.size_in_gb),
            // A disk `CreateDisk` created is additional block storage: the only
            // system disk is the one an instance's bundle provides, which is not
            // a `Disk` resource of its own.
            is_system_disk: Some(false),
            iops: Some(disk_iops(self.size_in_gb)),
            path: self.path.clone(),
            state: Some(self.state.to_string()),
            attached_to: self.attached_to.clone(),
            is_attached: Some(attached),
            // The discontinued member the modelled docs say is "still included
            // in the response" and that `isAttached` replaced, so it reports the
            // same fact in the older wording.
            attachment_state: Some(
                if attached {
                    "attached".to_string()
                } else {
                    "detached".to_string()
                },
            ),
            // Discontinued: the modelled `Disk$gbInUse` documentation says this
            // parameter "is now discontinued", so Lightsail reports none.
            gb_in_use: None,
            // The modelled `Disk$autoMountStatus` documentation says it "only
            // applies to Lightsail for Research resources"; a disk created
            // through `CreateDisk` is not one, so there is no mount to report.
            auto_mount_status: None,
        }
    }

    /// Record this disk as attached to the instance `instance_name` and exposed
    /// at the device `path`: the attachment and the path are what the modelled
    /// `Disk$attachedTo` and `$path` report, and the disk is [`DISK_STATE_IN_USE`]
    /// for as long as the attachment stands.
    ///
    /// A settle to [`DISK_STATE_AVAILABLE`] the clock still owed this disk is
    /// dropped here: settling there is what RELEASES an attachment (see
    /// [`ComAmazonawsLightsailLightsail20161128State::advance_clock`]), so
    /// leaving it in place would un-attach the disk this very call attached. A
    /// settle to any OTHER state is left alone — it is not this attachment's.
    /// Every operation that attaches an EXISTING disk goes through here, so the
    /// invariant lives next to the members it protects rather than in each
    /// handler. (A disk a restore CREATES already attached is stored attached,
    /// owed nothing, by the insert that creates it.)
    pub fn attach_to(&mut self, instance_name: String, path: String) {
        self.attached_to = Some(instance_name);
        self.path = Some(path);
        self.state = DISK_STATE_IN_USE;
        if self.due.is_some_and(|due| due.state == DISK_STATE_AVAILABLE) {
            self.due = None;
        }
    }
}

// ---------------------------------------------------------------------------
// Disk snapshots
// ---------------------------------------------------------------------------

/// The resource one disk snapshot was taken of: the modelled `DiskSnapshot`
/// reports the source's name AND its ARN, and a snapshot has exactly one source
/// — the modelled `CreateDiskSnapshot$diskName` and `$instanceName`
/// documentation says the two "are mutually exclusive" — so the pair is stored
/// as one value rather than as four independent members that could disagree.
#[derive(Debug, Clone)]
pub enum DiskSnapshotSource {
    /// A block storage disk, snapshotted through `CreateDiskSnapshot`'s
    /// `diskName`.
    Disk { name: String, arn: String },
    /// An instance's system volume, snapshotted through `instanceName`: "When
    /// this is defined, a snapshot of the instance's system volume is created."
    Instance { name: String, arn: String },
}

/// The `progress` a disk snapshot in `state` reports.
///
/// DERIVED from the snapshot's own state: a snapshot Lightsail is still taking
/// has nothing captured yet as far as this store knows, and one that completed
/// has captured all of the source volume. The percentage form is the one the
/// EC2 snapshot `Progress` this member mirrors uses (`"100%"`); the modelled
/// `DiskSnapshot$progress` documentation says only "The progress of the
/// snapshot", so the FORM is emulator-chosen while the fact it reports is the
/// store's own state.
pub(super) fn disk_snapshot_progress(state: &str) -> &'static str {
    if state == DISK_SNAPSHOT_STATE_COMPLETED {
        "100%"
    } else {
        "0%"
    }
}

/// One stored disk snapshot: every member `GetDiskSnapshot` reports, plus the
/// clock-owed transition its state is waiting on.
#[derive(Debug, Clone)]
pub struct StoredDiskSnapshot {
    /// The snapshot name the request named, kept verbatim — it is this record's
    /// store key and the `name` every read reports.
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    pub location: StoredLocation,
    pub tags: Vec<Tag>,
    /// The size of the snapshotted volume, copied from the source at the instant
    /// the snapshot was taken — the size a disk created from this snapshot
    /// starts at, so it does not follow a later change to the source.
    pub size_in_gb: i32,
    /// The current `DiskSnapshotState` value: [`DISK_SNAPSHOT_STATE_PENDING`] or
    /// [`DISK_SNAPSHOT_STATE_COMPLETED`].
    pub state: &'static str,
    /// The state the clock owes this snapshot, while one is outstanding.
    pub due: Option<DueNamedState>,
    /// What the snapshot was taken of.
    pub source: DiskSnapshotSource,
    /// Whether this snapshot was created from an AUTOMATIC snapshot, which is
    /// what `DiskSnapshot$isFromAutoSnapshot` reports: false for a snapshot the
    /// caller asked the service to take, true for one copied from a copy the
    /// `AutoSnapshot` add-on took.
    pub is_from_auto_snapshot: bool,
}

impl StoredDiskSnapshot {
    /// This record as the modelled `DiskSnapshot`.
    pub fn view(&self) -> crate::ops::types::DiskSnapshot {
        let (from_disk_name, from_disk_arn, from_instance_name, from_instance_arn) = match
            &self.source
        {
            DiskSnapshotSource::Disk { name, arn } => {
                (Some(name.clone()), Some(arn.clone()), None, None)
            }
            DiskSnapshotSource::Instance { name, arn } => {
                (None, None, Some(name.clone()), Some(arn.clone()))
            }
        };
        crate::ops::types::DiskSnapshot {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            support_code: Some(self.support_code.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_DISK_SNAPSHOT.to_string()),
            tags: Some(self.tags.clone()),
            size_in_gb: Some(self.size_in_gb),
            state: Some(self.state.to_string()),
            progress: Some(disk_snapshot_progress(self.state).to_string()),
            from_disk_name,
            from_disk_arn,
            from_instance_name,
            from_instance_arn,
            is_from_auto_snapshot: Some(self.is_from_auto_snapshot),
        }
    }
}
