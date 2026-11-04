// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-efs-magnolioapiservice-v20150201 service.
//!
//! One lock-guarded struct per engine, reached through [`service_state`]. It holds the
//! file systems, the creation tokens they were made idempotent by, their mount targets
//! and access points, and the VPC/subnet facts a mount target description is built from
//! — plus the counter every `fs-`/`fsmt-`/`fsap-`/`eni-`/`vpc-` id is minted from.
//!
//! The record types here are the single source of every description this service
//! returns: `FileSystem::describe`, `MountTarget::describe` and `AccessPoint::describe`
//! build the modelled output shape, so a create and a later describe cannot disagree.
//!
//! The request-derived error MESSAGES are built here too ([`does_not_exist`],
//! [`file_system_already_exists_message`]), for the same reason: every operation that
//! resolves an id refuses with one sentence construction instead of its own.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    CreateAccessPointOutput, CreateFileSystemOutput, CreateMountTargetOutput,
    CreateReplicationConfigurationOutput, Destination, FileSystemSize, LifecyclePolicy, PosixUser,
    RootDirectory, Tag, UpdateFileSystemOutput, UpdateFileSystemProtectionOutput,
};

/// Values of the modelled `LifeCycleState` enum this slice produces
/// (`smithy_query(query="shape",shape="LifeCycleState")` -> creating | available |
/// updating | deleting | deleted | error).
pub const LIFE_CYCLE_CREATING: &str = "creating";
/// See [`LIFE_CYCLE_CREATING`].
pub const LIFE_CYCLE_AVAILABLE: &str = "available";
/// See [`LIFE_CYCLE_CREATING`]. The phase a file system is in while a change the
/// service applies in the background — a throughput change — is still settling, and
/// `UpdateFileSystem`'s `IncorrectFileSystemLifeCycleState` is "returned if the file
/// system's lifecycle state is not available", so a second change cannot start while
/// one is in flight.
pub const LIFE_CYCLE_UPDATING: &str = "updating";
/// See [`LIFE_CYCLE_CREATING`]. The phase a resource a delete was accepted for is in
/// until that deletion completes: `DeleteAccessPoint` documents that "after deletion is
/// complete, new clients can no longer connect to the access point", so the deletion is
/// not complete when the call returns; see [`DELETING_FOR`].
pub const LIFE_CYCLE_DELETING: &str = "deleting";

/// Values of the modelled `ReplicationStatus` enum this slice produces
/// (`smithy_query(query="shape",shape="ReplicationStatus")` -> ENABLED | ENABLING |
/// DELETING | ERROR | PAUSED | PAUSING).
pub const REPLICATION_STATUS_ENABLING: &str = "ENABLING";
/// See [`REPLICATION_STATUS_ENABLING`].
pub const REPLICATION_STATUS_ENABLED: &str = "ENABLED";

/// Values of the modelled `ReplicationOverwriteProtection` enum
/// (`smithy_query(query="shape",shape="ReplicationOverwriteProtection")` -> ENABLED |
/// DISABLED | REPLICATING). `FileSystemProtectionDescription$ReplicationOverwriteProtection`:
/// "ENABLED – The file system cannot be used as the destination file system in a
/// replication configuration. The file system is writeable. Replication overwrite
/// protection is ENABLED by default."
pub const PROTECTION_ENABLED: &str = "ENABLED";
/// "DISABLED – The file system can be used as the destination file system in a
/// replication configuration. The file system is read-only and can only be modified by
/// EFS replication." See [`PROTECTION_ENABLED`].
pub const PROTECTION_DISABLED: &str = "DISABLED";
/// "REPLICATING – The file system is being used as the destination file system in a
/// replication configuration." See [`PROTECTION_ENABLED`].
pub const PROTECTION_REPLICATING: &str = "REPLICATING";

/// `CreateReplicationConfiguration`: "There can only be one destination file system in a
/// replication configuration", and its `Destinations` member: "Only one destination
/// configuration object is supported." The modelled list carries no length bound, so a
/// handler enforces the documented count itself.
pub const MAX_REPLICATION_DESTINATIONS: usize = 1;

/// `DescribeReplicationConfigurations$MaxResults`: "The default value is 100."
pub const DEFAULT_REPLICATION_PAGE_SIZE: usize = 100;

/// `PerformanceMode` value `generalPurpose`, the only one the Archive storage class is
/// available for.
pub const PERFORMANCE_MODE_GENERAL_PURPOSE: &str = "generalPurpose";

/// `CreateFileSystem$PerformanceMode`: "Default is generalPurpose."
pub const DEFAULT_PERFORMANCE_MODE: &str = PERFORMANCE_MODE_GENERAL_PURPOSE;
/// `CreateFileSystem$ThroughputMode`: "Default is bursting."
pub const DEFAULT_THROUGHPUT_MODE: &str = "bursting";
/// `ThroughputMode` value that requires `ProvisionedThroughputInMibps`.
pub const THROUGHPUT_MODE_PROVISIONED: &str = "provisioned";
/// `ThroughputMode` value `maxIO` is not supported with.
pub const THROUGHPUT_MODE_ELASTIC: &str = "elastic";
/// `PerformanceMode` value that is not supported on One Zone or Elastic file systems.
pub const PERFORMANCE_MODE_MAX_IO: &str = "maxIO";

/// `CreateFileSystem$ProvisionedThroughputInMibps`: "Valid values are 1-3414 MiBps,
/// with the upper limit depending on Region." The model carries only the lower bound
/// (`range {min: 1}`), so the documented upper one is enforced here.
pub const MAX_PROVISIONED_THROUGHPUT_MIBPS: f64 = 3414.0;

/// `CreateFileSystem$Encrypted`: "If you don't specify a KMS key, then the default KMS
/// key for Amazon EFS, /aws/elasticfilesystem, is used to protect the encrypted file
/// system."
pub const DEFAULT_EFS_KMS_ALIAS: &str = "alias/aws/elasticfilesystem";

/// The `IpAddressType` values of the modelled enum
/// (`smithy_query(query="shape",shape="IpAddressType")`).
pub const IP_ADDRESS_TYPE_IPV4_ONLY: &str = "IPV4_ONLY";
/// See [`IP_ADDRESS_TYPE_IPV4_ONLY`].
pub const IP_ADDRESS_TYPE_IPV6_ONLY: &str = "IPV6_ONLY";
/// See [`IP_ADDRESS_TYPE_IPV4_ONLY`].
pub const IP_ADDRESS_TYPE_DUAL_STACK: &str = "DUAL_STACK";

/// The region an unsigned request (one carrying no SigV4 credential scope) is
/// attributed to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// Published EFS quota: "Tags per file system: 50" ("Amazon EFS quotas that you cannot
/// change", Amazon EFS User Guide), which is also AWS's general limit of "50 tags per
/// resource" ("Tagging AWS resources", AWS General Reference), so every resource this
/// service tags is bounded by it. The modelled `Tags` list carries no length bound, so a
/// handler enforces the quota itself before it walks a caller-supplied list.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// Published in `CreateAccessPoint`'s own documentation: "A file system can have a
/// maximum of 10,000 access points unless you request an increase." It is also what
/// bounds this store: a file system holds at most this many access points and the Region
/// at most [`MAX_FILE_SYSTEMS_PER_REGION`] file systems, so a repeated create cannot grow
/// the process without limit; `AccessPointLimitExceeded` is the modelled answer past it.
pub const MAX_ACCESS_POINTS_PER_FILE_SYSTEM: usize = 10_000;

/// `DescribeAccessPoints$MaxResults`: "The default value is 100."
pub const DEFAULT_ACCESS_POINT_PAGE_SIZE: usize = 100;

/// `ListTagsForResource$MaxResults`: "(Optional) Specifies the maximum number of tag
/// objects to return in the response. The default value is 100."
pub const DEFAULT_TAG_PAGE_SIZE: usize = 100;

