// SPDX-License-Identifier: CC0-1.0

//! The volume slice of this service's state: the volumes a file system HAS, and what
//! `DescribeVolumes` reads.
//!
//! An FSx for OpenZFS file system is created WITH a volume: AWS documents
//! `OpenZFSFileSystemConfiguration$RootVolumeId` as "the ID of the root volume of the
//! OpenZFS file system" and takes its configuration on the create itself
//! (`CreateFileSystemOpenZFSConfiguration$RootVolumeConfiguration`). So the root volume is
//! a resource of its own, minted by the create that provisions the file system and stored
//! here, and the id the file system reports resolves to that stored record — never to a
//! value nothing holds. A volume belongs to its file system, so it is removed with it.
//!
//! The volumes `CreateVolume` mints are not here yet: that operation is still at 501, and
//! this slice holds exactly the volumes a file-system create provisions.

use std::collections::BTreeMap;
use std::time::{Instant, SystemTime};

use crate::ops::types::{
    OpenZFSCreateRootVolumeConfiguration, OpenZFSVolumeConfiguration, Volume,
};

use super::{
    epoch_date_time, fsx_arn, tag_list, within_error_message_length,
    ComAmazonawsFsxAwssimbaapiserviceV20180301State, FileSystemRecord, PendingTransition,
    TransitionTarget,
};

/// `VolumeLifecycle` members this slice uses, from the modelled enum
/// (`smithy_query(query="shape", shape="VolumeLifecycle")`: CREATING | CREATED | DELETING |
/// FAILED | MISCONFIGURED | PENDING | AVAILABLE). A volume a file-system create provisions
/// starts `CREATING` ("Amazon FSx is creating the new volume") and becomes `AVAILABLE`
/// ("the volume is fully available for use"); the other members describe work this slice
/// never starts.
pub const VOLUME_LIFECYCLE_CREATING: &str = "CREATING";
/// See [`VOLUME_LIFECYCLE_CREATING`].
pub const VOLUME_LIFECYCLE_AVAILABLE: &str = "AVAILABLE";

/// The `VolumeType` of a volume of an OpenZFS file system, from the modelled enum
/// (`smithy_query(query="shape", shape="VolumeType")`: ONTAP | OPENZFS).
pub const VOLUME_TYPE_OPENZFS: &str = "OPENZFS";

/// The resource type a volume's `ResourceARN` names, as the `<type>/<id>` half of an FSx
/// ARN (one of [`super::FSX_RESOURCE_TYPES`]).
pub const VOLUME_RESOURCE_TYPE: &str = "volume";

/// The name and path of an OpenZFS file system's ROOT volume.
///
/// AWS documents the root volume of an FSx for OpenZFS file system as `fsx`: it is the
/// volume a client mounts as `<file-system-dns-name>:/fsx`, and `OpenZFSVolumeConfiguration
/// $VolumePath` is "the path to the volume from the root volume" (`fsx/parentVolume/...`).
pub const OPEN_ZFS_ROOT_VOLUME_NAME: &str = "fsx";
/// See [`OPEN_ZFS_ROOT_VOLUME_NAME`].
pub const OPEN_ZFS_ROOT_VOLUME_PATH: &str = "/fsx";

/// The record size an OpenZFS volume has when its configuration states none, from
/// `OpenZFSCreateRootVolumeConfiguration$RecordSizeKiB` ("The default is 128 KiB").
pub const DEFAULT_RECORD_SIZE_KIB: i32 = 128;

/// The compression an OpenZFS volume has when its configuration states none, from the
/// modelled `OpenZFSDataCompressionType` documentation ("The compression type is NONE by
/// default").
pub const DEFAULT_DATA_COMPRESSION_TYPE: &str = "NONE";

/// Whether an OpenZFS volume's tags are copied to its snapshots when its configuration
/// states nothing, from `OpenZFSCreateRootVolumeConfiguration$CopyTagsToSnapshots` ("This
/// value defaults to false").
pub const DEFAULT_COPY_TAGS_TO_SNAPSHOTS: bool = false;

/// The largest page `DescribeVolumes` returns, from the modelled `Volumes` list's own
/// `length: max 50` (`smithy_query(query="operation", operation="DescribeVolumes")`).
pub const MAX_VOLUMES_PER_PAGE: usize = 50;

/// The most volumes this store holds for ONE file system.
///
/// AWS publishes a default quota of 500 volumes per FSx for OpenZFS file system, and models
/// the refusal as `ServiceLimitExceeded` with `ServiceLimit::VOLUMES_PER_FILE_SYSTEM`
/// (`smithy_query(query="shape", shape="ServiceLimit")`), so the records are bounded by the
/// service's own limit rather than by the size of the process.
pub const MAX_VOLUMES_PER_FILE_SYSTEM: usize = 500;

