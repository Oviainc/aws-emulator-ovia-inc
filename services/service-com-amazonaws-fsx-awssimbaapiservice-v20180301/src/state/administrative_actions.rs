// SPDX-License-Identifier: CC0-1.0

//! The administrative-action slice of this service's state: the asynchronous work an
//! `UpdateFileSystem` STARTS, and the storage-capacity target rule that decides whether it
//! may start at all.
//!
//! AWS does not carry out a storage-capacity target in line. `UpdateFileSystem` answers with
//! the file system as it still stands and records an `AdministrativeAction` of type
//! `FILE_SYSTEM_UPDATE` — "a file system update administrative action initiated from the
//! Amazon FSx console, API (`UpdateFileSystem`), or CLI" — whose `TargetFileSystemValues`
//! carry the target the request asked for; the reported `StorageCapacity` changes when that
//! action reaches `COMPLETED` ("Amazon FSx has finished processing the administrative
//! task"). So the action is a record of its own here, the target lives on it rather than on
//! the file system, and the clock owes its completion.
//!
//! The target itself is bounded by what AWS documents per file-system type on
//! `UpdateFileSystemRequest$StorageCapacity`, which [`storage_capacity_refusal`] states once
//! for every type: a target that does not clear its type's floor names no capacity the
//! service can move to, so it is refused before anything is written and no action is
//! recorded for it.

use std::time::{Instant, SystemTime};

use crate::ops::types::{AdministrativeAction, FileSystem};

use super::{
    epoch_date_time, within_error_message_length, FileSystemRecord, LIFECYCLE_TRANSITION,
};

/// The `AdministrativeActionType` an `UpdateFileSystem` starts, from the modelled enum
/// (`smithy_query(query="shape", shape="AdministrativeActionType")`): "`FILE_SYSTEM_UPDATE`
/// - A file system update administrative action initiated from the Amazon FSx console, API
/// (`UpdateFileSystem`), or CLI (`update-file-system`)".
pub const ACTION_TYPE_FILE_SYSTEM_UPDATE: &str = "FILE_SYSTEM_UPDATE";

/// The `Status` an administrative action the clock still owes reports, from the modelled
/// enum (`smithy_query(query="shape", shape="Status")`: FAILED | IN_PROGRESS | PENDING |
/// COMPLETED | UPDATED_OPTIMIZING | OPTIMIZING | PAUSED | CANCELLED): "`PENDING` - Amazon
/// FSx is waiting to process the administrative action".
pub const ACTION_STATUS_PENDING: &str = "PENDING";

/// The `Status` a settled administrative action reports: "`COMPLETED` - Amazon FSx has
/// finished processing the administrative task". See [`ACTION_STATUS_PENDING`].
pub const ACTION_STATUS_COMPLETED: &str = "COMPLETED";

/// The most administrative actions one file system's record keeps.
///
/// AWS publishes no bound on the history a file system reports, but a caller grows this list
/// by one entry per accepted update, so it is bounded here and trimmed oldest-first: the
/// actions that matter to a caller polling a resize are the recent ones, and an action that
/// has already completed has nothing left to report. CHOSEN for boundedness, not documented.
pub const MAX_ADMINISTRATIVE_ACTIONS_PER_FILE_SYSTEM: usize = 32;

/// The `FileSystemType`s this rule distinguishes, from the modelled `FileSystemType` enum
/// (`smithy_query(query="shape", shape="FileSystemType")`: LUSTRE | ONTAP | OPENZFS |
/// WINDOWS). Only Lustre and ONTAP have a rule of their own below; the others are the
/// documented "at least 10 percent greater".
pub const FILE_SYSTEM_TYPE_LUSTRE: &str = "LUSTRE";
/// See [`FILE_SYSTEM_TYPE_LUSTRE`].
pub const FILE_SYSTEM_TYPE_ONTAP: &str = "ONTAP";

/// The `LustreDeploymentType` a Lustre file system has when its create named none, from
/// `LustreFileSystemConfiguration$DeploymentType` ("The default is SCRATCH_1").
pub const DEFAULT_LUSTRE_DEPLOYMENT_TYPE: &str = "SCRATCH_1";

/// The `LustreDeploymentType` whose storage capacity cannot be increased at all, from
/// `UpdateFileSystemRequest$StorageCapacity`: "For `SCRATCH_1` file systems, you can't
/// increase the storage capacity."
pub const LUSTRE_DEPLOYMENT_SCRATCH_1: &str = "SCRATCH_1";