/// The root directory an access point exposes when the request names none: the file
/// system root. `RootDirectory$Path` "specifies the path on the EFS file system to expose
/// as the root directory to NFS clients using the access point", and its modelled pattern
/// (`^(\/|…)$`) admits exactly this value for the whole file system.
pub const ROOT_DIRECTORY_DEFAULT_PATH: &str = "/";

/// Published EFS quota: "File systems per account, per Region: 1,000" (same page). The
/// store is bounded by it, so a repeated create cannot grow this process without limit;
/// `FileSystemLimitExceeded` is the modelled answer past it.
pub const MAX_FILE_SYSTEMS_PER_REGION: usize = 1000;

/// Published VPC quota: "Subnets per VPC: 200" ("Amazon VPC quotas", Amazon VPC User
/// Guide). This emulator owns one VPC, so it records at most this many subnets and a
/// request naming a further one is answered `SubnetNotFound`.
pub const MAX_SUBNETS_PER_VPC: usize = 200;

/// Published EFS quota: "Mount targets per file system, per Availability Zone: 1" and
/// "Availability Zones per file system: 1 for One Zone, all for Regional" ("Amazon EFS
/// quotas", Amazon EFS User Guide) — so a file system holds at most one mount target per
/// zone this emulator presents, and the store as a whole is bounded by that times
/// [`MAX_FILE_SYSTEMS_PER_REGION`]. The ceiling is stated and checked here too, rather
/// than left implicit, and `NetworkInterfaceLimitExceeded` is the modelled answer past
/// it: each mount target holds one elastic network interface.
pub const MAX_MOUNT_TARGETS: usize = MAX_FILE_SYSTEMS_PER_REGION * AZ_SUFFIXES.len();

/// `DescribeFileSystems$MaxItems`: "This number is automatically set to 100."
pub const DEFAULT_FILE_SYSTEM_PAGE_SIZE: usize = 100;
/// `DescribeMountTargets$MaxItems`: "Currently, this number is automatically set to 10."
pub const DEFAULT_MOUNT_TARGET_PAGE_SIZE: usize = 10;
/// The page every paginated describe in this service "paginate[s] at 100 per page" at
/// (`DescribeFileSystems$MaxItems`, `DescribeMountTargets$MaxItems`,
/// `DescribeTags$MaxItems`), so a larger `MaxItems` is clamped rather than letting one
/// request walk the whole store.
pub const MAX_PAGE_SIZE: usize = 100;

/// How long a resource this service creates stays in `creating`, and a file system a
/// throughput change stays in `updating`, before the clock owes it `available`. AWS
/// publishes no duration for either transition, so this span is the
/// emulator's own choice inside the band the project fixes for a clock-owed
/// transition (250-750 ms). It is never zero, so a caller that acts immediately after
/// a create still observes `creating` and the state errors that go with it; the due
/// instant is stored on the record and materialised on the next call, never slept for.
pub const CREATING_FOR: Duration = Duration::from_millis(500);

/// How long a resource a delete has been accepted for stays in `deleting` before the
/// clock owes its removal. AWS publishes no duration for the transition either, so this
/// span is the emulator's own choice inside the same band (250-750 ms) and is never
/// zero: a caller that acts immediately after a delete still observes the record in
/// `deleting`, which is what lets every racing delete of one live resource be accepted
/// and converge on a single deletion. The due instant is stored on the record and
/// materialised on the next call, never slept for.
pub const DELETING_FOR: Duration = Duration::from_millis(500);

/// `UpdateFileSystem`'s `TooManyRequests` (HTTP 429): "Returned if you don't wait at
/// least 24 hours before either changing the throughput mode, or decreasing the
/// Provisioned Throughput value." The duration is the one that error documents, so it
/// is kept as published rather than chosen by this emulator.
pub const THROUGHPUT_CHANGE_COOLDOWN: Duration = Duration::from_secs(24 * 60 * 60);

/// Availability Zone name suffixes. An Availability Zone name is the Region code
/// followed by a letter identifier ("Regions and Zones", Amazon EC2 User Guide); every
/// commercial Region publishes at least three, so this emulator models six per Region.
pub const AZ_SUFFIXES: [char; 6] = ['a', 'b', 'c', 'd', 'e', 'f'];

/// Every VPC subnet reserves its first four addresses and its last one ("Subnet CIDR
/// blocks", Amazon VPC User Guide), so an assignable host octet of an emulated /24 runs
/// from 4 through 254.
const FIRST_ASSIGNABLE_HOST: u32 = 4;
/// See [`FIRST_ASSIGNABLE_HOST`].
const LAST_ASSIGNABLE_HOST: u32 = 254;

/// An emulated VPC subnet: a mount target's Availability Zone, VPC and address range
/// all come from its subnet, so the facts are recorded once, on first reference, and
/// every later mount target in the same subnet reads the same record back.
#[derive(Debug, Clone)]
pub struct Subnet {
    /// The `subnet-…` id the caller named.
    pub subnet_id: String,
    /// Allocation order of this subnet, which fixes its Availability Zone and its /24.
    pub ordinal: u32,
    /// The VPC every subnet of this emulator belongs to.
    pub vpc_id: String,
    /// `<region><letter>`, e.g. `us-east-1a`.
    pub availability_zone_name: String,
    /// `<region abbreviation>-az<n>`, e.g. `use1-az1`.
    pub availability_zone_id: String,
}

impl Subnet {
    /// The first three octets of this subnet's emulated `10.x.y.0/24`.
    fn ipv4_prefix(&self) -> (u32, u32, u32) {
        (10, (self.ordinal >> 8) & 0xff, self.ordinal & 0xff)
    }

    /// The IPv4 addresses this subnet can assign, lowest first.
    pub fn assignable_ipv4(&self) -> impl Iterator<Item = String> + '_ {
        let (a, b, c) = self.ipv4_prefix();
        (FIRST_ASSIGNABLE_HOST..=LAST_ASSIGNABLE_HOST).map(move |host| format!("{a}.{b}.{c}.{host}"))
    }

    /// The IPv6 addresses this subnet can assign, lowest first, from the unique-local
    /// `fd00::/8` space so an emulated address can never be a routable one.
    pub fn assignable_ipv6(&self) -> impl Iterator<Item = String> + '_ {
        let (_, b, c) = self.ipv4_prefix();
        (FIRST_ASSIGNABLE_HOST..=LAST_ASSIGNABLE_HOST)
            .map(move |host| format!("fd00:ec2:{b:x}:{c:x}::{host:x}"))
    }
}

/// A throughput change an `UpdateFileSystem` call has ACCEPTED and the clock has not
/// applied yet.
///
/// `UpdateFileSystem` answers HTTP 202, an acceptance rather than a completion: the file
/// system moves to the `updating` lifecycle phase and goes on reporting the throughput it
/// currently has until the transition finishes, so the requested values wait here instead
/// of on the reported members. See
/// [`ComAmazonawsEfsMagnolioapiserviceV20150201State::materialize_due_transitions`], which
/// applies them when [`CREATING_FOR`] has elapsed.
#[derive(Debug, Clone)]
pub struct PendingThroughput {
    /// The `ThroughputMode` the file system will carry once the change is applied.
    pub throughput_mode: String,
    /// The `ProvisionedThroughputInMibps` it will carry then — `None` for a mode the
    /// member is not valid for.
    pub provisioned_throughput_in_mibps: Option<f64>,
}