/// The most volumes this store holds AT ALL, across every file system: the per-file-system
/// quota over the account's own bound on live file systems ([`super::MAX_FILE_SYSTEMS`]),
/// which is the most a client can hold at once.
pub const MAX_VOLUMES: usize = super::MAX_FILE_SYSTEMS * MAX_VOLUMES_PER_FILE_SYSTEM;

/// The modelled `ServiceLimit` value that goes with [`MAX_VOLUMES_PER_FILE_SYSTEM`].
pub const SERVICE_LIMIT_VOLUMES_PER_FILE_SYSTEM: &str = "VOLUMES_PER_FILE_SYSTEM";

/// The modelled `VolumeFilterName`s `DescribeVolumes` selects volumes by
/// (`smithy_query(query="shape", shape="VolumeFilterName")`: file-system-id |
/// storage-virtual-machine-id).
pub const VOLUME_FILTER_NAMES: &[&str] = &[
    VOLUME_FILTER_FILE_SYSTEM_ID,
    VOLUME_FILTER_STORAGE_VIRTUAL_MACHINE_ID,
];
/// See [`VOLUME_FILTER_NAMES`].
pub const VOLUME_FILTER_FILE_SYSTEM_ID: &str = "file-system-id";
/// See [`VOLUME_FILTER_NAMES`].
pub const VOLUME_FILTER_STORAGE_VIRTUAL_MACHINE_ID: &str = "storage-virtual-machine-id";

/// [`ComAmazonawsFsxAwssimbaapiserviceV20180301State::insert_volume`] refused the record:
/// the store already holds as many volumes as the service's own limit allows. The caller
/// answers with the modelled `ServiceLimitExceeded`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeCountLimitReached {
    /// The file system already holds [`MAX_VOLUMES_PER_FILE_SYSTEM`].
    PerFileSystem,
    /// The store already holds [`MAX_VOLUMES`].
    Store,
}

/// One stored volume. Mirrors the modelled `Volume` shape so [`VolumeRecord::to_volume`] is
/// a projection and never a second source of truth.
#[derive(Debug, Clone)]
pub struct VolumeRecord {
    pub volume_id: String,
    /// The modelled `Volume$VolumeType`.
    pub volume_type: String,
    pub name: String,
    /// The file system this volume belongs to; the volume is removed with it.
    pub file_system_id: String,
    pub lifecycle: String,
    pub creation_time: SystemTime,
    pub resource_arn: String,
    pub tags: BTreeMap<String, String>,
    pub open_zfs_configuration: Option<OpenZFSVolumeConfiguration>,
    pub pending: Option<PendingTransition>,
}

impl VolumeRecord {
    /// This record as the modelled `Volume` shape. Every member comes from the stored
    /// record, so a response can never disagree with what a later read sees.
    pub fn to_volume(&self) -> Volume {
        Volume {
            creation_time: Some(epoch_date_time(self.creation_time)),
            file_system_id: Some(self.file_system_id.clone()),
            lifecycle: Some(self.lifecycle.clone()),
            name: Some(self.name.clone()),
            // An ONTAP volume belongs to a storage virtual machine, which
            // `CreateStorageVirtualMachine` (still 501) would mint: this slice holds no
            // ONTAP volumes, so there is no configuration of one to report.
            ontap_configuration: None,
            resource_arn: Some(self.resource_arn.clone()),
            tags: tag_list(&self.tags),
            volume_id: Some(self.volume_id.clone()),
            volume_type: Some(self.volume_type.clone()),
            // A volume this slice mints either owes the clock its transition or has
            // completed it, so there is no reason to explain and no action in progress.
            lifecycle_transition_reason: None,
            administrative_actions: None,
            open_zfs_configuration: self.open_zfs_configuration.clone(),
        }
    }

    /// Whether this volume satisfies one `Filter` of `DescribeVolumes`: the value the
    /// filter names, read off the stored record.
    ///
    /// `None` for a filter name no stored volume carries a value for, which selects
    /// nothing rather than everything.
    pub fn filtered_value(&self, filter_name: &str) -> Option<&str> {
        match filter_name {
            VOLUME_FILTER_FILE_SYSTEM_ID => Some(self.file_system_id.as_str()),
            // A storage virtual machine is an ONTAP resource this slice does not store
            // (`CreateStorageVirtualMachine` is still 501), and an OpenZFS volume belongs
            // to none, so no value of this filter selects one.
            VOLUME_FILTER_STORAGE_VIRTUAL_MACHINE_ID => None,
            _ => None,
        }
    }
}

