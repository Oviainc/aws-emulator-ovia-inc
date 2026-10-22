// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-efs-magnolioapiservice-v20150201 service.
//!
//! One lock-guarded struct per engine, reached through [`service_state`]. It holds the
//! file systems, the creation tokens they were made idempotent by, their mount targets,
//! and the VPC/subnet facts a mount target description is built from — plus the counter
//! every `fs-`/`fsmt-`/`eni-`/`vpc-` id is minted from.
//!
//! The record types here are the single source of every description this service
//! returns: `FileSystem::describe` and `MountTarget::describe` build the modelled
//! output shape, so a create and a later describe cannot disagree.
//!
//! The request-derived error MESSAGES are built here too ([`does_not_exist`],
//! [`file_system_already_exists_message`]), for the same reason: every operation that
//! resolves an id refuses with one sentence construction instead of its own.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    CreateFileSystemOutput, CreateMountTargetOutput, FileSystemSize, LifecyclePolicy, Tag,
};

/// Values of the modelled `LifeCycleState` enum this slice produces
/// (`smithy_query(query="shape",shape="LifeCycleState")` -> creating | available |
/// updating | deleting | deleted | error).
pub const LIFE_CYCLE_CREATING: &str = "creating";
/// See [`LIFE_CYCLE_CREATING`].
pub const LIFE_CYCLE_AVAILABLE: &str = "available";

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
/// change", Amazon EFS User Guide). The modelled `Tags` list carries no length bound, so
/// a handler enforces the quota itself before it walks a caller-supplied list.
pub const MAX_TAGS_PER_FILE_SYSTEM: usize = 50;

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
/// Both describe operations are "paginated at 100 per page", so a larger `MaxItems` is
/// clamped rather than letting one request walk the whole store.
pub const MAX_PAGE_SIZE: usize = 100;

/// How long a resource this service creates stays in `creating` before the clock owes
/// it `available`. AWS publishes no duration for the transition, so this span is the
/// emulator's own choice inside the band the project fixes for a clock-owed
/// transition (250-750 ms). It is never zero, so a caller that acts immediately after
/// a create still observes `creating` and the state errors that go with it; the due
/// instant is stored on the record and materialised on the next call, never slept for.
pub const CREATING_FOR: Duration = Duration::from_millis(500);

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
    /// When the clock owes this file system `available`; see [`CREATING_FOR`].
    pub available_at: SystemTime,
    pub life_cycle_state: String,
    pub name: Option<String>,
    pub performance_mode: String,
    pub throughput_mode: String,
    pub provisioned_throughput_in_mibps: Option<f64>,
    pub encrypted: Option<bool>,
    pub kms_key_id: Option<String>,
    pub availability_zone_name: Option<String>,
    pub availability_zone_id: Option<String>,
    /// Whether automatic backups were requested at creation. Read by the backup-policy
    /// operations, which are still scaffolded.
    pub backup: bool,
    /// The last known metered size, in bytes, and when it was metered.
    pub size_in_bytes: i64,
    pub size_measured_at: SystemTime,
    pub tags: Vec<Tag>,
    pub mount_target_ids: Vec<String>,
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
            size_in_bytes: FileSystemSize {
                value: self.size_in_bytes,
                timestamp: Some(epoch_seconds(self.size_measured_at)),
                value_in_ia: Some(0),
                value_in_standard: Some(self.size_in_bytes),
                value_in_archive: None,
            },
            performance_mode: self.performance_mode.clone(),
            encrypted: self.encrypted,
            kms_key_id: self.kms_key_id.clone(),
            throughput_mode: Some(self.throughput_mode.clone()),
            provisioned_throughput_in_mibps: self.provisioned_throughput_in_mibps,
            availability_zone_name: self.availability_zone_name.clone(),
            availability_zone_id: self.availability_zone_id.clone(),
            tags: self.tags.clone(),
            file_system_protection: None,
        }
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
    /// resource whose `creating` span has elapsed becomes `available`. Owed at the same
    /// instant whether or not anybody called, so a reporting operation may apply it.
    pub fn materialize_due_transitions(&mut self, now: SystemTime) {
        for file_system in self.file_systems.values_mut() {
            if file_system.life_cycle_state == LIFE_CYCLE_CREATING
                && file_system.available_at <= now
            {
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

/// The page size one describe request gets: its `MaxItems` when it names one, else
/// `default`, clamped to [`MAX_PAGE_SIZE`] — the page both operations document they
/// paginate at — so one request cannot amplify into the whole store.
pub fn page_size(max_items: Option<i32>, default: usize) -> usize {
    match max_items {
        Some(requested) => usize::try_from(requested)
            .unwrap_or(default)
            .clamp(1, MAX_PAGE_SIZE),
        None => default.min(MAX_PAGE_SIZE),
    }
}

/// A `FileSystemId` member accepts "either a file system ID or ARN", so reduce either
/// form to the bare `fs-…` id the state is keyed by.
pub fn file_system_id_of(value: &str) -> &str {
    match value.rsplit_once("file-system/") {
        Some((_, id)) => id,
        None => value,
    }
}

/// An `AccessPointId` member accepts an id or an ARN; reduce either to the bare id.
pub fn access_point_id_of(value: &str) -> &str {
    match value.rsplit_once("access-point/") {
        Some((_, id)) => id,
        None => value,
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

/// The key ARN EFS reports for an encrypted file system's KMS key. A `KmsKeyId` may
/// arrive as a key id, a key ARN, an alias or an alias ARN
/// (`CreateFileSystem$KmsKeyId`); the description carries the ARN.
pub fn kms_key_arn(key: &str, region: &str) -> String {
    if key.starts_with("arn:") {
        return key.to_string();
    }
    let resource = if key.starts_with("alias/") {
        key.to_string()
    } else {
        format!("key/{key}")
    };
    ::engine::emulator_arn("kms", region, &resource)
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