/// One stored EFS file system. `NumberOfMountTargets` is not a field: it is the length
/// of [`Self::mount_target_ids`], so the count cannot drift from the mount targets that
/// back it.
#[derive(Debug, Clone)]
pub struct FileSystem {
    /// Creation order, which fixes the order `DescribeFileSystems` pages in.
    pub ordinal: u64,
    pub file_system_id: String,
    pub file_system_arn: String,
    pub owner_id: String,
    pub creation_token: String,
    pub creation_time: SystemTime,
    /// When the clock owes this file system `available` again, whether it is `creating`
    /// or `updating`; see [`CREATING_FOR`].
    pub available_at: SystemTime,
    pub life_cycle_state: String,
    pub name: Option<String>,
    pub performance_mode: String,
    pub throughput_mode: String,
    /// When a throughput change the 24-hour restriction covers — a throughput-mode
    /// change, or a decrease of provisioned throughput — was last applied to this file
    /// system, and `None` while none has been. See [`THROUGHPUT_CHANGE_COOLDOWN`].
    pub last_restricted_throughput_change: Option<SystemTime>,
    pub provisioned_throughput_in_mibps: Option<f64>,
    /// The throughput change an accepted `UpdateFileSystem` has not applied yet, and
    /// `None` when none is outstanding. While one is held the file system reports its
    /// CURRENT throughput and the `updating` phase; see [`PendingThroughput`].
    pub pending_throughput: Option<PendingThroughput>,
    /// Whether this file system is encrypted. `FileSystemDescription$Encrypted` is "A
    /// Boolean value that, if true, indicates that the file system is encrypted" — a fact
    /// every file system has, so the description reports it either way rather than leaving
    /// the member out of an unencrypted one's.
    pub encrypted: bool,
    pub kms_key_id: Option<String>,
    pub availability_zone_name: Option<String>,
    pub availability_zone_id: Option<String>,
    /// Whether automatic backups were requested at creation. Read by the backup-policy
    /// operations, which are still scaffolded.
    pub backup: bool,
    /// The last known metered size, in bytes, and when it was metered — `None` while no
    /// metering has happened, which is what leaves `SizeInBytes.Timestamp` ("the time at
    /// which the size of data, returned in the Value field, was determined") out of the
    /// description. This emulator stores no file data, so nothing meters a file system.
    pub size_in_bytes: i64,
    pub size_measured_at: Option<SystemTime>,
    pub tags: Vec<Tag>,
    pub mount_target_ids: Vec<String>,
    /// This file system's replication overwrite protection: one of
    /// [`PROTECTION_ENABLED`], [`PROTECTION_DISABLED`] or [`PROTECTION_REPLICATING`].
    /// "Replication overwrite protection is ENABLED by default."
    pub replication_overwrite_protection: String,
    /// This file system's `LifecycleConfiguration`, as PutLifecycleConfiguration last
    /// stored it. "Each Amazon EFS file system supports one lifecycle configuration,
    /// which applies to all files in the file system", so it lives on the file-system
    /// record and goes away with it; empty means the file system has none, which
    /// DescribeLifecycleConfiguration reports as "an empty array in the response".
    pub lifecycle_policies: Vec<LifecyclePolicy>,
}

impl FileSystem {
    /// This record as the modelled `FileSystemDescription`.
    pub fn describe(&self) -> CreateFileSystemOutput {
        CreateFileSystemOutput {
            owner_id: self.owner_id.clone(),
            creation_token: self.creation_token.clone(),
            file_system_id: self.file_system_id.clone(),
            file_system_arn: Some(self.file_system_arn.clone()),
            creation_time: epoch_seconds(self.creation_time),
            life_cycle_state: self.life_cycle_state.clone(),
            name: self.name.clone(),
            number_of_mount_targets: i32::try_from(self.mount_target_ids.len())
                .unwrap_or(i32::MAX),
            // "The latest known metered size (in bytes) of data stored in the file
            // system, in its Value field, and the time at which that size was determined
            // in its Timestamp field": every per-storage-class member is reported — a
            // class holding nothing reports zero rather than being left out — and
            // `Timestamp` only once a metering time exists on the record.
            size_in_bytes: FileSystemSize {
                value: self.size_in_bytes,
                timestamp: self.size_measured_at.map(epoch_seconds),
                value_in_ia: Some(0),
                value_in_standard: Some(self.size_in_bytes),
                value_in_archive: Some(0),
            },
            performance_mode: self.performance_mode.clone(),
            encrypted: Some(self.encrypted),
            kms_key_id: self.kms_key_id.clone(),
            throughput_mode: Some(self.throughput_mode.clone()),
            provisioned_throughput_in_mibps: self.provisioned_throughput_in_mibps,
            availability_zone_name: self.availability_zone_name.clone(),
            availability_zone_id: self.availability_zone_id.clone(),
            tags: self.tags.clone(),
            // "Describes the protection on a file system" — every file system carries a
            // replication overwrite protection status, so the member is always reported.
            file_system_protection: Some(UpdateFileSystemProtectionOutput {
                replication_overwrite_protection: Some(
                    self.replication_overwrite_protection.clone(),
                ),
            }),
        }
    }

    /// This record as the modelled `FileSystemDescription`, in the per-operation shape
    /// `UpdateFileSystem` answers with.
    ///
    /// The model binds `CreateFileSystem`, `DescribeFileSystems` and `UpdateFileSystem`
    /// to the SAME `FileSystemDescription` output shape, but the generated types are
    /// per operation, so this is a projection of [`Self::describe`] rather than a second
    /// description: the two can never disagree.
    pub fn describe_update(&self) -> UpdateFileSystemOutput {
        let described = self.describe();
        UpdateFileSystemOutput {
            owner_id: described.owner_id,
            creation_token: described.creation_token,
            file_system_id: described.file_system_id,
            file_system_arn: described.file_system_arn,
            creation_time: described.creation_time,
            life_cycle_state: described.life_cycle_state,
            name: described.name,
            number_of_mount_targets: described.number_of_mount_targets,
            size_in_bytes: described.size_in_bytes,
            performance_mode: described.performance_mode,
            encrypted: described.encrypted,
            kms_key_id: described.kms_key_id,
            throughput_mode: described.throughput_mode,
            provisioned_throughput_in_mibps: described.provisioned_throughput_in_mibps,
            availability_zone_name: described.availability_zone_name,
            availability_zone_id: described.availability_zone_id,
            tags: described.tags,
            file_system_protection: described.file_system_protection,
        }
    }

    /// Apply the throughput change the clock owes this file system, if it carries one.
    ///
    /// Called where the `updating` -> `available` transition is materialised, so the
    /// requested throughput becomes the file system's OWN configuration at exactly the
    /// instant the transition completes — never before, which is what makes the 202 an
    /// acceptance rather than a completion.
    fn settle_throughput_change(&mut self) {
        if let Some(pending) = self.pending_throughput.take() {
            self.throughput_mode = pending.throughput_mode;
            self.provisioned_throughput_in_mibps = pending.provisioned_throughput_in_mibps;
        }
    }
}

/// One destination of a stored replication configuration. "There can only be one
/// destination file system in a replication configuration", so a configuration holds at
/// most [`MAX_REPLICATION_DESTINATIONS`] of these.
#[derive(Debug, Clone)]
pub struct ReplicationDestination {
    pub file_system_id: String,
    pub region: String,
    pub owner_id: String,
    /// The IAM role Amazon EFS performs replication under on a cross-account
    /// destination, when the request named one.
    pub role_arn: Option<String>,
    /// One of [`REPLICATION_STATUS_ENABLING`] / [`REPLICATION_STATUS_ENABLED`].
    pub status: String,
    /// When the clock owes this destination [`REPLICATION_STATUS_ENABLED`]; see
    /// [`CREATING_FOR`].
    pub enabled_at: SystemTime,
}

impl ReplicationDestination {
    /// This record as the modelled `Destination`.
    pub fn describe(&self) -> Destination {
        Destination {
            status: self.status.clone(),
            file_system_id: self.file_system_id.clone(),
            region: self.region.clone(),
            // `Destination$LastReplicatedTimestamp` is the time of the last successful
            // sync: this emulator moves no file data between file systems, so there is
            // no such instant and the member stays absent rather than carrying a time
            // nothing happened at.
            last_replicated_timestamp: None,
            owner_id: Some(self.owner_id.clone()),
            status_message: None,
            role_arn: self.role_arn.clone(),
        }
    }
}

