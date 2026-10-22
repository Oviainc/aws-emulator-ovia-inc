// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-docdb-amazonrdsv19 service.
//!
//! This is the Amazon DocumentDB control plane the implemented operations read and
//! write: the cluster records, the instance records that are a cluster's members, and
//! the final cluster snapshots `DeleteDBCluster` takes. It also holds the pieces more
//! than one handler needs — the documented input rules (`validate_*`), the documented
//! defaults, and the record-to-wire-shape projections — so no two handlers can disagree
//! about them.
//!
//! Every constant that encodes an AWS-published value names the model member whose
//! documentation states it; read it with
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-docdb-amazonrdsv19/smithy-model/com-amazonaws-docdb-amazonrdsv19.json")
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use crate::ops::types::{
    DBCluster, DBClusterMember, DBClusterParameterGroup, DBClusterSnapshot, DBInstance,
    DBSubnetGroup, Endpoint, ServerlessV2ScalingConfigurationInfo, Subnet, Tag,
    VpcSecurityGroupMembership,
};

/// The ARN namespace and sigv4 signing name DocumentDB shares with RDS, so a cluster ARN
/// reads `arn:aws:rds:<region>:<account>:cluster:<id>` (`OPERATIONS.md`, "Wire protocol").
pub const ARN_NAMESPACE: &str = "rds";

/// `CreateDBClusterMessage$Engine`: "Valid values: docdb".
pub const DOCDB_ENGINE: &str = "docdb";

/// `CreateDBClusterMessage$EngineVersion`: "The --engine-version will default to the
/// latest major engine version." Derived from the catalogue rather than restated, so a
/// version added to [`DOCDB_ENGINE_VERSIONS`] moves the default with it.
pub const DEFAULT_ENGINE_VERSION: &str = DOCDB_ENGINE_VERSIONS[DOCDB_ENGINE_VERSIONS.len() - 1];

/// The port DocumentDB listens on when `Port` is absent (the engine's default port, and
/// the one `aws docdb create-db-cluster` reports back for a cluster created without it).
pub const DEFAULT_PORT: i32 = 27017;

/// `CreateDBClusterMessage$BackupRetentionPeriod`: "Default: 1 Constraints: Must be a
/// value from 1 to 35."
pub const DEFAULT_BACKUP_RETENTION_PERIOD: i32 = 1;
/// See [`DEFAULT_BACKUP_RETENTION_PERIOD`].
pub const MIN_BACKUP_RETENTION_PERIOD: i32 = 1;
/// See [`DEFAULT_BACKUP_RETENTION_PERIOD`].
pub const MAX_BACKUP_RETENTION_PERIOD: i32 = 35;

/// `DescribeDBClustersMessage$MaxRecords` / `DescribeDBInstancesMessage$MaxRecords`:
/// "Default: 100 Constraints: Minimum 20, maximum 100."
pub const DEFAULT_MAX_RECORDS: i32 = 100;
/// See [`DEFAULT_MAX_RECORDS`].
pub const MIN_MAX_RECORDS: i32 = 20;
/// See [`DEFAULT_MAX_RECORDS`].
pub const MAX_MAX_RECORDS: i32 = 100;

/// `CreateDBInstanceMessage$PromotionTier`: "Default: 1 Valid values: 0-15".
pub const DEFAULT_PROMOTION_TIER: i32 = 1;
/// See [`DEFAULT_PROMOTION_TIER`].
pub const MIN_PROMOTION_TIER: i32 = 0;
/// See [`DEFAULT_PROMOTION_TIER`].
pub const MAX_PROMOTION_TIER: i32 = 15;

/// `CreateDBClusterMessage$DBClusterIdentifier`, `CreateDBInstanceMessage$DBInstanceIdentifier`
/// and `ModifyDBClusterMessage$NewDBClusterIdentifier` all state the same rule: "Must
/// contain from 1 to 63 letters, numbers, or hyphens. The first character must be a
/// letter. Cannot end with a hyphen or contain two consecutive hyphens."
pub const MAX_IDENTIFIER_LEN: usize = 63;

/// `DeleteDBClusterMessage$FinalDBSnapshotIdentifier` states the same rule with a longer
/// bound: "Must be from 1 to 255 letters, numbers, or hyphens. The first character must
/// be a letter. Cannot end with a hyphen or contain two consecutive hyphens."
pub const MAX_SNAPSHOT_IDENTIFIER_LEN: usize = 255;

/// `CreateDBClusterMessage$MasterUsername`: "Must be from 1 to 63 letters or numbers. The
/// first character must be a letter."
pub const MAX_MASTER_USERNAME_LEN: usize = 63;

/// `CreateDBClusterMessage$MasterUserPassword`: "Must contain from 8 to 100 characters."
pub const MIN_MASTER_USER_PASSWORD_LEN: usize = 8;
/// See [`MIN_MASTER_USER_PASSWORD_LEN`].
pub const MAX_MASTER_USER_PASSWORD_LEN: usize = 100;

/// `CreateDBClusterMessage$MasterUserPassword`: the password "can contain any printable
/// ASCII character except forward slash (/), double quote (\"), or the \"at\" symbol (@)".
pub const MASTER_USER_PASSWORD_FORBIDDEN: [char; 3] = ['/', '"', '@'];

/// `CreateDBClusterMessage$StorageType`: "Valid values for storage type - standard |
/// iopt1 Default value is standard ... The storage type isn't returned when you set it to
/// standard."
pub const DEFAULT_STORAGE_TYPE: &str = "standard";

/// The status a resource carries until the clock makes it available.
pub const STATUS_CREATING: &str = "creating";
/// The status of a cluster or instance that finished provisioning.
pub const STATUS_AVAILABLE: &str = "available";
/// The status returned for a resource the caller just deleted.
pub const STATUS_DELETING: &str = "deleting";

/// `DBClusterMember$DBClusterParameterGroupStatus` for a member whose parameter group is
/// the one its cluster carries.
pub const PARAMETER_GROUP_IN_SYNC: &str = "in-sync";

/// `VpcSecurityGroupMembership$Status` for a security group attached to a live resource.
pub const SECURITY_GROUP_ACTIVE: &str = "active";

/// `DBClusterSnapshot$SnapshotType` for the final snapshot `DeleteDBCluster` takes: AWS
/// classes a snapshot the caller asked for as `manual`.
pub const SNAPSHOT_TYPE_MANUAL: &str = "manual";

/// The region used when a request carries no SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The engine versions the `docdb` engine offers, oldest first. These are the versions
/// Amazon DocumentDB publishes for the engine ("Amazon DocumentDB release notes"; they are
/// also what `DescribeDBEngineVersions` reports), and they are the catalogue
/// [`resolve_engine_version`] answers from: a request naming a version that is not in it
/// names a version the engine cannot run.
pub const DOCDB_ENGINE_VERSIONS: [&str; 3] = ["3.6.0", "4.0.0", "5.0.0"];