/// The `LustreDeploymentType`s whose SSD capacity targets are multiples of
/// [`LUSTRE_SSD_CAPACITY_MULTIPLE`], from `UpdateFileSystemRequest$StorageCapacity`: "For
/// `SCRATCH_2`, `PERSISTENT_1`, and `PERSISTENT_2 SSD` deployment types, valid values are in
/// multiples of 2400 GiB."
pub const LUSTRE_SSD_MULTIPLE_DEPLOYMENT_TYPES: &[&str] =
    &["SCRATCH_2", "PERSISTENT_1", "PERSISTENT_2"];

/// See [`LUSTRE_SSD_MULTIPLE_DEPLOYMENT_TYPES`].
pub const LUSTRE_SSD_CAPACITY_MULTIPLE: i32 = 2400;

/// The `StorageType` whose Lustre capacity multiples are the per-throughput HDD ones, from
/// the modelled `StorageType` enum (HDD | INTELLIGENT_TIERING | SSD).
pub const STORAGE_TYPE_HDD: &str = "HDD";

/// The `PerUnitStorageThroughput` of a `PERSISTENT HDD` Lustre file system and the capacity
/// multiple that goes with it, from `UpdateFileSystemRequest$StorageCapacity`: "For
/// `PERSISTENT HDD` file systems, valid values are multiples of 6000 GiB for 12-MBps
/// throughput per TiB file systems and multiples of 1800 GiB for 40-MBps throughput per TiB
/// file systems."
pub const LUSTRE_HDD_CAPACITY_MULTIPLES: &[(i32, i32)] = &[(12, 6000), (40, 1800)];

/// Why a storage-capacity target names no capacity the file system can move to.
///
/// One variant per documented rule on `UpdateFileSystemRequest$StorageCapacity`, so the
/// refusal a caller reads states the rule its request broke rather than one generic
/// sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageCapacityRefusal {
    /// An increase that does not clear the documented 10-percent floor (OpenZFS, Windows,
    /// and an ONTAP increase), which a decrease and an equal target also fail.
    BelowTenPercentFloor { requested: i32, current: i32 },
    /// An ONTAP decrease that is not at least 9 percent smaller than the current capacity.
    AboveNinePercentFloor { requested: i32, current: i32 },
    /// A Lustre target that is not greater than the current capacity.
    NotGreaterThanCurrent { requested: i32, current: i32 },
    /// A Lustre target that is not a multiple of the deployment type's documented step.
    NotAMultiple { requested: i32, multiple: i32 },
    /// A deployment type whose storage capacity cannot be increased at all.
    IncreaseUnsupported {
        requested: i32,
        deployment_type: String,
    },
    /// A capacity update is already in flight for this file system.
    UpdateInProgress { requested: i32 },
}

/// One recorded administrative action. Mirrors the modelled `AdministrativeAction` shape so
/// [`AdministrativeActionRecord::to_administrative_action`] is a projection and never a
/// second source of truth.
#[derive(Debug, Clone)]
pub struct AdministrativeActionRecord {
    /// The modelled `AdministrativeAction$AdministrativeActionType`.
    pub action_type: String,
    /// The modelled `AdministrativeAction$Status`.
    pub status: String,
    pub request_time: SystemTime,
    /// The `StorageCapacity` this action moves the file system to, which is what
    /// `TargetFileSystemValues` reports until the action completes and the file system
    /// itself reports it.
    pub target_storage_capacity: Option<i32>,
    /// When the clock owes this action its completion; `None` once it has completed. Never
    /// slept on — [`FileSystemRecord::settle_due_administrative_actions`] applies it on the
    /// first call at or after this instant.
    pub due: Option<Instant>,
}

impl AdministrativeActionRecord {
    /// The `FILE_SYSTEM_UPDATE` action an accepted storage-capacity target starts, waiting
    /// to be processed and due its completion a [`LIFECYCLE_TRANSITION`] from now.
    pub fn file_system_update(target_storage_capacity: i32) -> Self {
        Self {
            action_type: ACTION_TYPE_FILE_SYSTEM_UPDATE.to_string(),
            status: ACTION_STATUS_PENDING.to_string(),
            request_time: SystemTime::now(),
            target_storage_capacity: Some(target_storage_capacity),
            due: Some(Instant::now() + LIFECYCLE_TRANSITION),
        }
    }

    /// Whether this action is still work the clock owes.
    pub fn is_in_flight(&self) -> bool {
        self.due.is_some()
    }