/// One stored EFS replication configuration.
#[derive(Debug, Clone)]
pub struct ReplicationConfiguration {
    /// Creation order, which fixes the order `DescribeReplicationConfigurations` pages
    /// in.
    pub ordinal: u64,
    pub source_file_system_id: String,
    pub source_file_system_arn: String,
    pub original_source_file_system_arn: String,
    pub source_file_system_region: String,
    pub source_file_system_owner_id: String,
    pub creation_time: SystemTime,
    pub destinations: Vec<ReplicationDestination>,
}

impl ReplicationConfiguration {
    /// This record as the modelled `ReplicationConfigurationDescription`.
    pub fn describe(&self) -> CreateReplicationConfigurationOutput {
        CreateReplicationConfigurationOutput {
            source_file_system_id: self.source_file_system_id.clone(),
            source_file_system_region: self.source_file_system_region.clone(),
            source_file_system_arn: self.source_file_system_arn.clone(),
            original_source_file_system_arn: self.original_source_file_system_arn.clone(),
            creation_time: epoch_seconds(self.creation_time),
            destinations: self
                .destinations
                .iter()
                .map(ReplicationDestination::describe)
                .collect(),
            source_file_system_owner_id: Some(self.source_file_system_owner_id.clone()),
        }
    }

    /// Whether `file_system_id` takes part in this configuration, as its source or as
    /// its destination.
    pub fn holds(&self, file_system_id: &str) -> bool {
        self.source_file_system_id == file_system_id
            || self
                .destinations
                .iter()
                .any(|destination| destination.file_system_id == file_system_id)
    }
}

/// One stored EFS mount target.
#[derive(Debug, Clone)]
pub struct MountTarget {
    /// Creation order, which fixes the order `DescribeMountTargets` pages in.
    pub ordinal: u64,
    pub mount_target_id: String,
    pub file_system_id: String,
    pub subnet_id: String,
    pub owner_id: String,
    pub life_cycle_state: String,
    /// When the clock owes this mount target `available`; see [`CREATING_FOR`].
    pub available_at: SystemTime,
    pub ip_address: Option<String>,
    pub ipv6_address: Option<String>,
    pub network_interface_id: String,
    pub availability_zone_id: String,
    pub availability_zone_name: String,
    pub vpc_id: String,
    pub security_groups: Vec<String>,
}

impl MountTarget {
    /// This record as the modelled `MountTargetDescription`.
    pub fn describe(&self) -> CreateMountTargetOutput {
        CreateMountTargetOutput {
            owner_id: Some(self.owner_id.clone()),
            mount_target_id: self.mount_target_id.clone(),
            file_system_id: self.file_system_id.clone(),
            subnet_id: self.subnet_id.clone(),
            life_cycle_state: self.life_cycle_state.clone(),
            ip_address: self.ip_address.clone(),
            ipv6_address: self.ipv6_address.clone(),
            network_interface_id: Some(self.network_interface_id.clone()),
            availability_zone_id: Some(self.availability_zone_id.clone()),
            availability_zone_name: Some(self.availability_zone_name.clone()),
            vpc_id: Some(self.vpc_id.clone()),
        }
    }
}

/// One stored EFS access point: "an application-specific view into an EFS file system".
#[derive(Debug, Clone)]
pub struct AccessPoint {
    /// Creation order, which fixes the order `DescribeAccessPoints` pages in.
    pub ordinal: u64,
    pub access_point_id: String,
    pub access_point_arn: String,
    pub file_system_id: String,
    pub owner_id: String,
    /// "The opaque string specified in the request to ensure idempotent creation."
    pub client_token: String,
    /// "The name of the access point. This is the value of the Name tag."
    pub name: Option<String>,
    pub life_cycle_state: String,
    /// When the clock owes this access point `available`; see [`CREATING_FOR`].
    pub available_at: SystemTime,
    /// When the clock owes this access point's removal, once a delete has been accepted
    /// for it; `None` while no delete has been. See [`DELETING_FOR`].
    pub deleted_at: Option<SystemTime>,
    pub posix_user: Option<PosixUser>,
    pub root_directory: RootDirectory,
    pub tags: Vec<Tag>,
}

impl AccessPoint {
    /// This record as the modelled `AccessPointDescription`.
    pub fn describe(&self) -> CreateAccessPointOutput {
        CreateAccessPointOutput {
            client_token: Some(self.client_token.clone()),
            name: self.name.clone(),
            tags: Some(self.tags.clone()),
            access_point_id: Some(self.access_point_id.clone()),
            access_point_arn: Some(self.access_point_arn.clone()),
            file_system_id: Some(self.file_system_id.clone()),
            posix_user: self.posix_user.clone(),
            root_directory: Some(self.root_directory.clone()),
            owner_id: Some(self.owner_id.clone()),
            life_cycle_state: Some(self.life_cycle_state.clone()),
        }
    }
}

/// The SplitMix64 increment, the step the id counter advances by.
const SPLITMIX64_STEP: u64 = 0x9E37_79B9_7F4A_7C15;

/// The seed the one emulated VPC's id is derived from. A fixed seed, not a draw from the
/// id counter, so reading the VPC id is a pure read; the value is outside the counter's
/// own sequence because the counter starts at zero and advances by
/// [`SPLITMIX64_STEP`], never reaching it.
const VPC_ID_SEED: u64 = 0x5EFC_0DE0_0000_0001;

/// 16 lowercase hex digits from one SplitMix64 finalisation of `seed`. A bijection, so
/// two distinct seeds never produce the same digits.
fn hex_of(seed: u64) -> String {
    let mut z = seed;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    format!("{z:016x}")
}

/// Per-service state for com-amazonaws-efs-magnolioapiservice-v20150201. A handler
/// reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsEfsMagnolioapiserviceV20150201State {
    /// Keyed by `fs-…` id.
    pub file_systems: BTreeMap<String, FileSystem>,
    /// `CreationToken` -> `fs-…` id; backs CreateFileSystem's idempotency rule.
    pub creation_tokens: BTreeMap<String, String>,
    /// Keyed by `fsmt-…` id.
    pub mount_targets: BTreeMap<String, MountTarget>,
    /// Keyed by `subnet-…` id, recorded on first reference.
    pub subnets: BTreeMap<String, Subnet>,
    /// Keyed by `fsap-…` id.
    pub access_points: BTreeMap<String, AccessPoint>,
    /// `ClientToken` -> `fsap-…` id; backs CreateAccessPoint's idempotency rule.
    pub access_point_tokens: BTreeMap<String, String>,
    /// Keyed by the SOURCE `fs-…` id — the resource a replication configuration is
    /// addressed by, and the key its uniqueness rule is stated over: "A file system can
    /// be part of only one replication configuration."
    pub replications: BTreeMap<String, ReplicationConfiguration>,
    /// Drives [`Self::mint_hex`]; see there for why it is not a plain counter.
    next_id_seed: u64,
    /// Drives [`Self::next_ordinal`].
    next_ordinal: u64,
}

impl ComAmazonawsEfsMagnolioapiserviceV20150201State {
    /// 16 lowercase hex digits for the body of a minted id, so `fs-`/`fsmt-` ids match
    /// their modelled `^(fs|fsmt)-[0-9a-f]{8,40}$` patterns and their length bounds.
    ///
    /// The digits come from SplitMix64 over a stored counter rather than from the
    /// counter itself: the step is a bijection, so two resources never collide, while
    /// the ids do not read as consecutive integers a client could guess the next of.
    pub fn mint_hex(&mut self) -> String {
        self.next_id_seed = self.next_id_seed.wrapping_add(SPLITMIX64_STEP);
        hex_of(self.next_id_seed)
    }