/// `CreateDBSubnetGroupMessage$DBSubnetGroupName`: "Must contain no more than 255 letters,
/// numbers, periods, underscores, spaces, or hyphens."
pub const MAX_SUBNET_GROUP_NAME_LEN: usize = 255;

/// `CreateDBSubnetGroupMessage$DBSubnetGroupName` and
/// `DeleteDBSubnetGroupMessage$DBSubnetGroupName` both state it: "Must not be default".
pub const RESERVED_SUBNET_GROUP_NAME: &str = "default";

/// `CreateDBSubnetGroup`: "subnet groups must contain at least one subnet in at least two
/// Availability Zones in the Amazon Web Services Region."
///
/// Which Availability Zone a subnet sits in is EC2's to say, and this emulator has no
/// subnet inventory to read it from, so the rule is enforced at the granularity the request
/// itself carries: fewer than this many distinct subnets cannot cover two zones. A group
/// that does clear the bound reports its subnets without inventing a zone for them (see
/// [`DbSubnetGroupRecord::to_shape`]).
pub const MIN_SUBNETS_PER_SUBNET_GROUP: usize = 2;

/// `DBSubnetGroup$SubnetGroupStatus` for a group whose subnets are all usable.
pub const SUBNET_GROUP_COMPLETE: &str = "Complete";

/// `Subnet$SubnetStatus` for a subnet that is in service.
pub const SUBNET_ACTIVE: &str = "Active";

/// The default-parameter-group name prefix AWS uses: a default cluster parameter group is
/// named `default.<family>` (`default.docdb5.0`), one per engine-version family, and exists
/// in every account and Region without anybody creating it.
pub const DEFAULT_PARAMETER_GROUP_PREFIX: &str = "default.";

// ---------------------------------------------------------------------------
// Store bounds
//
// Every record below is created by a caller and reclaimed only by an explicit delete,
// so each growth site carries a cap and nothing caller-sized reaches the store
// unchecked. The caps are Amazon DocumentDB's own published default quotas ("Amazon
// DocumentDB quotas and limits" in the Developer Guide) where it publishes one; where
// it publishes none the constant says so and the value is this emulator's resource
// guard, set well above any realistic use.
//
// The product is therefore finite: at most MAX_DB_CLUSTERS clusters, at most
// MAX_DB_INSTANCES instances (and MAX_DB_INSTANCES_PER_CLUSTER of them in any one
// cluster), at most MAX_DB_CLUSTER_SNAPSHOTS final snapshots, and each record's
// caller-sized lists capped at MAX_COLLECTION_ENTRIES entries (MAX_TAGS tags) of at
// most MAX_STRING_CHARS characters.
// ---------------------------------------------------------------------------

/// Amazon DocumentDB's published default quota "Clusters: 40" per Region. An over-quota
/// create is refused with the modelled `DBClusterQuotaExceededFault` rather than stored.
pub const MAX_DB_CLUSTERS: usize = 40;

/// Amazon DocumentDB's published quota "Instances per cluster: 16".
pub const MAX_DB_INSTANCES_PER_CLUSTER: usize = 16;

/// Amazon DocumentDB's published default quota "Instances: 40" per Region.
pub const MAX_DB_INSTANCES: usize = 40;

/// Amazon DocumentDB's published default quota "Manual cluster snapshots: 100" per
/// Region. This also bounds the one store with no reclamation path yet: the snapshot
/// operations that would delete a snapshot are still scaffolded, so a repeated
/// create/delete-with-final-snapshot loop stops at this cap with the modelled
/// `SnapshotQuotaExceededFault` instead of growing without limit.
pub const MAX_DB_CLUSTER_SNAPSHOTS: usize = 100;

/// Amazon DocumentDB's published default quota "Subnet groups: 50" per Region. An
/// over-quota create is refused with the modelled `DBSubnetGroupQuotaExceededFault` rather
/// than stored.
pub const MAX_DB_SUBNET_GROUPS: usize = 50;

/// Amazon DocumentDB's published default quota "Subnets per subnet group: 20". A create
/// naming more is refused with the modelled `DBSubnetQuotaExceededFault`.
pub const MAX_SUBNETS_PER_SUBNET_GROUP: usize = 20;

/// AWS's tagging limit of 50 tags per resource.
pub const MAX_TAGS: usize = 50;

/// AWS's tagging limit: a tag key is at most 128 characters.
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// AWS's tagging limit: a tag value is at most 256 characters.
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// Emulator resource guard, NOT an AWS quota: the most entries this emulator will store
/// for a caller-sized list whose length the service publishes no limit for.
pub const MAX_COLLECTION_ENTRIES: usize = 50;

/// Emulator resource guard, NOT an AWS quota: the longest caller-supplied string this
/// emulator will store for a member whose length the service publishes no limit for.
pub const MAX_STRING_CHARS: usize = 512;

/// Refuse a caller-sized collection whose entry count would grow the store past `max`.
///
/// `Err` carries the message, naming the member and the bound it broke, for the caller
/// to fix the request from.
pub fn bound_entries(member: &str, count: usize, max: usize) -> Result<(), String> {
    if count > max {
        return Err(format!(
            "{member} may carry at most {max} entries, got {count}"
        ));
    }
    Ok(())
}

/// Refuse a caller-supplied string longer than `max` characters; see [`bound_entries`].
pub fn bound_chars(member: &str, value: &str, max: usize) -> Result<(), String> {
    let length = value.chars().count();
    if length > max {
        return Err(format!(
            "{member} may be at most {max} characters, got {length}"
        ));
    }
    Ok(())
}

/// Refuse a `Tags` list this emulator will not store: too many tags, or a key or value
/// past AWS's tagging limits.
pub fn bound_tags(tags: Option<&Vec<Tag>>) -> Result<(), String> {
    let Some(tags) = tags else {
        return Ok(());
    };
    bound_entries("Tags", tags.len(), MAX_TAGS)?;
    for tag in tags {
        if let Some(key) = &tag.key {
            bound_chars("Tags.Key", key, MAX_TAG_KEY_CHARS)?;
        }
        if let Some(value) = &tag.value {
            bound_chars("Tags.Value", value, MAX_TAG_VALUE_CHARS)?;
        }
    }
    Ok(())
}

/// Refuse a caller-sized string list this emulator will not store: too many entries, or
/// an entry past [`MAX_STRING_CHARS`].
pub fn bound_string_list(member: &str, values: Option<&Vec<String>>) -> Result<(), String> {
    let Some(values) = values else {
        return Ok(());
    };
    bound_entries(member, values.len(), MAX_COLLECTION_ENTRIES)?;
    for value in values {
        bound_chars(member, value, MAX_STRING_CHARS)?;
    }
    Ok(())
}

