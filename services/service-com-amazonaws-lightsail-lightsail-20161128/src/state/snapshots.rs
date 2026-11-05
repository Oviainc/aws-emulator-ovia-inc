// SPDX-License-Identifier: CC0-1.0

//! Instance snapshots, the automatic snapshots the add-on takes, and the records
//! an export of one of them to Amazon EC2 leaves behind.

use super::*;

/// One stored instance snapshot: every member `GetInstanceSnapshot` and
/// `GetInstanceSnapshots` report, plus the clock-owed transition its state is
/// waiting on.
///
/// A snapshot is a point-in-time copy, so everything it says about its SOURCE is
/// copied out of the instance record at the instant the snapshot was taken and
/// never followed afterwards: the snapshot keeps reporting the blueprint, bundle,
/// system disk size and attached block storage disks the instance had then, even
/// after the instance is changed or deleted.
#[derive(Debug, Clone)]
pub struct StoredInstanceSnapshot {
    /// The snapshot name the request named, kept verbatim — it is this record's
    /// store key and the `name` every read reports.
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    pub location: StoredLocation,
    pub tags: Vec<Tag>,
    /// The size of the snapshotted system volume in GB, copied from the source
    /// instance's bundle — "The size in GB of the SSD".
    pub size_in_gb: i32,
    /// The current `InstanceSnapshotState` value:
    /// [`INSTANCE_SNAPSHOT_STATE_PENDING`] or
    /// [`INSTANCE_SNAPSHOT_STATE_AVAILABLE`].
    pub state: &'static str,
    /// The state the clock owes this snapshot, while one is outstanding.
    pub due: Option<DueNamedState>,
    /// The name of the instance the snapshot was taken of.
    pub from_instance_name: String,
    /// That instance's ARN.
    pub from_instance_arn: String,
    /// That instance's blueprint id.
    pub from_blueprint_id: String,
    /// That instance's bundle id.
    pub from_bundle_id: String,
    /// Every block storage disk attached to the source instance at the instant
    /// the snapshot was taken, copied whole: `InstanceSnapshot$fromAttachedDisks`
    /// is "an array of disk objects containing information about all block
    /// storage disks", and a disk later detached from the instance was still
    /// part of what this snapshot captured.
    pub from_attached_disks: Vec<StoredDisk>,
    /// Whether this snapshot was created from an AUTOMATIC snapshot, which is
    /// what `InstanceSnapshot$isFromAutoSnapshot` reports: false for a snapshot
    /// the caller asked the service to take, true for one copied from a copy the
    /// `AutoSnapshot` add-on took.
    pub is_from_auto_snapshot: bool,
}

impl StoredInstanceSnapshot {
    /// This record as the modelled `InstanceSnapshot`.
    pub fn view(&self) -> crate::ops::types::InstanceSnapshot {
        crate::ops::types::InstanceSnapshot {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            support_code: Some(self.support_code.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_INSTANCE_SNAPSHOT.to_string()),
            tags: Some(self.tags.clone()),
            state: Some(self.state.to_string()),
            // The modelled `InstanceSnapshot$progress` documentation says this
            // member "is populated only for disk snapshots, and is null for
            // instance snapshots", so an instance snapshot reports none.
            progress: None,
            from_attached_disks: Some(
                self.from_attached_disks
                    .iter()
                    // What this snapshot CAPTURED of each disk, as it was then.
                    // When a further automatic copy of one of those disks falls
                    // due is a live fact about the disk itself — the schedule its
                    // own add-on runs on — and not something a capture taken in
                    // the past can report, so the captured add-on carries no next
                    // daily time.
                    .map(|disk| disk.view(None))
                    .collect(),
            ),
            from_instance_name: Some(self.from_instance_name.clone()),
            from_instance_arn: Some(self.from_instance_arn.clone()),
            from_blueprint_id: Some(self.from_blueprint_id.clone()),
            from_bundle_id: Some(self.from_bundle_id.clone()),
            is_from_auto_snapshot: Some(self.is_from_auto_snapshot),
            size_in_gb: Some(self.size_in_gb),
        }
    }
}