    /// The next creation ordinal, which fixes list and pagination order.
    pub fn next_ordinal(&mut self) -> u64 {
        self.next_ordinal = self.next_ordinal.wrapping_add(1);
        self.next_ordinal
    }

    /// The one VPC every emulated subnet belongs to.
    ///
    /// Derived from a fixed seed rather than minted from the id counter, so this is a
    /// pure read: resolving a subnet's facts for a request that may still be refused
    /// writes nothing at all, and every caller observes the same VPC for the life of the
    /// process.
    pub fn vpc_id(&self) -> String {
        format!("vpc-{}", hex_of(VPC_ID_SEED))
    }

    /// The record for `subnet_id`, or the Availability Zone and /24 it WOULD be given on
    /// first reference — without recording anything. Allocation order fixes the
    /// Availability Zone, so two distinct subnets land in distinct zones until the
    /// Region's zones are exhausted.
    ///
    /// Resolving is deliberately separate from [`Self::record_subnet`]: a subnet id is
    /// caller-chosen and the request that names it may still be refused further down, so
    /// the facts are computed here and committed only on the success path.
    ///
    /// `None` once the emulated VPC already holds [`MAX_SUBNETS_PER_VPC`] subnets and
    /// `subnet_id` is not one of them, which bounds this map against a caller that names
    /// a fresh subnet on every request.
    pub fn subnet(&self, subnet_id: &str, region: &str) -> Option<Subnet> {
        if let Some(existing) = self.subnets.get(subnet_id) {
            return Some(existing.clone());
        }
        if self.subnets.len() >= MAX_SUBNETS_PER_VPC {
            return None;
        }
        let vpc_id = self.vpc_id();
        let ordinal = u32::try_from(self.subnets.len()).unwrap_or(u32::MAX);
        let suffix = AZ_SUFFIXES[usize::try_from(ordinal).unwrap_or(0) % AZ_SUFFIXES.len()];
        let availability_zone_name = format!("{region}{suffix}");
        let availability_zone_id = availability_zone_id(&availability_zone_name);
        Some(Subnet {
            subnet_id: subnet_id.to_string(),
            ordinal,
            vpc_id,
            availability_zone_name,
            availability_zone_id,
        })
    }

    /// Commit the facts [`Self::subnet`] resolved, on the success path. A subnet already
    /// recorded keeps the facts it was first recorded with, so every mount target in one
    /// subnet reports the same zone, VPC and address range.
    pub fn record_subnet(&mut self, subnet: Subnet) {
        self.subnets
            .entry(subnet.subnet_id.clone())
            .or_insert(subnet);
    }

    /// Materialise every lifecycle transition the clock has already made due: a
    /// resource whose `creating` span has elapsed becomes `available`, as does a file
    /// system whose `updating` span has, and a resource whose accepted deletion's
    /// `deleting` span has elapsed is gone. Owed at the same instant whether or not
    /// anybody called, so a reporting operation may apply it.
    pub fn materialize_due_transitions(&mut self, now: SystemTime) {
        for file_system in self.file_systems.values_mut() {
            if (file_system.life_cycle_state == LIFE_CYCLE_CREATING
                || file_system.life_cycle_state == LIFE_CYCLE_UPDATING)
                && file_system.available_at <= now
            {
                // The throughput change an `updating` file system was accepted for is
                // applied by the completion of that transition, not by the call that
                // started it.
                file_system.settle_throughput_change();
                file_system.life_cycle_state = LIFE_CYCLE_AVAILABLE.to_string();
            }
        }
        for mount_target in self.mount_targets.values_mut() {
            if mount_target.life_cycle_state == LIFE_CYCLE_CREATING
                && mount_target.available_at <= now
            {
                mount_target.life_cycle_state = LIFE_CYCLE_AVAILABLE.to_string();
            }
        }
        for access_point in self.access_points.values_mut() {
            if access_point.life_cycle_state == LIFE_CYCLE_CREATING
                && access_point.available_at <= now
            {
                access_point.life_cycle_state = LIFE_CYCLE_AVAILABLE.to_string();
            }
        }
        // An accepted deletion whose `deleting` span has elapsed is complete: the record
        // goes, so the id is "not found" to every later read and delete, and the
        // `ClientToken` it was created with is free to mint an access point again.
        let completed: Vec<String> = self
            .access_points
            .values()
            .filter(|access_point| access_point.deleted_at.is_some_and(|due| due <= now))
            .map(|access_point| access_point.access_point_id.clone())
            .collect();
        for access_point_id in completed {
            if let Some(removed) = self.access_points.remove(&access_point_id) {
                if self.access_point_tokens.get(&removed.client_token)
                    == Some(&removed.access_point_id)
                {
                    self.access_point_tokens.remove(&removed.client_token);
                }
            }
        }
        for replication in self.replications.values_mut() {
            for destination in &mut replication.destinations {
                if destination.status == REPLICATION_STATUS_ENABLING
                    && destination.enabled_at <= now
                {
                    destination.status = REPLICATION_STATUS_ENABLED.to_string();
                }
            }
        }
    }

    /// The replication configuration `file_system_id` takes part in, as its source or as
    /// its destination. "A file system can be part of only one replication
    /// configuration", so there is at most one.
    pub fn replication_of(&self, file_system_id: &str) -> Option<&ReplicationConfiguration> {
        self.replications
            .values()
            .find(|replication| replication.holds(file_system_id))
    }

    /// The stored replication configurations, in creation order — the order
    /// `DescribeReplicationConfigurations` pages in.
    pub fn replications_in_order(&self) -> Vec<&ReplicationConfiguration> {
        let mut replications: Vec<&ReplicationConfiguration> =
            self.replications.values().collect();
        replications.sort_by_key(|replication| replication.ordinal);
        replications
    }

    /// Whether any mount target in `subnet_id` already holds `address`.
    pub fn ipv4_in_use(&self, subnet_id: &str, address: &str) -> bool {
        self.mount_targets.values().any(|mount_target| {
            mount_target.subnet_id == subnet_id
                && mount_target.ip_address.as_deref() == Some(address)
        })
    }

    /// Whether any mount target in `subnet_id` already holds `address`.
    pub fn ipv6_in_use(&self, subnet_id: &str, address: &str) -> bool {
        self.mount_targets.values().any(|mount_target| {
            mount_target.subnet_id == subnet_id
                && mount_target.ipv6_address.as_deref() == Some(address)
        })
    }

    /// The lowest address in `subnet` no mount target holds, as EC2's
    /// `CreateNetworkInterface` assigns a primary private address when the request
    /// names none. `None` when the subnet has no free address left.
    pub fn free_ipv4(&self, subnet: &Subnet) -> Option<String> {
        subnet
            .assignable_ipv4()
            .find(|address| !self.ipv4_in_use(&subnet.subnet_id, address))
    }

    /// See [`Self::free_ipv4`].
    pub fn free_ipv6(&self, subnet: &Subnet) -> Option<String> {
        subnet
            .assignable_ipv6()
            .find(|address| !self.ipv6_in_use(&subnet.subnet_id, address))
    }

    /// How many access points one file system holds — the count alone, without building
    /// the records [`Self::access_points_of`] returns, for the quota check a create runs.
    pub fn access_point_count_of(&self, file_system_id: &str) -> usize {
        self.access_points
            .values()
            .filter(|access_point| access_point.file_system_id == file_system_id)
            .count()
    }

    /// The access points of one file system in creation order — or, when
    /// `file_system_id` is `None`, every stored access point in that same order. The one
    /// way this service lists access points, so a filtered and an unfiltered read cannot
    /// order them differently.
    pub fn access_points_of(&self, file_system_id: Option<&str>) -> Vec<AccessPoint> {
        let mut access_points: Vec<AccessPoint> = self
            .access_points
            .values()
            .filter(|access_point| {
                file_system_id.is_none_or(|id| access_point.file_system_id == id)
            })
            .cloned()
            .collect();
        access_points.sort_by_key(|access_point| access_point.ordinal);
        access_points
    }