/// Refuse an optional free-form string past [`MAX_STRING_CHARS`].
pub fn bound_optional_string(member: &str, value: Option<&String>) -> Result<(), String> {
    match value {
        Some(value) => bound_chars(member, value, MAX_STRING_CHARS),
        None => Ok(()),
    }
}

/// How long after the create call the `creating` -> `available` transition is owed.
///
/// AWS takes minutes; this emulator owes it inside the window the project fixes for a
/// clock-driven transition, so a caller that acts immediately after the create still
/// observes `creating` and the state errors that go with it. Nothing ever sleeps: the due
/// instant is stored on the record and [`ComAmazonawsDocdbAmazonrdsv19State::settle_clock`]
/// materialises it on the next call that reports the resource.
pub const PROVISIONING_DELAY: Duration = Duration::from_millis(300);

/// Alphabet for a minted resource-id token: the uppercase letters and digits AWS uses in
/// a `DbClusterResourceId` / `DbiResourceId`, as a 32-symbol set so the token is a plain
/// base-32 rendering of the monotonic counter.
const TOKEN_ALPHABET: [u8; 32] = *b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Token length, matching the fixed-width tokens AWS mints.
const TOKEN_LEN: usize = 12;

/// One instance's membership of a cluster.
#[derive(Debug, Clone)]
pub struct ClusterMember {
    /// The member instance's identifier, the key into [`ComAmazonawsDocdbAmazonrdsv19State::instances`].
    pub db_instance_identifier: String,
    /// Whether this member is the cluster's writer (its primary).
    pub is_cluster_writer: bool,
    /// Failover promotion order; see [`DEFAULT_PROMOTION_TIER`].
    pub promotion_tier: i32,
}

/// A persisted Amazon DocumentDB cluster.
///
/// The identifier is stored lowercased (`DBClusterIdentifier`: "This parameter is stored
/// as a lowercase string"), and is also this record's key in
/// [`ComAmazonawsDocdbAmazonrdsv19State::clusters`]. The ARN and the two endpoint host
/// names are DERIVED from the identifier rather than stored, because renaming a cluster
/// through `ModifyDBCluster` changes all three.
#[derive(Debug, Clone)]
pub struct DbClusterRecord {
    /// The lowercased `DBClusterIdentifier`.
    pub identifier: String,
    /// `DbClusterResourceId`, minted once and stable across a rename.
    pub resource_id: String,
    /// The region whose credential scope the create was signed with; the ARN and the
    /// endpoint host names sit in it.
    pub region: String,
    /// The DNS suffix shared by this cluster's endpoints and its instances' endpoints:
    /// `<token>.<region>.docdb.amazonaws.com`.
    pub endpoint_suffix: String,
    /// `Status`.
    pub status: String,
    /// When the `creating` -> `available` transition falls due; `None` once settled.
    pub available_at: Option<Instant>,
    /// `Engine`.
    pub engine: String,
    /// `EngineVersion`.
    pub engine_version: String,
    /// `Port`.
    pub port: i32,
    /// `MasterUsername`.
    pub master_username: Option<String>,
    /// `ClusterCreateTime`.
    pub cluster_create_time: ::aws_smithy_types::DateTime,
    /// `BackupRetentionPeriod`.
    pub backup_retention_period: i32,
    /// `PreferredBackupWindow`.
    pub preferred_backup_window: String,
    /// `PreferredMaintenanceWindow`.
    pub preferred_maintenance_window: String,
    /// `StorageEncrypted`.
    pub storage_encrypted: bool,
    /// `KmsKeyId`.
    pub kms_key_id: Option<String>,
    /// `DeletionProtection`.
    pub deletion_protection: bool,
    /// `AvailabilityZones`.
    pub availability_zones: Vec<String>,
    /// The ids behind `VpcSecurityGroups`.
    pub vpc_security_group_ids: Vec<String>,
    /// `DBSubnetGroup`.
    pub db_subnet_group: Option<String>,
    /// `DBClusterParameterGroup`.
    pub db_cluster_parameter_group: Option<String>,
    /// `EnabledCloudwatchLogsExports`.
    pub enabled_cloudwatch_logs_exports: Vec<String>,
    /// `StorageType`, kept only when it is not the default; see [`DEFAULT_STORAGE_TYPE`].
    pub storage_type: Option<String>,
    /// `NetworkType`.
    pub network_type: Option<String>,
    /// `CopyTagsToSnapshot`.
    pub copy_tags_to_snapshot: bool,
    /// `ServerlessV2ScalingConfiguration` as (min capacity, max capacity).
    pub serverless_v2_scaling: Option<(f64, f64)>,
    /// `DBClusterMembers`, in the order the instances joined.
    pub members: Vec<ClusterMember>,
    /// The cluster's tags, as (key, value).
    pub tags: Vec<(String, String)>,
}

impl DbClusterRecord {
    /// `DBClusterArn`, derived so a rename moves it.
    pub fn arn(&self) -> String {
        ::engine::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("cluster:{}", self.identifier),
        )
    }

    /// `Endpoint`: the cluster (writer) endpoint.
    pub fn endpoint(&self) -> String {
        format!("{}.cluster-{}", self.identifier, self.endpoint_suffix)
    }

    /// `ReaderEndpoint`: the load-balanced reader endpoint.
    pub fn reader_endpoint(&self) -> String {
        format!("{}.cluster-ro-{}", self.identifier, self.endpoint_suffix)
    }

    /// Materialise the `creating` -> `available` transition if the clock already owes it.
    fn settle(&mut self, now: Instant) {
        if self.available_at.is_some_and(|due| now >= due) {
            self.status = STATUS_AVAILABLE.to_string();
            self.available_at = None;
        }
    }
}

/// A persisted Amazon DocumentDB instance: one member of exactly one cluster.
///
/// Like a cluster, the identifier is stored lowercased and is this record's key. Values a
/// member inherits from its cluster (port, backup settings, encryption, security groups)
/// are NOT copied here: they are read from the cluster when the instance is reported, so
/// a `ModifyDBCluster` is visible on the members too.
#[derive(Debug, Clone)]
pub struct DbInstanceRecord {
    /// The lowercased `DBInstanceIdentifier`.
    pub identifier: String,
    /// `DbiResourceId`.
    pub dbi_resource_id: String,
    /// The region whose credential scope the create was signed with.
    pub region: String,
    /// `DBInstanceClass`.
    pub instance_class: String,
    /// `Engine`.
    pub engine: String,
    /// `DBInstanceStatus`.
    pub status: String,
    /// When the `creating` -> `available` transition falls due; `None` once settled.
    pub available_at: Option<Instant>,
    /// The key of the cluster this instance belongs to.
    pub cluster_identifier: String,
    /// `AvailabilityZone`.
    pub availability_zone: String,
    /// `PromotionTier`.
    pub promotion_tier: i32,
    /// `Endpoint$Address`.
    pub endpoint_address: String,
    /// `InstanceCreateTime`.
    pub instance_create_time: ::aws_smithy_types::DateTime,
    /// `PreferredMaintenanceWindow`.
    pub preferred_maintenance_window: String,
    /// `AutoMinorVersionUpgrade`.
    pub auto_minor_version_upgrade: bool,
    /// `CopyTagsToSnapshot`.
    pub copy_tags_to_snapshot: bool,
    /// `PerformanceInsightsEnabled`.
    pub performance_insights_enabled: bool,
    /// `PerformanceInsightsKMSKeyId`.
    pub performance_insights_kms_key_id: Option<String>,
    /// `CACertificateIdentifier`.
    pub ca_certificate_identifier: Option<String>,
    /// The instance's tags, as (key, value).
    pub tags: Vec<(String, String)>,
}