// ---------------------------------------------------------------------------
// Automatic snapshots
//
// An AUTOMATIC snapshot is a copy the `AutoSnapshot` add-on takes of the
// resource it is enabled for, once a day at the hour that add-on carries. It is
// not a resource of its own: it has no name, no ARN and no tags, it is addressed
// by the resource it was taken of plus the DATE it was taken on
// (`GetAutoSnapshots`, `DeleteAutoSnapshot$date`), and nothing but the clock
// creates one. So the copies are held beside the schedule that takes them,
// keyed by the resource, rather than in a map of named records like the
// snapshots a caller asks for.
// ---------------------------------------------------------------------------

/// One automatic snapshot the `AutoSnapshot` add-on took.
#[derive(Debug, Clone)]
pub struct StoredAutoSnapshot {
    /// The UTC date the copy was taken on, in the `YYYY-MM-DD` form
    /// `AutoSnapshotDetails$date` reports and `DeleteAutoSnapshot$date`
    /// addresses it by. One copy a day, so this identifies it among the
    /// resource's copies.
    pub date: String,
    /// The instant the service started taking the copy, which
    /// `AutoSnapshotDetails$createdAt` reports.
    pub created_at_ms: i64,
    /// The current `AutoSnapshotStatus` value:
    /// [`AUTO_SNAPSHOT_STATUS_IN_PROGRESS`] or
    /// [`AUTO_SNAPSHOT_STATUS_SUCCESS`].
    pub status: &'static str,
    /// The status the clock owes this copy, while one is outstanding.
    pub due: Option<DueNamedState>,
    /// The block storage disks attached to the INSTANCE this copy was taken of
    /// at the instant it was taken, captured whole — `CopySnapshot` turns one of
    /// these copies into a manual instance snapshot, which describes the disks it
    /// captured as the resources they are. `AutoSnapshotDetails$fromAttachedDisks`
    /// reports them as "the block storage disks attached to the instance when the
    /// automatic snapshot was created": an empty list for an instance that had
    /// none, and `None` for a copy of a DISK, which is one volume and has no
    /// disks attached to it.
    pub from_attached_disks: Option<Vec<StoredDisk>>,
}

impl StoredAutoSnapshot {
    /// This record as the modelled `AutoSnapshotDetails`.
    pub fn view(&self) -> AutoSnapshotDetails {
        AutoSnapshotDetails {
            date: Some(self.date.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            status: Some(self.status.to_string()),
            // What the modelled `AttachedDisk` carries of each captured disk:
            // the device path it was mounted at and its size.
            from_attached_disks: self.from_attached_disks.as_ref().map(|disks| {
                disks
                    .iter()
                    .map(|disk| AttachedDisk {
                        path: disk.path.clone(),
                        size_in_gb: Some(disk.size_in_gb),
                    })
                    .collect()
            }),
        }
    }
}

/// What a delete recorded about the resource a set of automatic snapshots was
/// taken of, once that resource's own record is gone.
///
/// A delete that is not asked to delete the resource's add-ons leaves the copies
/// the add-on took in the account, and the record that said where that resource
/// was is no longer there to ask — so what a later call still needs of it is
/// recorded here.
#[derive(Debug, Clone)]
pub struct KeptAutoSnapshotSource {
    /// The instant the resource's record was removed, which is what orders one
    /// account's kept copies against [`MAX_KEPT_AUTO_SNAPSHOT_SOURCES`].
    pub removed_at_ms: i64,
    /// Where that resource was: what an operation logged against one of these
    /// copies is located at, exactly as it was while the resource was held.
    pub location: StoredLocation,
}

/// What the `AutoSnapshot` add-on of one resource has taken, and when it next
/// takes a copy.
#[derive(Debug, Default)]
pub struct StoredAutoSnapshots {
    /// The instant the next automatic snapshot of the resource is due, while
    /// the add-on is enabled on it. `None` once the add-on is disabled: the
    /// copies already taken stay, and no further one is owed.
    pub due_at_ms: Option<i64>,
    /// The copies taken so far, oldest first — the order they were taken, and
    /// the order `GetAutoSnapshots` reports them in. At most
    /// [`MAX_AUTO_SNAPSHOTS_PER_RESOURCE`] of them.
    pub taken: Vec<StoredAutoSnapshot>,
    /// Set once the resource these copies were taken of has been deleted and the
    /// copies were KEPT, which is what a delete that was not asked to delete the
    /// resource's add-ons does. `None` for as long as this account holds that
    /// resource, whose own record is then what says where it is.
    pub kept_source: Option<KeptAutoSnapshotSource>,
}

impl StoredAutoSnapshots {
    /// Record the copy the schedule owed at `at_ms`, capturing
    /// `from_attached_disks`.
    ///
    /// The copy is taken `InProgress` and the CLOCK settles it into `Success`:
    /// the modelled `AddOn$snapshotTimeOfDay` documentation says the snapshot
    /// "is automatically created between the time shown and up to 45 minutes
    /// after", so there is an interval during which the service holds a copy it
    /// has not finished taking.
    ///
    /// One copy a day: an automatic snapshot is addressed by its date, so a date
    /// this resource already holds a copy for is not copied a second time.
    pub(super) fn take(&mut self, at_ms: i64, from_attached_disks: Option<Vec<StoredDisk>>) {
        let date = utc_date(at_ms);
        if self.taken.iter().any(|taken| taken.date == date) {
            return;
        }
        self.taken.push(StoredAutoSnapshot {
            date,
            created_at_ms: at_ms,
            status: AUTO_SNAPSHOT_STATUS_IN_PROGRESS,
            due: Some(DueNamedState::after(at_ms, AUTO_SNAPSHOT_STATUS_SUCCESS)),
            from_attached_disks,
        });
        // The oldest copy drops out once a new one would take the resource past
        // what this store holds of it.
        while self.taken.len() > MAX_AUTO_SNAPSHOTS_PER_RESOURCE {
            self.taken.remove(0);
        }
    }