    /// Accept a delete of the access point `access_point_id` names: the record moves
    /// into its `deleting` phase and keeps answering reads until the clock removes it
    /// ([`DELETING_FOR`]), which is what makes every delete that reaches a still-present
    /// record succeed. The due instant is pinned by the FIRST accepted delete and is
    /// never postponed by a later one, so repeated or concurrent deletes converge on
    /// exactly one deletion at one instant.
    ///
    /// `false` when no record is present — the id was never minted, or its deletion has
    /// already been materialised — which is the caller's `AccessPointNotFound`.
    pub fn begin_access_point_deletion(
        &mut self,
        access_point_id: &str,
        now: SystemTime,
    ) -> bool {
        let Some(access_point) = self.access_points.get_mut(access_point_id) else {
            return false;
        };
        access_point.life_cycle_state = LIFE_CYCLE_DELETING.to_string();
        if access_point.deleted_at.is_none() {
            access_point.deleted_at = Some(now + DELETING_FOR);
        }
        true
    }

    /// The tag set the resource a `ResourceId` names holds, or `None` when this store
    /// holds no such resource. The one way a tag set is read, so every reporting
    /// operation reads the set the writes left.
    pub fn tags_of(&self, resource: TaggedResource<'_>) -> Option<&[Tag]> {
        match resource {
            TaggedResource::FileSystem(id) => {
                self.file_systems.get(id).map(|held| held.tags.as_slice())
            }
            TaggedResource::AccessPoint(id) => {
                self.access_points.get(id).map(|held| held.tags.as_slice())
            }
        }
    }

    /// The same resource's tag set for a WRITE, together with the `Name` its description
    /// derives from the `Name` tag; see [`TaggedRecordMut`]. `None` when this store holds
    /// no such resource.
    pub fn tagged_record_mut(&mut self, resource: TaggedResource<'_>) -> Option<TaggedRecordMut<'_>> {
        match resource {
            TaggedResource::FileSystem(id) => {
                self.file_systems.get_mut(id).map(|held| TaggedRecordMut {
                    tags: &mut held.tags,
                    name: &mut held.name,
                })
            }
            TaggedResource::AccessPoint(id) => {
                self.access_points.get_mut(id).map(|held| TaggedRecordMut {
                    tags: &mut held.tags,
                    name: &mut held.name,
                })
            }
        }
    }

    /// The mount targets of one file system, in creation order.
    pub fn mount_targets_of(&self, file_system_id: &str) -> Vec<MountTarget> {
        let mut targets: Vec<MountTarget> = self
            .mount_targets
            .values()
            .filter(|mount_target| mount_target.file_system_id == file_system_id)
            .cloned()
            .collect();
        targets.sort_by_key(|mount_target| mount_target.ordinal);
        targets
    }
}

/// The page size one paginated read gets: its `MaxItems`/`MaxResults` when it names one,
/// else `default`, clamped to [`MAX_PAGE_SIZE`] — the page these operations document they
/// paginate at — so one request cannot amplify into the whole store.
pub fn page_size(max_items: Option<i32>, default: usize) -> usize {
    match max_items {
        Some(requested) => usize::try_from(requested)
            .unwrap_or(default)
            .clamp(1, MAX_PAGE_SIZE),
        None => default.min(MAX_PAGE_SIZE),
    }
}

/// The resource segment an EFS file-system ARN carries
/// (`arn:aws:elasticfilesystem:<region>:<account>:file-system/fs-…`), which is also how
/// an ARN form is told apart from a bare id.
pub const FILE_SYSTEM_ARN_PREFIX: &str = "file-system/";
/// The resource segment an EFS access-point ARN carries; see [`FILE_SYSTEM_ARN_PREFIX`].
pub const ACCESS_POINT_ARN_PREFIX: &str = "access-point/";
/// The prefix of a bare access point id (`fsap-…`). A file system id is `fs-…`, which
/// this is NOT a prefix of, so the two id forms are distinguishable.
pub const ACCESS_POINT_ID_PREFIX: &str = "fsap-";

/// A `FileSystemId` member accepts "either a file system ID or ARN", so reduce either
/// form to the bare `fs-…` id the state is keyed by.
pub fn file_system_id_of(value: &str) -> &str {
    match value.rsplit_once(FILE_SYSTEM_ARN_PREFIX) {
        Some((_, id)) => id,
        None => value,
    }
}

/// The Region segment of an ARN: `arn:<partition>:<service>:<region>:<account>:<resource>`
/// — the Region a resource this emulator minted an ARN for lives in.
pub fn arn_region(arn: &str) -> Option<&str> {
    arn.split(':').nth(3).filter(|region| !region.is_empty())
}

/// An `AccessPointId` member accepts an id or an ARN; reduce either to the bare id.
pub fn access_point_id_of(value: &str) -> &str {
    match value.rsplit_once(ACCESS_POINT_ARN_PREFIX) {
        Some((_, id)) => id,
        None => value,
    }
}

/// The EFS resource a tag-addressed `ResourceId` names, reduced to the bare id the store
/// is keyed by. "You can retrieve tags for EFS file systems and access points using this
/// API endpoint" (`ListTagsForResource$ResourceId`), and the member's modelled pattern
/// admits exactly those two resources, each as a bare id or as its ARN:
/// `^(arn:aws[-a-z]*:elasticfilesystem:[0-9a-z-:]+:(access-point/fsap|file-system/fs)-[0-9a-f]{8,40}|fs(ap)?-[0-9a-f]{8,40})$`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedResource<'a> {
    /// A `fs-…` file system, keyed in [`ComAmazonawsEfsMagnolioapiserviceV20150201State::file_systems`].
    FileSystem(&'a str),
    /// A `fsap-…` access point, keyed in
    /// [`ComAmazonawsEfsMagnolioapiserviceV20150201State::access_points`].
    AccessPoint(&'a str),
}

/// Which resource a tag-addressed `ResourceId` names: the access-point forms are the
/// `access-point/fsap-…` ARN and the `fsap-…` id, and every other form the modelled
/// pattern admits names a file system.
///
/// One construction for every tag operation of this service, so a tag write and a later
/// tag read can never resolve the same `ResourceId` to two different records.
pub fn tagged_resource(resource_id: &str) -> TaggedResource<'_> {
    if resource_id.contains(ACCESS_POINT_ARN_PREFIX)
        || resource_id.starts_with(ACCESS_POINT_ID_PREFIX)
    {
        TaggedResource::AccessPoint(access_point_id_of(resource_id))
    } else {
        TaggedResource::FileSystem(file_system_id_of(resource_id))
    }
}

/// The tag-carrying parts of one stored resource, for a tag WRITE: its tag set and the
/// `Name` its description reports, which is derived from the `Name` tag
/// (`FileSystemDescription$Name`: "If the file system has a Name tag, Amazon EFS returns
/// the value in this field"; `AccessPointDescription$Name`: "The name of the access
/// point. This is the value of the Name tag.").
///
/// Handed out by [`ComAmazonawsEfsMagnolioapiserviceV20150201State::tagged_record_mut`]
/// so a tag write touches both through one construction and the reported `Name` cannot
/// drift from the stored `Name` tag.
#[derive(Debug)]
pub struct TaggedRecordMut<'a> {
    pub tags: &'a mut Vec<Tag>,
    pub name: &'a mut Option<String>,
}

impl TaggedRecordMut<'_> {
    /// Add `requested` to this resource's tag set by [`merge_tags`], then re-derive the
    /// reported `Name` from the resulting set.
    pub fn apply(&mut self, requested: &[Tag]) {
        merge_tags(self.tags, requested);
        *self.name = name_tag(self.tags);
    }

    /// Remove every tag whose key is in `keys`, then re-derive the reported `Name` from
    /// the resulting set — so removing the `Name` tag also clears the `Name` the
    /// description reports.
    ///
    /// A key the resource does not carry leaves the set as it is: the request asks for
    /// the key to be gone, which it already is, so a repeated removal is idempotent.
    pub fn remove(&mut self, keys: &[String]) {
        self.tags.retain(|tag| !keys.contains(&tag.key));
        *self.name = name_tag(self.tags);
    }
}