impl DbInstanceRecord {
    /// `DBInstanceArn`.
    pub fn arn(&self) -> String {
        ::engine::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("db:{}", self.identifier),
        )
    }

    /// Materialise the `creating` -> `available` transition if the clock already owes it.
    fn settle(&mut self, now: Instant) {
        if self.available_at.is_some_and(|due| now >= due) {
            self.status = STATUS_AVAILABLE.to_string();
            self.available_at = None;
        }
    }
}

/// A persisted cluster snapshot.
///
/// `DeleteDBCluster` writes one when the caller does not skip the final snapshot, so the
/// snapshot the API says was taken really exists in state. The snapshot-reading
/// operations (`DescribeDBClusterSnapshots`, `CreateDBClusterSnapshot`, ...) are still
/// scaffolded and answer 501; they will read these records when they are implemented.
#[derive(Debug, Clone)]
pub struct DbClusterSnapshotRecord {
    /// `DBClusterSnapshotIdentifier`, lowercased, and this record's key. The member's own
    /// documentation does not repeat the "stored as a lowercase string" sentence the
    /// cluster and instance identifiers carry, but keying it the same way is what makes
    /// the duplicate check case-insensitive, as it is for every other identifier here.
    pub identifier: String,
    /// `DBClusterIdentifier` of the cluster the snapshot was taken from.
    pub cluster_identifier: String,
    /// The region the snapshot's ARN sits in.
    pub region: String,
    /// `SnapshotCreateTime`.
    pub snapshot_create_time: ::aws_smithy_types::DateTime,
    /// `ClusterCreateTime`, carried over from the cluster.
    pub cluster_create_time: ::aws_smithy_types::DateTime,
    /// `Engine`.
    pub engine: String,
    /// `EngineVersion`.
    pub engine_version: String,
    /// `MasterUsername`.
    pub master_username: Option<String>,
    /// `Port`.
    pub port: i32,
    /// `AvailabilityZones`.
    pub availability_zones: Vec<String>,
    /// `StorageEncrypted`.
    pub storage_encrypted: bool,
    /// `KmsKeyId`.
    pub kms_key_id: Option<String>,
    /// `StorageType`.
    pub storage_type: Option<String>,
    /// `SnapshotType`; see [`SNAPSHOT_TYPE_MANUAL`].
    pub snapshot_type: String,
}

impl DbClusterSnapshotRecord {
    /// `DBClusterSnapshotArn`.
    pub fn arn(&self) -> String {
        ::engine::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("cluster-snapshot:{}", self.identifier),
        )
    }

    /// This record as the modeled `DBClusterSnapshot`.
    pub fn to_shape(&self) -> DBClusterSnapshot {
        DBClusterSnapshot {
            availability_zones: Some(self.availability_zones.clone()),
            db_cluster_snapshot_identifier: Some(self.identifier.clone()),
            db_cluster_identifier: Some(self.cluster_identifier.clone()),
            snapshot_create_time: Some(self.snapshot_create_time),
            engine: Some(self.engine.clone()),
            status: Some(STATUS_AVAILABLE.to_string()),
            port: Some(self.port),
            vpc_id: None,
            cluster_create_time: Some(self.cluster_create_time),
            master_username: self.master_username.clone(),
            engine_version: Some(self.engine_version.clone()),
            snapshot_type: Some(self.snapshot_type.clone()),
            percent_progress: Some(100),
            storage_encrypted: Some(self.storage_encrypted),
            kms_key_id: self.kms_key_id.clone(),
            db_cluster_snapshot_arn: Some(self.arn()),
            source_db_cluster_snapshot_arn: None,
            storage_type: self.storage_type.clone(),
        }
    }
}

/// A persisted subnet group: the set of VPC subnets a cluster's instances may be placed
/// in. Created by `CreateDBSubnetGroup`, reported by `DescribeDBSubnetGroups`, named by
/// `CreateDBCluster`'s `DBSubnetGroupName`, and reclaimed by `DeleteDBSubnetGroup`.
///
/// The name is stored lowercased (`DBSubnetGroupName`: "This value is stored as a lowercase
/// string") and is this record's key in
/// [`ComAmazonawsDocdbAmazonrdsv19State::subnet_groups`]. The ARN is derived from the name
/// rather than stored.
#[derive(Debug, Clone)]
pub struct DbSubnetGroupRecord {
    /// The lowercased `DBSubnetGroupName`.
    pub name: String,
    /// `DBSubnetGroupDescription`.
    pub description: String,
    /// The region whose credential scope the create was signed with; the ARN sits in it.
    pub region: String,
    /// The `SubnetIds` the caller named, in request order.
    pub subnet_ids: Vec<String>,
    /// The group's tags, as (key, value).
    pub tags: Vec<(String, String)>,
}

impl DbSubnetGroupRecord {
    /// `DBSubnetGroupArn`, derived from the name.
    pub fn arn(&self) -> String {
        ::engine::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("subgrp:{}", self.name),
        )
    }

    /// This record as the modeled `DBSubnetGroup`.
    ///
    /// `VpcId` and `SupportedNetworkTypes` are EC2's to answer — the VPC a subnet belongs
    /// to, and whether that subnet carries IPv6 — and this emulator has no subnet
    /// inventory to read either from, so both members are omitted rather than invented.
    /// `SubnetAvailabilityZone` is omitted for the same reason.
    pub fn to_shape(&self) -> DBSubnetGroup {
        DBSubnetGroup {
            db_subnet_group_name: Some(self.name.clone()),
            db_subnet_group_description: Some(self.description.clone()),
            vpc_id: None,
            subnet_group_status: Some(SUBNET_GROUP_COMPLETE.to_string()),
            subnets: Some(
                self.subnet_ids
                    .iter()
                    .map(|id| Subnet {
                        subnet_identifier: Some(id.clone()),
                        subnet_availability_zone: None,
                        subnet_status: Some(SUBNET_ACTIVE.to_string()),
                    })
                    .collect(),
            ),
            db_subnet_group_arn: Some(self.arn()),
            supported_network_types: None,
        }
    }
}