/// The ROOT volume an OpenZFS file-system create provisions, from the configuration that
/// create states for it.
///
/// The documented defaults are MATERIALISED here rather than echoed, the same way the file
/// system's own defaults are: a stored volume reports the record size, compression and
/// snapshot-tag behaviour it really has. The root volume has no parent volume and no
/// storage reservation or quota, because both are "from the parent volume", which it has
/// none of.
pub fn open_zfs_root_volume(
    volume_id: String,
    file_system_id: &str,
    region: &str,
    requested: Option<&OpenZFSCreateRootVolumeConfiguration>,
    tags: BTreeMap<String, String>,
) -> VolumeRecord {
    VolumeRecord {
        resource_arn: fsx_arn(region, VOLUME_RESOURCE_TYPE, &volume_id),
        volume_id,
        volume_type: VOLUME_TYPE_OPENZFS.to_string(),
        name: OPEN_ZFS_ROOT_VOLUME_NAME.to_string(),
        file_system_id: file_system_id.to_string(),
        lifecycle: VOLUME_LIFECYCLE_CREATING.to_string(),
        creation_time: SystemTime::now(),
        tags,
        open_zfs_configuration: Some(OpenZFSVolumeConfiguration {
            parent_volume_id: None,
            volume_path: Some(OPEN_ZFS_ROOT_VOLUME_PATH.to_string()),
            storage_capacity_reservation_gi_b: None,
            storage_capacity_quota_gi_b: None,
            record_size_ki_b: requested
                .and_then(|configuration| configuration.record_size_ki_b)
                .or(Some(DEFAULT_RECORD_SIZE_KIB)),
            data_compression_type: requested
                .and_then(|configuration| configuration.data_compression_type.clone())
                .or_else(|| Some(DEFAULT_DATA_COMPRESSION_TYPE.to_string())),
            copy_tags_to_snapshots: requested
                .and_then(|configuration| configuration.copy_tags_to_snapshots)
                .or(Some(DEFAULT_COPY_TAGS_TO_SNAPSHOTS)),
            // The volume was created with the file system, not from a snapshot, and no
            // snapshot operation has run against it: `CreateSnapshot`,
            // `RestoreVolumeFromSnapshot` and `CopySnapshotAndUpdateVolume` are the
            // operations that would fill these, and each is still at 501.
            origin_snapshot: None,
            read_only: requested.and_then(|configuration| configuration.read_only),
            nfs_exports: requested.and_then(|configuration| configuration.nfs_exports.clone()),
            user_and_group_quotas: requested
                .and_then(|configuration| configuration.user_and_group_quotas.clone()),
            restore_to_snapshot: None,
            delete_intermediate_snaphots: None,
            delete_cloned_volumes: None,
            delete_intermediate_data: None,
            source_snapshot_arn: None,
            destination_snapshot: None,
            copy_strategy: None,
        }),
        // Provisioned with the file system, so the clock owes it the same transition to
        // `AVAILABLE` the file system owes.
        pending: Some(PendingTransition::due_now_plus_span(
            TransitionTarget::Available,
        )),
    }
}

/// The root volume a RESTORE provisions, from the volume configuration the backup
/// recorded.
///
/// A backup records the volumes of the file system it was taken of, so a restore that
/// states no root volume configuration of its own gets the recorded one — with the new
/// file system's own minted id, because the volume is a new resource and not the one the
/// backup was taken of.
pub fn open_zfs_root_volume_from_record(
    volume_id: String,
    file_system_id: &str,
    region: &str,
    recorded: &OpenZFSVolumeConfiguration,
    tags: BTreeMap<String, String>,
) -> VolumeRecord {
    VolumeRecord {
        resource_arn: fsx_arn(region, VOLUME_RESOURCE_TYPE, &volume_id),
        volume_id,
        volume_type: VOLUME_TYPE_OPENZFS.to_string(),
        name: OPEN_ZFS_ROOT_VOLUME_NAME.to_string(),
        file_system_id: file_system_id.to_string(),
        lifecycle: VOLUME_LIFECYCLE_CREATING.to_string(),
        creation_time: SystemTime::now(),
        tags,
        open_zfs_configuration: Some(recorded.clone()),
        pending: Some(PendingTransition::due_now_plus_span(
            TransitionTarget::Available,
        )),
    }
}

/// The tags the root volume of `file_system` carries.
///
/// AWS documents `CopyTagsToVolumes` on the OpenZFS configuration as "a Boolean value
/// indicating whether tags for the file system should be copied to volumes", defaulting to
/// false, so the volume carries the file system's tags exactly when the file system says
/// they are copied.
pub fn root_volume_tags(file_system: &FileSystemRecord) -> BTreeMap<String, String> {
    let copies = file_system
        .open_zfs_configuration
        .as_ref()
        .and_then(|configuration| configuration.copy_tags_to_volumes)
        .unwrap_or(false);
    if copies {
        file_system.tags.clone()
    } else {
        BTreeMap::new()
    }
}