/// The Region a request is attributed to: its SigV4 credential scope, which is the
/// Region an AWS SDK signed the call for, else [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Split an Availability Zone name into its Region code and its zone letter. An AZ name
/// is the Region code followed by a letter identifier ("Regions and Zones", Amazon EC2
/// User Guide).
pub fn split_availability_zone_name(az_name: &str) -> Option<(&str, char)> {
    let suffix = az_name.chars().next_back()?;
    if !AZ_SUFFIXES.contains(&suffix) {
        return None;
    }
    let region = az_name.get(..az_name.len() - suffix.len_utf8())?;
    (!region.is_empty()).then_some((region, suffix))
}

/// The Availability Zone ID for an Availability Zone name: `<region abbreviation>-az<n>`
/// — `us-east-1a` -> `use1-az1`, `ap-southeast-1b` -> `apse1-az2`, `eu-central-1c` ->
/// `euc1-az3` ("Availability Zone IDs for your resources", AWS Resource Access Manager
/// User Guide).
pub fn availability_zone_id(az_name: &str) -> String {
    let (region, index) = match split_availability_zone_name(az_name) {
        Some((region, suffix)) => (
            region,
            AZ_SUFFIXES
                .iter()
                .position(|candidate| *candidate == suffix)
                .map_or(1, |position| position + 1),
        ),
        None => (az_name, 1),
    };
    format!("{}-az{index}", region_abbreviation(region))
}

/// The Region abbreviation an Availability Zone ID is prefixed with: the leading
/// geography code whole, then each following word reduced to the initials of its
/// compound-direction parts, with a numeric segment appended as it stands — `us-east-1`
/// -> `use1`, `ap-southeast-1` -> `apse1`, `eu-central-1` -> `euc1`, `us-gov-west-1` ->
/// `usgw1`.
fn region_abbreviation(region: &str) -> String {
    let mut abbreviation = String::new();
    for (index, segment) in region.split('-').enumerate() {
        if index == 0 || segment.bytes().all(|byte| byte.is_ascii_digit()) {
            abbreviation.push_str(segment);
        } else {
            abbreviation.push_str(&compound_initials(segment));
        }
    }
    abbreviation
}

/// The initials of a direction word's compound parts: `southeast` -> `se`, `east` ->
/// `e`, `central` -> `c`. A word that is not built from those parts contributes its own
/// first letter (`gov` -> `g`).
fn compound_initials(word: &str) -> String {
    /// The direction words an AWS Region code is built from.
    const PARTS: [&str; 5] = ["north", "south", "east", "west", "central"];

    let mut initials = String::new();
    let mut rest = word;
    while !rest.is_empty() {
        match PARTS.iter().find(|part| rest.starts_with(**part)) {
            Some(part) => {
                if let Some(first) = part.chars().next() {
                    initials.push(first);
                }
                rest = &rest[part.len()..];
            }
            None => {
                if let Some(first) = rest.chars().next() {
                    initials.push(first);
                }
                break;
            }
        }
    }
    initials
}

/// The `alias/` resource prefix a KMS alias carries, bare or inside an ARN, which is how
/// an alias is told apart from the key it points at. Both forms appear in the modelled
/// `KmsKeyId` pattern (`smithy_query(query="shape",shape="KmsKeyId")`).
pub const KMS_ALIAS_PREFIX: &str = "alias/";

/// The seed a KMS key id derived from an alias name is finalised from. A fixed salt, so
/// a derived key id can never collide with an id this store minted from its own counter.
const KMS_ALIAS_SEED: u64 = 0x4B4D_5341_4C49_4153;

/// The KEY IDENTIFIER EFS reports for an encrypted file system's KMS key.
///
/// `CreateFileSystem$KmsKeyId` accepts "the ID of the KMS key that you want to use" as a
/// key id, a key ARN, an alias or an alias ARN, and `FileSystemDescription$KmsKeyId` is
/// "the ID of an KMS key used to protect the encrypted file system" — the key itself. An
/// alias is a pointer to a key and not a key, so an alias in the request is RESOLVED to
/// the key ARN of the key it names (`arn:aws:kms:<region>:<account>:key/<key id>`, one of
/// the forms the modelled pattern admits), rather than echoed: a client that feeds the
/// reported value to KMS or pins it against a configured CMK needs the key.
///
/// The key an alias resolves to is derived from the alias name, so one alias always names
/// the same key and two aliases never name the same one, without this service standing up
/// a KMS key store of its own.
pub fn kms_key_arn(key: &str, region: &str) -> String {
    let is_arn = key.starts_with("arn:");
    // An ARN names the Region its key lives in; a bare form lives in the request's.
    let region = if is_arn {
        arn_region(key).unwrap_or(region)
    } else {
        region
    };
    // The resource segment of an ARN (`key/…` or `alias/…`) is its last colon field.
    let resource = if is_arn {
        key.rsplit(':').next().unwrap_or(key)
    } else {
        key
    };
    match resource.strip_prefix(KMS_ALIAS_PREFIX) {
        Some(alias_name) => ::engine::emulator_arn(
            "kms",
            region,
            &format!("key/{}", kms_key_id_of_alias(alias_name)),
        ),
        // A key ARN already names the key, in the account that owns it.
        None if is_arn => key.to_string(),
        None => ::engine::emulator_arn("kms", region, &format!("key/{key}")),
    }
}

/// The key id the KMS alias `alias_name` resolves to: `8-4-4-4-12` lowercase hex digits,
/// the key-id form the modelled `KmsKeyId` pattern admits.
///
/// Derived from the alias name through [`digest_of`] and [`hex_of`], so the resolution is
/// stable — every read of a file system created under one alias reports the same key — and
/// injective in practice, so distinct aliases resolve to distinct keys.
fn kms_key_id_of_alias(alias_name: &str) -> String {
    let seed = KMS_ALIAS_SEED ^ digest_of(alias_name);
    let digits = format!(
        "{}{}",
        hex_of(seed),
        hex_of(seed.wrapping_add(SPLITMIX64_STEP))
    );
    format!(
        "{}-{}-{}-{}-{}",
        &digits[..8],
        &digits[8..12],
        &digits[12..16],
        &digits[16..20],
        &digits[20..32]
    )
}