    /// This record as the modelled `AdministrativeAction` shape.
    pub fn to_administrative_action(&self) -> AdministrativeAction {
        AdministrativeAction {
            administrative_action_type: Some(self.action_type.clone()),
            // "The percentage-complete status of a `STORAGE_OPTIMIZATION` or
            // `DOWNLOAD_DATA_FROM_BACKUP` administrative action. Does not apply to any other
            // administrative action type" (`AdministrativeAction$ProgressPercent`).
            progress_percent: None,
            request_time: Some(epoch_date_time(self.request_time)),
            status: Some(self.status.clone()),
            // "The target value for the administration action, provided in the
            // `UpdateFileSystem` operation. Returned for `FILE_SYSTEM_UPDATE` administrative
            // actions": the members this action moves, and no others.
            target_file_system_values: self.target_storage_capacity.map(|capacity| FileSystem {
                storage_capacity: Some(capacity),
                ..FileSystem::default()
            }),
            // Nothing failed, and this action is of a file system rather than of a volume or
            // a snapshot, so none of the remaining members has a value to report.
            failure_details: None,
            target_volume_values: None,
            target_snapshot_values: None,
            total_transfer_bytes: None,
            remaining_transfer_bytes: None,
            message: None,
        }
    }
}

impl FileSystemRecord {
    /// Record one administrative action on this file system, dropping the oldest once
    /// [`MAX_ADMINISTRATIVE_ACTIONS_PER_FILE_SYSTEM`] are held.
    ///
    /// The ONE write path for the list, so the bound cannot be bypassed by another operation
    /// that starts an action.
    pub fn record_administrative_action(&mut self, action: AdministrativeActionRecord) {
        self.administrative_actions.push(action);
        let held = self.administrative_actions.len();
        if held > MAX_ADMINISTRATIVE_ACTIONS_PER_FILE_SYSTEM {
            self.administrative_actions
                .drain(..held - MAX_ADMINISTRATIVE_ACTIONS_PER_FILE_SYSTEM);
        }
    }

    /// Whether a storage-capacity update is already in flight for this file system.
    ///
    /// "You can't make a storage capacity increase request if there is an existing storage
    /// capacity increase request in progress"
    /// (`UpdateFileSystemRequest$StorageCapacity`).
    pub fn storage_capacity_update_in_flight(&self) -> bool {
        self.administrative_actions
            .iter()
            .any(|action| action.is_in_flight() && action.target_storage_capacity.is_some())
    }

    /// Apply every administrative action the clock has already made due: the action reaches
    /// `COMPLETED` and the file system reports the capacity it targeted.
    ///
    /// This materialises a change that was owed at `now` whether or not anybody called, so a
    /// reporting operation may run it: it never changes what a later caller would otherwise
    /// have seen.
    pub fn settle_due_administrative_actions(&mut self, now: Instant) {
        let mut reached_capacity: Option<i32> = None;
        for action in &mut self.administrative_actions {
            let Some(due) = action.due else {
                continue;
            };
            if due > now {
                continue;
            }
            action.due = None;
            action.status = ACTION_STATUS_COMPLETED.to_string();
            if let Some(target) = action.target_storage_capacity {
                reached_capacity = Some(target);
            }
        }
        if let Some(capacity) = reached_capacity {
            self.storage_capacity = Some(capacity);
            // The provisioned SSD IOPS of an `AUTOMATIC` configuration are derived from the
            // capacity, so they move with it the moment the capacity does.
            self.materialize_provisioned_disk_iops();
        }
    }
}

/// Why `requested` is not a storage-capacity target this file system can move to, or `None`
/// when it is one.
///
/// THE rule, stated once for every file-system type from
/// `UpdateFileSystemRequest$StorageCapacity`: an OpenZFS, Windows or ONTAP increase "must be
/// at least 10 percent greater than the current storage capacity value", an ONTAP decrease
/// "must be at least 9 percent smaller than the current SSD storage capacity", and a Lustre
/// target "must be greater than the current storage capacity" and on its deployment type's
/// documented multiple. A decrease or an equal target therefore fails every type but a
/// second-generation ONTAP decrease, which is the band this states for ONTAP.
///
/// Mutates nothing: it is asked BEFORE anything is written, so a refused update leaves the
/// record exactly as it was.
pub fn storage_capacity_refusal(
    record: &FileSystemRecord,
    requested: i32,
) -> Option<StorageCapacityRefusal> {
    if record.storage_capacity_update_in_flight() {
        return Some(StorageCapacityRefusal::UpdateInProgress { requested });
    }
    // A record that reports no capacity at all (a Lustre create may state none) has no floor
    // to clear: there is no current value for the documented rules to be relative to.
    let current = record.storage_capacity?;
    if record.file_system_type == FILE_SYSTEM_TYPE_LUSTRE {
        return lustre_storage_capacity_refusal(record, current, requested);
    }
    // "When decreasing storage capacity on second-generation file systems, the target value
    // must be at least 9 percent smaller than the current SSD storage capacity."
    if record.file_system_type == FILE_SYSTEM_TYPE_ONTAP && requested < current {
        return (i64::from(requested) * 100 > i64::from(current) * 91).then_some(
            StorageCapacityRefusal::AboveNinePercentFloor { requested, current },
        );
    }
    // "The storage capacity target value must be at least 10 percent greater than the
    // current storage capacity value."
    (i64::from(requested) * 10 < i64::from(current) * 11)
        .then_some(StorageCapacityRefusal::BelowTenPercentFloor { requested, current })
}