/// Per-service state for com-amazonaws-docdb-amazonrdsv19. A handler reaches it through [`service_state`].
///
/// Every map is ordered, so a describe and the pagination marker it hands back are stable
/// across calls.
#[derive(Debug, Default)]
pub struct ComAmazonawsDocdbAmazonrdsv19State {
    /// Clusters, keyed by lowercased `DBClusterIdentifier`.
    pub clusters: BTreeMap<String, DbClusterRecord>,
    /// Instances, keyed by lowercased `DBInstanceIdentifier`.
    pub instances: BTreeMap<String, DbInstanceRecord>,
    /// Final cluster snapshots, keyed by `DBClusterSnapshotIdentifier`.
    pub snapshots: BTreeMap<String, DbClusterSnapshotRecord>,
    /// Subnet groups, keyed by lowercased `DBSubnetGroupName`.
    pub subnet_groups: BTreeMap<String, DbSubnetGroupRecord>,
    /// Monotonic counter behind every minted resource id, so ids are deterministic.
    pub next_resource_id: u64,
}

impl ComAmazonawsDocdbAmazonrdsv19State {
    /// Take the next value of the monotonic resource-id counter.
    pub fn next_resource_ordinal(&mut self) -> u64 {
        self.next_resource_id += 1;
        self.next_resource_id
    }

    /// Materialise every `creating` -> `available` transition the clock already owes.
    ///
    /// A reporting operation calls this: the transition was due at the same instant
    /// whether or not anybody asked, so settling it on read changes nothing a later
    /// caller would not have seen anyway.
    pub fn settle_clock(&mut self) {
        let now = Instant::now();
        for cluster in self.clusters.values_mut() {
            cluster.settle(now);
        }
        for instance in self.instances.values_mut() {
            instance.settle(now);
        }
    }

    /// The key of the cluster `value` names: either its (case-insensitive) identifier or
    /// its ARN, both of which `DBClusterIdentifier` and the `db-cluster-id` filter accept.
    pub fn resolve_cluster_key(&self, value: &str) -> Option<String> {
        let lowered = value.to_ascii_lowercase();
        if self.clusters.contains_key(&lowered) {
            return Some(lowered);
        }
        // Only an ARN can match the scan below, so a plain identifier that named nothing
        // does not pay for rendering every stored record's ARN.
        if !value.starts_with("arn:") {
            return None;
        }
        self.clusters
            .values()
            .find(|record| record.arn() == value)
            .map(|record| record.identifier.clone())
    }

    /// The key of the instance `value` names: either its identifier or its ARN.
    pub fn resolve_instance_key(&self, value: &str) -> Option<String> {
        let lowered = value.to_ascii_lowercase();
        if self.instances.contains_key(&lowered) {
            return Some(lowered);
        }
        // See [`Self::resolve_cluster_key`]: only an ARN can match the scan.
        if !value.starts_with("arn:") {
            return None;
        }
        self.instances
            .values()
            .find(|record| record.arn() == value)
            .map(|record| record.identifier.clone())
    }

    /// The key of the subnet group `value` names: either its (case-insensitive) name or its
    /// ARN, the two forms every `DBSubnetGroupName` member accepts.
    pub fn resolve_subnet_group_key(&self, value: &str) -> Option<String> {
        let lowered = value.to_ascii_lowercase();
        if self.subnet_groups.contains_key(&lowered) {
            return Some(lowered);
        }
        // See [`Self::resolve_cluster_key`]: only an ARN can match the scan.
        if !value.starts_with("arn:") {
            return None;
        }
        self.subnet_groups
            .values()
            .find(|record| record.arn() == value)
            .map(|record| record.name.clone())
    }

    /// The identifiers of the clusters that are placed in the subnet group keyed `key`.
    ///
    /// `DeleteDBSubnetGroup`: "The specified database subnet group must not be associated
    /// with any DB instances", so a group with users is refused rather than deleted.
    pub fn subnet_group_users(&self, key: &str) -> Vec<String> {
        self.clusters
            .values()
            .filter(|cluster| {
                cluster
                    .db_subnet_group
                    .as_deref()
                    .is_some_and(|name| name.eq_ignore_ascii_case(key))
            })
            .map(|cluster| cluster.identifier.clone())
            .collect()
    }