/// A 64-bit digest of `value`, from the hasher the standard library ships: a name a
/// request carries derives the same emulated resource on every call, with no hash
/// algorithm written here and no dependency added for one.
fn digest_of(value: &str) -> u64 {
    use ::std::hash::{Hash, Hasher};

    let mut hasher = ::std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// A value a request named, as EFS renders it inside an error message: in single
/// quotes, so the value can be lifted back out of the sentence by a client or a log
/// reader. Recorded EFS errors show the construction on both the id of a resource and
/// the creation token of a request ("File system 'fs-…' does not exist.", "File system
/// 'fs-…' already exists with creation token '…'").
pub fn quoted(value: &str) -> String {
    format!("'{value}'")
}

/// The noun EFS names a file system by in its messages; see [`does_not_exist`].
pub const FILE_SYSTEM_NOUN: &str = "File system";
/// The noun EFS names a mount target by in its messages; see [`does_not_exist`].
pub const MOUNT_TARGET_NOUN: &str = "Mount target";
/// The noun EFS names an access point by in its messages; see [`does_not_exist`].
pub const ACCESS_POINT_NOUN: &str = "Access point";
/// The noun EFS names a subnet by in its messages; see [`does_not_exist`].
pub const SUBNET_NOUN: &str = "Subnet";

/// The single construction behind every "not found" message this service emits:
/// `<Noun> '<id>' does not exist.` — the noun of the resource the request asked for,
/// then the id the REQUEST named in single quotes ([`quoted`]), in a sentence
/// terminated with a period.
///
/// Built here once rather than per handler, so the same request-derived sentence is
/// reached through every entry point that resolves an id (describe, delete, create).
pub fn does_not_exist(noun: &str, id: &str) -> String {
    format!("{noun} {} does not exist.", quoted(id))
}

/// `FileSystemNotFound`'s message — "Returned if the specified `FileSystemId` value
/// doesn't exist in the requester's Amazon Web Services account": the requested
/// `FileSystemId`, rendered by [`does_not_exist`].
pub fn file_system_not_found_message(file_system_id: &str) -> String {
    does_not_exist(FILE_SYSTEM_NOUN, file_system_id)
}

/// `MountTargetNotFound`'s message — "Returned if there is no mount target with the
/// specified ID found in the caller's Amazon Web Services account".
pub fn mount_target_not_found_message(mount_target_id: &str) -> String {
    does_not_exist(MOUNT_TARGET_NOUN, mount_target_id)
}

/// `AccessPointNotFound`'s message — "Returned if the specified `AccessPointId` value
/// doesn't exist in the requester's Amazon Web Services account".
pub fn access_point_not_found_message(access_point_id: &str) -> String {
    does_not_exist(ACCESS_POINT_NOUN, access_point_id)
}

/// `ReplicationNotFound`'s message on the replication READ
/// (`DescribeReplicationConfigurations`) — the refusal reports that the request's filter
/// matched no replication configuration, so the sentence is FIXED text: it interpolates
/// nothing from the request or from state (no file-system id, no member name), and is
/// terminated with a period.
pub const NO_REPLICATIONS_FOUND_MESSAGE: &str = "No replications found.";

/// `ReplicationNotFound`'s message on `DeleteReplicationConfiguration` — the refusal
/// names the file system whose configuration could not be found, so the sentence is
/// "Cannot find a replication configuration for file system " plus the `FileSystemId`
/// the REQUEST named (bare, not [`quoted`]), terminated with a period.
pub fn replication_configuration_not_found_message(file_system_id: &str) -> String {
    format!("Cannot find a replication configuration for file system {file_system_id}.")
}

/// `FileSystemAlreadyExists`' message — "Returned if the file system you are trying to
/// create already exists, with the creation token you provided". The one piece of
/// actionable data is WHICH file system already holds the token, so the message names
/// the existing record's id and the `CreationToken` the request carried, each
/// single-quoted ([`quoted`]).
pub fn file_system_already_exists_message(
    existing_file_system_id: &str,
    creation_token: &str,
) -> String {
    format!(
        "{FILE_SYSTEM_NOUN} {} already exists with creation token {}",
        quoted(existing_file_system_id),
        quoted(creation_token)
    )
}

/// The `BadRequest` sentence every throughput request that resolves to the
/// `provisioned` mode ([`THROUGHPUT_MODE_PROVISIONED`]) and carries no
/// `ProvisionedThroughputInMibps` — neither on the request nor already on the record —
/// is refused with. EFS states the rule in prose ("Required if ThroughputMode is set to
/// provisioned") and the sentence that announces it is FIXED text: it interpolates
/// nothing from the request or from state (no file-system id, no member name, no limit),
/// and is terminated with a period.
///
/// Held here once rather than per handler, so `CreateFileSystem` and `UpdateFileSystem`
/// — the two entry points that resolve a throughput mode — announce the one rule with
/// the one sentence.
pub const PROVISIONED_THROUGHPUT_REQUIRED_MESSAGE: &str =
    "Provisioned throughput must be set for file systems that use provisioned throughput mode.";

/// Why a caller-supplied tag list cannot be stored, as the `BadRequest` sentence to
/// refuse with — `None` when the list is storable. Two rules, one per resource this
/// service tags: the list holds at most [`MAX_TAGS_PER_RESOURCE`] tags, and "each key
/// must be unique" (`Tags`). The modelled list carries no length bound, so the quota is
/// checked first and the key scan then runs over a bounded list through a set, not a
/// nested loop.
///
/// Built here once rather than per handler, so every operation that accepts a tag list
/// refuses it with the same sentence.
pub fn tag_list_problem(tags: &[Tag]) -> Option<String> {
    if tags.len() > MAX_TAGS_PER_RESOURCE {
        return Some(format!(
            "Tags must hold at most {MAX_TAGS_PER_RESOURCE} tags; got {}",
            tags.len()
        ));
    }
    let mut keys = BTreeSet::new();
    for tag in tags {
        if !keys.insert(tag.key.as_str()) {
            return Some(format!(
                "Tags must each have a unique Key; Key {} appears more than once",
                tag.key
            ));
        }
    }
    None
}

/// Why a tag write cannot be stored, as the `BadRequest` sentence to refuse it with —
/// `None` when it can be. Two rules, applied to the set the write LEAVES rather than to
/// the request alone: the caller-supplied list must itself be storable
/// ([`tag_list_problem`]), and the resulting set must stay inside the published quota. A
/// requested key the resource already carries replaces a value instead of adding a tag,
/// so only the keys that are new count towards the quota.
///
/// Built here once rather than per handler, so `TagResource` and the deprecated
/// `CreateTags` — two write paths onto ONE stored tag set — refuse the same write the
/// same way.
pub fn tag_write_problem(held: &[Tag], requested: &[Tag], resource_id: &str) -> Option<String> {
    if let Some(problem) = tag_list_problem(requested) {
        return Some(problem);
    }
    let added = requested
        .iter()
        .filter(|tag| !held.iter().any(|tag_held| tag_held.key == tag.key))
        .count();
    let resulting = held.len() + added;
    (resulting > MAX_TAGS_PER_RESOURCE).then(|| {
        format!(
            "resource {} would hold {resulting} tags, more than the maximum of {}",
            quoted(resource_id),
            MAX_TAGS_PER_RESOURCE
        )
    })
}

/// The tag key whose value a resource's reported `Name` is: "Name your file system on
/// creation by including a "Key":"Name","Value":"{value}" key-value pair"
/// (`CreateFileSystem$Tags`), and an access point's `Name` "is the value of the Name
/// tag".
pub const NAME_TAG_KEY: &str = "Name";

/// The `Name` a description reports for a resource holding `tags`: the value of its
/// [`NAME_TAG_KEY`] tag, and `None` while it carries none.
///
/// One construction, reached by every operation that stores a tag set, so a create and a
/// later tag write report the member the same way.
pub fn name_tag(tags: &[Tag]) -> Option<String> {
    tags.iter()
        .find(|tag| tag.key == NAME_TAG_KEY)
        .map(|tag| tag.value.clone())
}

/// Add a caller-supplied tag list to a stored one, in place.
///
/// A tag key identifies a tag on a resource — "each key must be unique" (`Tags`) — so a
/// requested key the resource already carries REPLACES that key's value and a key it does
/// not carry is appended, leaving the order the keys were first set in untouched. A
/// request may therefore be replayed without growing the set.
pub fn merge_tags(stored: &mut Vec<Tag>, requested: &[Tag]) {
    for tag in requested {
        match stored.iter_mut().find(|held| held.key == tag.key) {
            Some(held) => held.value = tag.value.clone(),
            None => stored.push(tag.clone()),
        }
    }
}

/// A `SystemTime` as the modelled `Timestamp`. EFS reports a file system's
/// `CreationTime` and its `SizeInBytes.Timestamp` as "the integer number of seconds
/// since 1970-01-01T00:00:00Z".
pub fn epoch_seconds(time: SystemTime) -> ::aws_smithy_types::DateTime {
    let seconds = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    ::aws_smithy_types::DateTime::from_secs(seconds)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEfsMagnolioapiserviceV20150201State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEfsMagnolioapiserviceV20150201State>)
}