/// The Lustre half of [`storage_capacity_refusal`]: a target greater than the current
/// capacity, on the multiple its deployment type and storage type document, and nothing at
/// all for `SCRATCH_1`.
fn lustre_storage_capacity_refusal(
    record: &FileSystemRecord,
    current: i32,
    requested: i32,
) -> Option<StorageCapacityRefusal> {
    let deployment_type = record
        .lustre_configuration
        .as_ref()
        .and_then(|configuration| configuration.deployment_type.as_deref())
        .unwrap_or(DEFAULT_LUSTRE_DEPLOYMENT_TYPE);
    if requested <= current {
        return Some(StorageCapacityRefusal::NotGreaterThanCurrent { requested, current });
    }
    if deployment_type == LUSTRE_DEPLOYMENT_SCRATCH_1 {
        return Some(StorageCapacityRefusal::IncreaseUnsupported {
            requested,
            deployment_type: deployment_type.to_string(),
        });
    }
    let multiple = lustre_capacity_multiple(record, deployment_type)?;
    (requested % multiple != 0).then_some(StorageCapacityRefusal::NotAMultiple {
        requested,
        multiple,
    })
}

/// The documented step a Lustre storage-capacity target must be a multiple of, or `None`
/// when the record's configuration names no deployment type (or throughput, for HDD) the
/// documentation states a step for — in which case only "greater than the current storage
/// capacity" is documented and that is all that is enforced.
fn lustre_capacity_multiple(record: &FileSystemRecord, deployment_type: &str) -> Option<i32> {
    if record.storage_type == STORAGE_TYPE_HDD {
        let per_unit = record
            .lustre_configuration
            .as_ref()
            .and_then(|configuration| configuration.per_unit_storage_throughput)?;
        return LUSTRE_HDD_CAPACITY_MULTIPLES
            .iter()
            .find(|(throughput, _)| *throughput == per_unit)
            .map(|(_, multiple)| *multiple);
    }
    LUSTRE_SSD_MULTIPLE_DEPLOYMENT_TYPES
        .contains(&deployment_type)
        .then_some(LUSTRE_SSD_CAPACITY_MULTIPLE)
}

/// The `BadRequest$Message` a refused storage-capacity target is answered with.
///
/// FSx opens every one of these with the target the request asked for and then states the
/// rule it broke, so the requested value and the file system's own stored capacity are
/// templated in from the request and from state; nothing here is a value from one run.
/// Clamped to the modelled `ErrorMessage` `length 1..256`.
pub fn storage_capacity_refusal_message(refusal: &StorageCapacityRefusal) -> String {
    let sentence = match refusal {
        StorageCapacityRefusal::BelowTenPercentFloor { requested, current } => format!(
            "Invalid desired storage capacity provided: {requested} GiB. Desired storage \
             capacity must be at least 10% higher than the existing value of {current} GiB."
        ),
        StorageCapacityRefusal::AboveNinePercentFloor { requested, current } => format!(
            "Invalid desired storage capacity provided: {requested} GiB. Desired storage \
             capacity must be at least 9% lower than the existing value of {current} GiB."
        ),
        StorageCapacityRefusal::NotGreaterThanCurrent { requested, current } => format!(
            "Invalid desired storage capacity provided: {requested} GiB. Desired storage \
             capacity must be greater than the existing value of {current} GiB."
        ),
        StorageCapacityRefusal::NotAMultiple {
            requested,
            multiple,
        } => format!(
            "Invalid desired storage capacity provided: {requested} GiB. Desired storage \
             capacity must be a multiple of {multiple} GiB."
        ),
        StorageCapacityRefusal::IncreaseUnsupported {
            requested,
            deployment_type,
        } => format!(
            "Invalid desired storage capacity provided: {requested} GiB. Storage capacity \
             cannot be increased for file systems with the {deployment_type} deployment type."
        ),
        StorageCapacityRefusal::UpdateInProgress { requested } => format!(
            "Invalid desired storage capacity provided: {requested} GiB. A storage capacity \
             update is already in progress for this file system."
        ),
    };
    within_error_message_length(sentence)
}