    /// Every copy as the modelled `AutoSnapshotDetails`, oldest first.
    pub(super) fn view(&self) -> Vec<AutoSnapshotDetails> {
        self.taken.iter().map(StoredAutoSnapshot::view).collect()
    }
}

/// What a request to delete ONE automatic snapshot of a resource found.
///
/// A deletion addresses a copy by the resource it was taken of plus the date it
/// was taken on, so there are three outcomes and the caller answers each
/// differently: no such resource (the absent value of the
/// [`Option`](ComAmazonawsLightsailLightsail20161128State::delete_auto_snapshot)
/// this is wrapped in), a resource holding no copy for that date, and the copy
/// removed.
#[derive(Debug)]
pub enum AutoSnapshotDeletion {
    /// The copy was removed. Carries what an operation-log entry for the
    /// deletion needs: the kind of resource the copy was taken of, and where
    /// that resource is.
    Deleted {
        /// The `ResourceType` the resource the copy was taken of reports.
        resource_type: &'static str,
        /// Where that resource is.
        location: StoredLocation,
    },
    /// The resource exists and holds no copy taken on the requested date.
    NoCopyForDate {
        /// The `ResourceType` that resource reports, for the refusal's message.
        resource_type: &'static str,
    },
}

// ---------------------------------------------------------------------------
// Export snapshot records
// ---------------------------------------------------------------------------

/// The prefix a minted export snapshot record name carries.
///
/// CHOSEN: Lightsail does not document how it names an export snapshot record,
/// only that the record HAS a name the caller never supplied — the modelled
/// `ExportSnapshot` input takes nothing but the snapshot to export, while
/// `ExportSnapshotRecord$name` targets `ResourceName` (pattern
/// `^\w[\w\-]*\w$`). So the name is minted here: this prefix, which says which
/// operation created the record, followed by the id this store allocated for the
/// record — the same id its ARN carries, so one value identifies it everywhere.
pub const EXPORT_SNAPSHOT_RECORD_NAME_PREFIX: &str = "ExportSnapshot-";

/// The id prefix of the Amazon EC2 resource an exported INSTANCE snapshot
/// becomes, from the modelled `ExportSnapshot` documentation: "Exported instance
/// snapshots appear in Amazon EC2 as Amazon Machine Images (AMIs)". See
/// [`ec2_destination_id`].
pub const EC2_IMAGE_ID_PREFIX: &str = "ami-";

/// The id prefix of the Amazon EC2 resource an exported DISK snapshot becomes,
/// from the same modelled documentation: "Exported disk snapshots appear in
/// Amazon EC2 as Amazon EBS volumes". See [`ec2_destination_id`].
pub const EC2_VOLUME_ID_PREFIX: &str = "vol-";

/// How many hexadecimal characters an EC2 resource id carries after its prefix:
/// the 17 of the long-form id format every EC2 resource created since 2018 uses
/// (`vol-0123456789abcdef0`), which is the form `DestinationInfo$id` reports for
/// a resource created in EC2.
pub(super) const EC2_RESOURCE_ID_HEX_LEN: usize = 17;

/// The destination service an export snapshot record reports, as
/// `DestinationInfo$service` ("The destination service of the record").
///
/// `ExportSnapshot` exports to Amazon EC2 and to nothing else, so the one value
/// this member can carry is that service, named by its own ARN namespace and
/// endpoint prefix (`ec2`) — the identifier this emulator names a service with
/// everywhere else.
pub const EXPORT_DESTINATION_SERVICE: &str = "ec2";

/// The id of the Amazon EC2 resource an export creates: `prefix` (which kind of
/// resource the exported snapshot became) plus the long-form EC2 id's
/// [`EC2_RESOURCE_ID_HEX_LEN`] hexadecimal characters, taken from the store-minted
/// `id` so no two exports in this process name the same destination resource.
pub fn ec2_destination_id(prefix: &str, id: &str) -> String {
    let hex = id.replace('-', "");
    format!(
        "{prefix}{}",
        hex.get(..EC2_RESOURCE_ID_HEX_LEN).unwrap_or(hex.as_str())
    )
}

/// The ARN of the export snapshot record `id` in `region`.
pub fn export_snapshot_record_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{EXPORT_SNAPSHOT_RECORD_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// One stored export snapshot record: the record `ExportSnapshot` created, with
/// the clock-owed transition its state is waiting on.
///
/// Every `ExportSnapshot` call appends ONE record — exporting the same snapshot
/// twice is two exports, and so two records — which is why these are keyed by a
/// minted name rather than by the snapshot's.
#[derive(Debug, Clone)]
pub struct StoredExportSnapshotRecord {
    /// The record name this store minted — this record's store key and the
    /// `name` every read reports. See [`EXPORT_SNAPSHOT_RECORD_NAME_PREFIX`].
    pub name: String,
    pub arn: String,
    pub created_at_ms: i64,
    /// Where the record is: the location of the snapshot that was exported,
    /// because "Snapshots are exported to the same Amazon Web Services Region in
    /// Amazon EC2 as the source Lightsail snapshot".
    pub location: StoredLocation,
    /// The current `RecordState` value: [`RECORD_STATE_STARTED`] or
    /// [`RECORD_STATE_SUCCEEDED`].
    pub state: &'static str,
    /// The state the clock owes this record, while one is outstanding.
    pub due: Option<DueNamedState>,
    /// The snapshot this export was taken of, captured at the instant of the
    /// export — so the record keeps describing it even if the snapshot itself
    /// later changes or is deleted.
    pub source_info: ExportSnapshotRecordSourceInfo,
    /// The id of the Amazon EC2 resource this export creates. See
    /// [`ec2_destination_id`].
    pub destination_id: String,
}

impl StoredExportSnapshotRecord {
    /// Which KIND of snapshot this export was taken of, as the modelled
    /// `ExportSnapshotRecordSourceType` names it: [`RESOURCE_TYPE_INSTANCE_SNAPSHOT`]
    /// or [`RESOURCE_TYPE_DISK_SNAPSHOT`].
    ///
    /// The distinction is what the export produced in Amazon EC2 — a machine
    /// image for an instance snapshot, a volume for a disk snapshot (see
    /// [`EC2_IMAGE_ID_PREFIX`] and [`EC2_VOLUME_ID_PREFIX`]) — so an operation
    /// that consumes the exported resource asks the record for it rather than
    /// re-deriving it.
    pub fn exported_snapshot_type(&self) -> &str {
        self.source_info.resource_type.as_deref().unwrap_or_default()
    }

    /// This record as the modelled `ExportSnapshotRecord`.
    pub fn view(&self) -> ExportSnapshotRecord {
        ExportSnapshotRecord {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_EXPORT_SNAPSHOT_RECORD.to_string()),
            state: Some(self.state.to_string()),
            source_info: Some(self.source_info.clone()),
            // `DestinationInfo$id` is "The ID of the resource created at the
            // destination", and while the export is still `Started` nothing has
            // been created there yet — so the destination is reported from the
            // instant the export succeeded, not before.
            destination_info: (self.state == RECORD_STATE_SUCCEEDED).then(|| DestinationInfo {
                id: Some(self.destination_id.clone()),
                service: Some(EXPORT_DESTINATION_SERVICE.to_string()),
            }),
        }
    }
}