    /// Give the cluster a writer again after one was removed: the surviving member with
    /// the lowest `PromotionTier` is promoted, which is the order `PromotionTier`
    /// documents ("the order in which an Amazon DocumentDB replica is promoted to the
    /// primary instance after a failure of the existing primary instance").
    pub fn reelect_writer(&mut self, cluster_key: &str) {
        let Some(cluster) = self.clusters.get_mut(cluster_key) else {
            return;
        };
        if cluster.members.iter().any(|member| member.is_cluster_writer) {
            return;
        }
        let winner = cluster
            .members
            .iter()
            .enumerate()
            .min_by(|(_, left), (_, right)| {
                left.promotion_tier
                    .cmp(&right.promotion_tier)
                    .then_with(|| left.db_instance_identifier.cmp(&right.db_instance_identifier))
            })
            .map(|(index, _)| index);
        if let Some(index) = winner {
            cluster.members[index].is_cluster_writer = true;
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDocdbAmazonrdsv19State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDocdbAmazonrdsv19State>)
}

/// The region the caller signed this request for, read from its SigV4 credential scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Now, as the modeled timestamp type.
pub fn now() -> ::aws_smithy_types::DateTime {
    let since_epoch = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The fixed-width token behind the `n`th minted resource id, as a base-32 rendering of
/// `n` over [`TOKEN_ALPHABET`]. Deterministic, so two runs of the same call sequence mint
/// the same ids.
pub fn resource_token(ordinal: u64) -> String {
    let mut digits = [TOKEN_ALPHABET[0]; TOKEN_LEN];
    let mut remaining = ordinal;
    for slot in digits.iter_mut().rev() {
        *slot = TOKEN_ALPHABET[(remaining % TOKEN_ALPHABET.len() as u64) as usize];
        remaining /= TOKEN_ALPHABET.len() as u64;
    }
    digits.iter().map(|byte| char::from(*byte)).collect()
}

/// The Availability Zones a cluster lands in when `AvailabilityZones` is absent:
/// DocumentDB spreads a cluster's storage over three zones of the request's region.
pub fn default_availability_zones(region: &str) -> Vec<String> {
    ["a", "b", "c"]
        .iter()
        .map(|suffix| format!("{region}{suffix}"))
        .collect()
}

/// The `PreferredBackupWindow` a cluster gets when the caller names none.
///
/// `CreateDBClusterMessage$PreferredBackupWindow`: "The default is a 30-minute window
/// selected at random from an 8-hour block of time for each Amazon Web Services Region."
/// The emulator picks it from the cluster's ordinal instead of at random, so it is stable
/// for the life of the cluster, and keeps the documented `hh24:mi-hh24:mi` shape.
pub fn default_backup_window(ordinal: u64) -> String {
    let hour = (ordinal % 8) as u32;
    format!("{hour:02}:00-{hour:02}:30")
}

/// The `PreferredMaintenanceWindow` a cluster or instance gets when the caller names none.
///
/// `CreateDBClusterMessage$PreferredMaintenanceWindow`: "The default is a 30-minute window
/// selected at random from an 8-hour block of time for each Amazon Web Services Region,
/// occurring on a random day of the week ... Format: ddd:hh24:mi-ddd:hh24:mi". It must
/// "not conflict with the preferred backup window", so the hour is taken from the block
/// after [`default_backup_window`]'s.
pub fn default_maintenance_window(ordinal: u64) -> String {
    // The weekdays `PreferredMaintenanceWindow` lists: "Valid days: Mon, Tue, Wed, Thu,
    // Fri, Sat, Sun".
    const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
    let day = DAYS[(ordinal % DAYS.len() as u64) as usize];
    let hour = (ordinal % 8) as u32 + 8;
    format!("{day}:{hour:02}:00-{day}:{hour:02}:30")
}

/// Refuse a value that breaks the identifier rule every DocumentDB identifier shares; see
/// [`MAX_IDENTIFIER_LEN`] for the documentation it comes from.
pub fn validate_identifier(member: &str, value: &str) -> Result<(), String> {
    validate_identifier_within(member, value, MAX_IDENTIFIER_LEN)
}

/// The identifier rule with an explicit length bound, for the members whose own
/// documentation states a different one (see [`MAX_SNAPSHOT_IDENTIFIER_LEN`]).
pub fn validate_identifier_within(member: &str, value: &str, max_len: usize) -> Result<(), String> {
    let well_formed = !value.is_empty()
        && value.chars().count() <= max_len
        && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && value.starts_with(|c: char| c.is_ascii_alphabetic())
        && !value.ends_with('-')
        && !value.contains("--");
    if well_formed {
        return Ok(());
    }
    Err(format!(
        "Invalid value '{value}' for {member}: it must contain from 1 to {max_len} \
         letters, numbers, or hyphens, the first character must be a letter, and it cannot end \
         with a hyphen or contain two consecutive hyphens"
    ))
}

/// Refuse an `Engine` that is not the one Amazon DocumentDB runs; see [`DOCDB_ENGINE`].
pub fn validate_engine(value: &str) -> Result<(), String> {
    if value.eq_ignore_ascii_case(DOCDB_ENGINE) {
        return Ok(());
    }
    Err(format!(
        "Invalid value '{value}' for Engine: the only valid value is '{DOCDB_ENGINE}'"
    ))
}

/// Refuse a subnet-group name outside its documented rule; see
/// [`MAX_SUBNET_GROUP_NAME_LEN`] and [`RESERVED_SUBNET_GROUP_NAME`].
pub fn validate_subnet_group_name(member: &str, value: &str) -> Result<(), String> {
    let allowed = |c: char| {
        c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == ' ' || c == '-'
    };
    let well_formed = !value.is_empty()
        && value.chars().count() <= MAX_SUBNET_GROUP_NAME_LEN
        && value.chars().all(allowed)
        && !value.eq_ignore_ascii_case(RESERVED_SUBNET_GROUP_NAME);
    if well_formed {
        return Ok(());
    }
    Err(format!(
        "Invalid value '{value}' for {member}: it must contain no more than \
         {MAX_SUBNET_GROUP_NAME_LEN} letters, numbers, periods, underscores, spaces, or \
         hyphens, and must not be '{RESERVED_SUBNET_GROUP_NAME}'"
    ))
}

/// The published engine version a requested `EngineVersion` names, or `None` when the
/// engine offers none.
///
/// A request may name a version as precisely as it likes: an exact published version
/// (`5.0.0`) or a leading component of one at a dot boundary (`5.0`, `5`), which is how a
/// caller asks for "the latest in that line" and how `EngineVersion`'s own documentation
/// talks about "the intended major engine version". The newest matching version wins, which
/// is what "The --engine-version will default to the latest major engine version" describes
/// for the absent case. Anything else names a version the engine cannot run.
pub fn resolve_engine_version(requested: &str) -> Option<&'static str> {
    DOCDB_ENGINE_VERSIONS
        .iter()
        .rev()
        .find(|published| {
            **published == requested
                || published
                    .strip_prefix(requested)
                    .is_some_and(|rest| rest.starts_with('.'))
        })
        .copied()
}

/// The `DBParameterGroupFamily` of an engine version: the engine name followed by the
/// version's `major.minor` line (`5.0.0` -> `docdb5.0`), which is the form
/// `DBClusterParameterGroup$DBParameterGroupFamily` reports.
pub fn parameter_group_family(version: &str) -> String {
    let mut parts = version.split('.');
    let major = parts.next().unwrap_or(version);
    match parts.next() {
        Some(minor) => format!("{DOCDB_ENGINE}{major}.{minor}"),
        None => format!("{DOCDB_ENGINE}{major}"),
    }
}

/// The cluster parameter group every account and Region already has for `family`, as AWS
/// names it: `default.<family>`; see [`DEFAULT_PARAMETER_GROUP_PREFIX`].
pub fn default_parameter_group_name(family: &str) -> String {
    format!("{DEFAULT_PARAMETER_GROUP_PREFIX}{family}")
}

/// The default cluster parameter groups the engine's published versions imply: one per
/// engine-version family, ordered by name, each sitting in `region`.
///
/// These are not caller-created records and so are not in the store: AWS provides one
/// default cluster parameter group per family in every account and Region, and
/// `DescribeDBClusterParameterGroups` reports them with no create having happened. A
/// caller-created group (`CreateDBClusterParameterGroup`, still scaffolded) will be a
/// stored record read alongside these.
pub fn default_parameter_groups(region: &str) -> Vec<DBClusterParameterGroup> {
    let mut families: Vec<String> = DOCDB_ENGINE_VERSIONS
        .iter()
        .map(|version| parameter_group_family(version))
        .collect();
    families.sort();
    families.dedup();
    families
        .iter()
        .map(|family| {
            let name = default_parameter_group_name(family);
            DBClusterParameterGroup {
                db_cluster_parameter_group_arn: Some(::engine::emulator_arn(
                    ARN_NAMESPACE,
                    region,
                    &format!("cluster-pg:{name}"),
                )),
                db_cluster_parameter_group_name: Some(name),
                db_parameter_group_family: Some(family.clone()),
                description: Some(format!("Default cluster parameter group for {family}")),
            }
        })
        .collect()
}

/// The cluster parameter group `name` names, or `None` when there is no such group.
///
/// Matching is case-insensitive on the name, as every parameter-group name member is.
pub fn resolve_parameter_group(name: &str, region: &str) -> Option<DBClusterParameterGroup> {
    default_parameter_groups(region).into_iter().find(|group| {
        group
            .db_cluster_parameter_group_name
            .as_deref()
            .is_some_and(|stored| stored.eq_ignore_ascii_case(name))
    })
}

/// Whether `family` is a parameter-group family THIS model publishes (`docdb5.0`).
///
/// A family belongs to one engine, so a request naming another engine's family
/// (`aurora-mysql8.0`) is not this model's to answer — see [`serves_parameter_group_name`],
/// which the shared `rds` front door reads for the same decision.
pub fn serves_parameter_group_family(family: &str) -> bool {
    DOCDB_ENGINE_VERSIONS
        .iter()
        .any(|version| parameter_group_family(version).eq_ignore_ascii_case(family))
}

/// Whether THIS model can answer for the cluster parameter group `name` at all.
///
/// A default group's name names its family (`default.docdb5.0`), and a family belongs to one
/// engine, so a `default.<family>` name is answerable only by the model that publishes that
/// family — this one answers for the DocumentDB families and for no other engine's. Any
/// other name is a caller-created group, and this service's store is the only cluster
/// parameter-group store in this emulator, so a name it does not hold is a group that does
/// not exist whichever engine asked.
///
/// The shared `rds` front door reads this to decide which co-located model serves a
/// parameter-group request (see
/// `services/service-com-amazonaws-rds-amazonrdsv19/src/service.rs`), so a caller on another
/// engine naming ITS family is not answered from this engine's catalogue.
pub fn serves_parameter_group_name(name: &str) -> bool {
    match name.strip_prefix(DEFAULT_PARAMETER_GROUP_PREFIX) {
        Some(family) => serves_parameter_group_family(family),
        None => true,
    }
}

/// Refuse a `MasterUsername` outside its documented rule; see [`MAX_MASTER_USERNAME_LEN`].
pub fn validate_master_username(value: &str) -> Result<(), String> {
    let well_formed = !value.is_empty()
        && value.chars().count() <= MAX_MASTER_USERNAME_LEN
        && value.chars().all(|c| c.is_ascii_alphanumeric())
        && value.starts_with(|c: char| c.is_ascii_alphabetic());
    if well_formed {
        return Ok(());
    }
    Err(format!(
        "Invalid value '{value}' for MasterUsername: it must be from 1 to \
         {MAX_MASTER_USERNAME_LEN} letters or numbers and the first character must be a letter"
    ))
}

/// Refuse a `MasterUserPassword` outside its documented rule; see
/// [`MIN_MASTER_USER_PASSWORD_LEN`] and [`MASTER_USER_PASSWORD_FORBIDDEN`]. The value
/// itself is never quoted back, and this service never stores it.
pub fn validate_master_user_password(value: &str) -> Result<(), String> {
    let length = value.chars().count();
    let printable_ascii = value.chars().all(|c| c.is_ascii() && !c.is_ascii_control());
    let well_formed = (MIN_MASTER_USER_PASSWORD_LEN..=MAX_MASTER_USER_PASSWORD_LEN)
        .contains(&length)
        && printable_ascii
        && !value.contains(|c: char| MASTER_USER_PASSWORD_FORBIDDEN.contains(&c));
    if well_formed {
        return Ok(());
    }
    Err(format!(
        "Invalid value for MasterUserPassword: it must contain from \
         {MIN_MASTER_USER_PASSWORD_LEN} to {MAX_MASTER_USER_PASSWORD_LEN} printable ASCII \
         characters and cannot contain '/', '\"' or '@'"
    ))
}

/// Refuse a `BackupRetentionPeriod` outside its documented range; see
/// [`MIN_BACKUP_RETENTION_PERIOD`].
pub fn validate_backup_retention_period(value: i32) -> Result<(), String> {
    if (MIN_BACKUP_RETENTION_PERIOD..=MAX_BACKUP_RETENTION_PERIOD).contains(&value) {
        return Ok(());
    }
    Err(format!(
        "Invalid value {value} for BackupRetentionPeriod: it must be a value from \
         {MIN_BACKUP_RETENTION_PERIOD} to {MAX_BACKUP_RETENTION_PERIOD}"
    ))
}

/// Refuse a `PromotionTier` outside its documented range; see [`MIN_PROMOTION_TIER`].
pub fn validate_promotion_tier(value: i32) -> Result<(), String> {
    if (MIN_PROMOTION_TIER..=MAX_PROMOTION_TIER).contains(&value) {
        return Ok(());
    }
    Err(format!(
        "Invalid value {value} for PromotionTier: valid values are \
         {MIN_PROMOTION_TIER}-{MAX_PROMOTION_TIER}"
    ))
}

/// The page size a describe must use: the caller's `MaxRecords` once it is inside the
/// documented range, else [`DEFAULT_MAX_RECORDS`]; see [`MIN_MAX_RECORDS`].
pub fn page_size(max_records: Option<i32>) -> Result<usize, String> {
    let Some(value) = max_records else {
        return Ok(DEFAULT_MAX_RECORDS as usize);
    };
    if (MIN_MAX_RECORDS..=MAX_MAX_RECORDS).contains(&value) {
        return Ok(value as usize);
    }
    Err(format!(
        "Invalid value {value} for MaxRecords: minimum {MIN_MAX_RECORDS}, maximum \
         {MAX_MAX_RECORDS}"
    ))
}

/// One page of an ordered key list, plus the `Marker` to report.
///
/// The `Marker` doc is what fixes the semantics: "An optional pagination token provided
/// by a previous request. If this parameter is specified, the response includes only
/// records BEYOND the marker" — so the token handed back is the LAST key of this page and
/// the next page starts strictly after it. The last page carries none, which is the round
/// trip `@paginated(inputToken: Marker, outputToken: Marker)` describes.
///
/// A marker that names no key in `keys` is taken as a position rather than refused: the
/// key it named can legitimately have been deleted between two pages, and the real
/// service answers the records beyond it rather than failing a caller that paged
/// correctly.
pub fn paginate(
    keys: &[String],
    marker: Option<&str>,
    page_size: usize,
) -> (Vec<String>, Option<String>) {
    let start = match marker {
        Some(from) => keys
            .iter()
            .position(|key| key.as_str() > from)
            .unwrap_or(keys.len()),
        None => 0,
    };
    let end = start.saturating_add(page_size).min(keys.len());
    let page = keys[start..end].to_vec();
    let next = (end < keys.len())
        .then(|| page.last().cloned())
        .flatten();
    (page, next)
}

/// The major-version component of an engine version: everything before the first `.`, or
/// the whole string when it carries none. `ModifyDBCluster`'s `AllowMajorVersionUpgrade`
/// is what makes this a rule the service enforces.
pub fn engine_major_version(version: &str) -> &str {
    version.split('.').next().unwrap_or(version)
}

/// The `Tags` of a create request as the (key, value) pairs state keeps.
pub fn tag_pairs(tags: Option<Vec<Tag>>) -> Vec<(String, String)> {
    tags.unwrap_or_default()
        .into_iter()
        .map(|tag| {
            (
                tag.key.unwrap_or_default(),
                tag.value.unwrap_or_default(),
            )
        })
        .collect()
}

/// A cluster record as the modeled `DBCluster`.
///
/// `MultiAZ` is computed from the member instances' Availability Zones, and the endpoints
/// and ARN from the identifier, so nothing here is a value the handler was handed.
pub fn cluster_to_shape(
    record: &DbClusterRecord,
    instances: &BTreeMap<String, DbInstanceRecord>,
) -> DBCluster {
    let zones_in_use: BTreeSet<&str> = record
        .members
        .iter()
        .filter_map(|member| instances.get(&member.db_instance_identifier))
        .map(|instance| instance.availability_zone.as_str())
        .collect();
    DBCluster {
        availability_zones: Some(record.availability_zones.clone()),
        backup_retention_period: Some(record.backup_retention_period),
        db_cluster_identifier: Some(record.identifier.clone()),
        db_cluster_parameter_group: record.db_cluster_parameter_group.clone(),
        db_subnet_group: record.db_subnet_group.clone(),
        status: Some(record.status.clone()),
        percent_progress: None,
        earliest_restorable_time: None,
        endpoint: Some(record.endpoint()),
        reader_endpoint: Some(record.reader_endpoint()),
        multi_az: Some(zones_in_use.len() > 1),
        engine: Some(record.engine.clone()),
        engine_version: Some(record.engine_version.clone()),
        latest_restorable_time: None,
        port: Some(record.port),
        master_username: record.master_username.clone(),
        preferred_backup_window: Some(record.preferred_backup_window.clone()),
        preferred_maintenance_window: Some(record.preferred_maintenance_window.clone()),
        replication_source_identifier: None,
        read_replica_identifiers: Some(Vec::new()),
        db_cluster_members: Some(
            record
                .members
                .iter()
                .map(|member| DBClusterMember {
                    db_instance_identifier: Some(member.db_instance_identifier.clone()),
                    is_cluster_writer: Some(member.is_cluster_writer),
                    db_cluster_parameter_group_status: Some(PARAMETER_GROUP_IN_SYNC.to_string()),
                    promotion_tier: Some(member.promotion_tier),
                })
                .collect(),
        ),
        vpc_security_groups: Some(
            record
                .vpc_security_group_ids
                .iter()
                .map(|id| VpcSecurityGroupMembership {
                    vpc_security_group_id: Some(id.clone()),
                    status: Some(SECURITY_GROUP_ACTIVE.to_string()),
                })
                .collect(),
        ),
        hosted_zone_id: None,
        storage_encrypted: Some(record.storage_encrypted),
        kms_key_id: record.kms_key_id.clone(),
        db_cluster_resource_id: Some(record.resource_id.clone()),
        db_cluster_arn: Some(record.arn()),
        associated_roles: Some(Vec::new()),
        clone_group_id: None,
        cluster_create_time: Some(record.cluster_create_time),
        enabled_cloudwatch_logs_exports: (!record.enabled_cloudwatch_logs_exports.is_empty())
            .then(|| record.enabled_cloudwatch_logs_exports.clone()),
        deletion_protection: Some(record.deletion_protection),
        io_optimized_next_allowed_modification_time: None,
        storage_type: record.storage_type.clone(),
        serverless_v2_scaling_configuration: record.serverless_v2_scaling.map(|(min, max)| {
            ServerlessV2ScalingConfigurationInfo {
                min_capacity: Some(min),
                max_capacity: Some(max),
            }
        }),
        master_user_secret: None,
        network_type: record.network_type.clone(),
        copy_tags_to_snapshot: Some(record.copy_tags_to_snapshot),
    }
}

/// An instance record as the modeled `DBInstance`.
///
/// `cluster` is the instance's cluster, when it is still in state: the members that a
/// cluster owns (port, backup settings, encryption, security groups, engine version) are
/// read from it rather than copied at create time, so a later `ModifyDBCluster` shows up
/// on its instances too.
pub fn instance_to_shape(
    record: &DbInstanceRecord,
    cluster: Option<&DbClusterRecord>,
) -> DBInstance {
    DBInstance {
        db_instance_identifier: Some(record.identifier.clone()),
        db_instance_class: Some(record.instance_class.clone()),
        engine: Some(record.engine.clone()),
        db_instance_status: Some(record.status.clone()),
        endpoint: Some(Endpoint {
            address: Some(record.endpoint_address.clone()),
            port: Some(cluster.map_or(DEFAULT_PORT, |owner| owner.port)),
            hosted_zone_id: None,
        }),
        instance_create_time: Some(record.instance_create_time),
        preferred_backup_window: cluster.map(|owner| owner.preferred_backup_window.clone()),
        backup_retention_period: cluster.map(|owner| owner.backup_retention_period),
        vpc_security_groups: cluster.map(|owner| {
            owner
                .vpc_security_group_ids
                .iter()
                .map(|id| VpcSecurityGroupMembership {
                    vpc_security_group_id: Some(id.clone()),
                    status: Some(SECURITY_GROUP_ACTIVE.to_string()),
                })
                .collect()
        }),
        availability_zone: Some(record.availability_zone.clone()),
        db_subnet_group: None,
        preferred_maintenance_window: Some(record.preferred_maintenance_window.clone()),
        pending_modified_values: None,
        latest_restorable_time: None,
        engine_version: cluster.map(|owner| owner.engine_version.clone()),
        auto_minor_version_upgrade: Some(record.auto_minor_version_upgrade),
        publicly_accessible: Some(false),
        status_infos: None,
        db_cluster_identifier: Some(record.cluster_identifier.clone()),
        storage_encrypted: cluster.map(|owner| owner.storage_encrypted),
        kms_key_id: cluster.and_then(|owner| owner.kms_key_id.clone()),
        dbi_resource_id: Some(record.dbi_resource_id.clone()),
        ca_certificate_identifier: record.ca_certificate_identifier.clone(),
        copy_tags_to_snapshot: Some(record.copy_tags_to_snapshot),
        promotion_tier: Some(record.promotion_tier),
        db_instance_arn: Some(record.arn()),
        enabled_cloudwatch_logs_exports: cluster.and_then(|owner| {
            (!owner.enabled_cloudwatch_logs_exports.is_empty())
                .then(|| owner.enabled_cloudwatch_logs_exports.clone())
        }),
        certificate_details: None,
        performance_insights_enabled: Some(record.performance_insights_enabled),
        performance_insights_kms_key_id: record.performance_insights_kms_key_id.clone(),
    }
}