/// The message a refused volume insert is reported with: which bound it hit, and the limit
/// that states it. The wording lives beside the bounds, so every operation that provisions
/// a volume refuses it in the same words.
pub fn volume_count_limit_message(limit: VolumeCountLimitReached) -> String {
    match limit {
        VolumeCountLimitReached::PerFileSystem => within_error_message_length(format!(
            "this file system already has the maximum of {MAX_VOLUMES_PER_FILE_SYSTEM} volumes"
        )),
        VolumeCountLimitReached::Store => within_error_message_length(format!(
            "this account already has the maximum of {MAX_VOLUMES} volumes in this Region"
        )),
    }
}

impl ComAmazonawsFsxAwssimbaapiserviceV20180301State {
    /// Apply every volume lifecycle transition the clock has already made due. Called from
    /// [`ComAmazonawsFsxAwssimbaapiserviceV20180301State::settle_due_transitions`], which
    /// every handler runs.
    pub(super) fn settle_due_volume_transitions(&mut self, now: Instant) {
        let mut removed: Vec<String> = Vec::new();
        for (id, record) in &mut self.volumes {
            let Some(pending) = record.pending else {
                continue;
            };
            if pending.due > now {
                continue;
            }
            match pending.target {
                TransitionTarget::Available => {
                    record.lifecycle = VOLUME_LIFECYCLE_AVAILABLE.to_string();
                    record.pending = None;
                }
                TransitionTarget::Removed => removed.push(id.clone()),
            }
        }
        for id in removed {
            self.volumes.remove(&id);
        }
    }

    /// Store a newly provisioned volume, refusing once its file system holds
    /// [`MAX_VOLUMES_PER_FILE_SYSTEM`] or the store holds [`MAX_VOLUMES`].
    ///
    /// The ONE write path for a new record, so neither bound can be bypassed by a later
    /// operation that also creates volumes (`CreateVolume`, say).
    pub fn insert_volume(&mut self, record: VolumeRecord) -> Result<(), VolumeCountLimitReached> {
        if self.volumes.contains_key(&record.volume_id) {
            self.volumes.insert(record.volume_id.clone(), record);
            return Ok(());
        }
        if self.volumes.len() >= MAX_VOLUMES {
            return Err(VolumeCountLimitReached::Store);
        }
        if self.volume_count_of_file_system(&record.file_system_id) >= MAX_VOLUMES_PER_FILE_SYSTEM {
            return Err(VolumeCountLimitReached::PerFileSystem);
        }
        self.volumes.insert(record.volume_id.clone(), record);
        Ok(())
    }

    /// How many stored volumes belong to `file_system_id`.
    pub fn volume_count_of_file_system(&self, file_system_id: &str) -> usize {
        self.volumes
            .values()
            .filter(|record| record.file_system_id == file_system_id)
            .count()
    }

    /// Remove every volume of `file_system_id`.
    ///
    /// A volume belongs to its file system — AWS deletes the volumes of an OpenZFS file
    /// system with it — so the records go when the file system does, which is also what
    /// keeps this store bounded by the live file systems.
    pub fn remove_volumes_of_file_system(&mut self, file_system_id: &str) {
        self.volumes
            .retain(|_, record| record.file_system_id != file_system_id);
    }

    /// Mint the next `VolumeId`.
    ///
    /// Shaped to the modelled `VolumeId` pattern `^(fsvol-[0-9a-f]{17,})$` with
    /// `length 23..23`: `fsvol-` plus 17 lowercase hex digits, the form AWS documents as
    /// "the system-generated, unique ID of the volume".
    pub fn mint_volume_id(&mut self) -> String {
        self.next_volume_id_counter += 1;
        format!("fsvol-{:017x}", self.next_volume_id_counter)
    }
}

/// The `BadRequest$Message` for a `DescribeVolumes` `Filter` whose name this listing does
/// not select by, from the shared wording in
/// [`super::unsupported_filter_name_message_for`] with this listing's own accepted names.
pub fn unsupported_volume_filter_name_message(filter_name: &str) -> String {
    super::unsupported_filter_name_message_for("DescribeVolumes", VOLUME_FILTER_NAMES, filter_name)
}

/// The pagination scope of one `DescribeVolumes` listing, from the shared
/// [`super::filtered_page_scope`] under this listing's own name.
pub fn volumes_page_scope(selected: &[String], filters: &[(String, Vec<String>)]) -> String {
    super::filtered_page_scope("volumes", selected, filters)
}
