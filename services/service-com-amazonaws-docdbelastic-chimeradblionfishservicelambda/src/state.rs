// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-docdbelastic-chimeradblionfishservicelambda service.
//!
//! The elastic-cluster lifecycle is stateful: `CreateCluster` mints a record here,
//! every reader renders its response from the record, and `DeleteCluster` removes it.
//! The record types below are the persisted form; [`ClusterRecord::as_cluster`] and
//! [`SnapshotRecord::as_snapshot`] project them onto the modelled wire shapes, which is
//! the only place a response body is built from.

use crate::ops::types::{
    Cluster, ClusterInList, ClusterSnapshot, ConflictException, ResourceNotFoundException,
    ServiceQuotaExceededException, Shard, ValidationException, ValidationExceptionField,
};
use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime};

/// How long after the call that starts it a cluster (or snapshot) status transition the
/// clock owes becomes due.
///
/// The emulator never sleeps for it: the due instant is stored on the record and
/// materialised by whoever next looks at that record. The span is deliberately
/// non-zero, so a caller acting straight after `CreateCluster` still observes
/// `CREATING` and the state errors that go with it.
const SETTLE_AFTER: Duration = Duration::from_millis(500);

/// Per-service state for com-amazonaws-docdbelastic-chimeradblionfishservicelambda. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsDocdbelasticChimeradblionfishservicelambdaState {
    /// Every live elastic cluster, keyed by its `clusterArn`. `BTreeMap` so
    /// `ListClusters` pages in a stable, total order without sorting on every call.
    pub clusters: BTreeMap<String, ClusterRecord>,
    /// `clusterName` -> `clusterArn`, so a duplicate name is refused with
    /// `ConflictException` instead of minting a second cluster with the same name.
    pub cluster_names: BTreeMap<String, String>,
    /// Every cluster snapshot, keyed by its `snapshotArn`.
    pub snapshots: BTreeMap<String, SnapshotRecord>,
    /// `snapshotName` -> `snapshotArn`, so a duplicate snapshot name is refused
    /// without a linear scan of `snapshots`.
    pub snapshot_names: BTreeMap<String, String>,
}

/// One persisted elastic cluster.
///
/// `admin_user_password` is held because `UpdateCluster` can change it, and is NEVER
/// projected onto a response: the modelled `Cluster` shape has no such member, and
/// [`as_cluster`](ClusterRecord::as_cluster) is the only projection. `Debug` is written
/// by hand rather than derived so the secret cannot reach a log or a panic message
/// either — the model marks it `@sensitive`.
#[derive(Clone)]
pub struct ClusterRecord {
    pub cluster_name: String,
    pub cluster_arn: String,
    pub status: String,
    pub cluster_endpoint: String,
    pub create_time: String,
    pub admin_user_name: String,
    pub admin_user_password: String,
    pub auth_type: String,
    pub shard_capacity: i32,
    pub shard_count: i32,
    pub shard_instance_count: Option<i32>,
    pub vpc_security_group_ids: Vec<String>,
    pub subnet_ids: Vec<String>,
    pub preferred_maintenance_window: String,
    pub preferred_backup_window: Option<String>,
    pub backup_retention_period: Option<i32>,
    pub kms_key_id: String,
    pub shards: Vec<ShardRecord>,
    pub tags: BTreeMap<String, String>,
    /// When the clock owes this cluster a move to `ACTIVE`; `None` once it is settled.
    pub settles_at: Option<Instant>,
}

/// One shard of a persisted cluster.
#[derive(Debug, Clone)]
pub struct ShardRecord {
    pub shard_id: String,
    pub create_time: String,
    pub status: String,
}

/// One persisted cluster snapshot.
#[derive(Debug, Clone)]
pub struct SnapshotRecord {
    pub snapshot_name: String,
    pub snapshot_arn: String,
    pub cluster_arn: String,
    pub snapshot_creation_time: String,
    pub cluster_creation_time: String,
    pub status: String,
    pub snapshot_type: String,
    pub admin_user_name: String,
    pub kms_key_id: String,
    pub subnet_ids: Vec<String>,
    pub vpc_security_group_ids: Vec<String>,
    pub tags: BTreeMap<String, String>,
    /// When the clock owes this snapshot a move to `ACTIVE`; `None` once settled.
    pub settles_at: Option<Instant>,
}

impl ::std::fmt::Debug for ClusterRecord {
    /// Everything except the administrator password, which is `@sensitive` and so is
    /// rendered as a fixed placeholder.
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("ClusterRecord")
            .field("cluster_name", &self.cluster_name)
            .field("cluster_arn", &self.cluster_arn)
            .field("status", &self.status)
            .field("cluster_endpoint", &self.cluster_endpoint)
            .field("create_time", &self.create_time)
            .field("admin_user_name", &self.admin_user_name)
            .field("admin_user_password", &"<redacted>")
            .field("auth_type", &self.auth_type)
            .field("shard_capacity", &self.shard_capacity)
            .field("shard_count", &self.shard_count)
            .field("shard_instance_count", &self.shard_instance_count)
            .field("vpc_security_group_ids", &self.vpc_security_group_ids)
            .field("subnet_ids", &self.subnet_ids)
            .field(
                "preferred_maintenance_window",
                &self.preferred_maintenance_window,
            )
            .field("preferred_backup_window", &self.preferred_backup_window)
            .field("backup_retention_period", &self.backup_retention_period)
            .field("kms_key_id", &self.kms_key_id)
            .field("shards", &self.shards)
            .field("tags", &self.tags)
            .field("settles_at", &self.settles_at)
            .finish()
    }
}

impl ClusterRecord {
    /// Start a status transition the clock owes: `status` now, `ACTIVE` once
    /// [`SETTLE_AFTER`] has elapsed.
    pub fn begin_transition(&mut self, status: &str) {
        self.status = status.to_string();
        self.settles_at = Some(settle_instant());
        for shard in &mut self.shards {
            shard.status = status.to_string();
        }
    }

    /// Materialise the transition the clock has already made due. Reporting
    /// operations call this: it only completes work that was owed at this instant
    /// whether or not anybody called.
    pub fn settle_due(&mut self) {
        if self.settles_at.is_some_and(|due| Instant::now() >= due) {
            self.settles_at = None;
            self.status = STATUS_ACTIVE.to_string();
            for shard in &mut self.shards {
                shard.status = STATUS_ACTIVE.to_string();
            }
        }
    }

    /// The modelled `Cluster` for this record. Every `@required` member is populated.
    pub fn as_cluster(&self) -> Cluster {
        Cluster {
            cluster_name: self.cluster_name.clone(),
            cluster_arn: self.cluster_arn.clone(),
            status: self.status.clone(),
            cluster_endpoint: self.cluster_endpoint.clone(),
            create_time: self.create_time.clone(),
            admin_user_name: self.admin_user_name.clone(),
            auth_type: self.auth_type.clone(),
            shard_capacity: self.shard_capacity,
            shard_count: self.shard_count,
            vpc_security_group_ids: self.vpc_security_group_ids.clone(),
            subnet_ids: self.subnet_ids.clone(),
            preferred_maintenance_window: self.preferred_maintenance_window.clone(),
            kms_key_id: self.kms_key_id.clone(),
            shards: Some(
                self.shards
                    .iter()
                    .map(|shard| Shard {
                        shard_id: shard.shard_id.clone(),
                        create_time: shard.create_time.clone(),
                        status: shard.status.clone(),
                    })
                    .collect(),
            ),
            backup_retention_period: self.backup_retention_period,
            preferred_backup_window: self.preferred_backup_window.clone(),
            shard_instance_count: self.shard_instance_count,
        }
    }

    /// The summary form `ListClusters` returns.
    pub fn as_cluster_in_list(&self) -> ClusterInList {
        ClusterInList {
            cluster_name: self.cluster_name.clone(),
            cluster_arn: self.cluster_arn.clone(),
            status: self.status.clone(),
        }
    }
}

impl SnapshotRecord {
    /// Materialise the transition the clock has already made due; see
    /// [`ClusterRecord::settle_due`].
    pub fn settle_due(&mut self) {
        if self.settles_at.is_some_and(|due| Instant::now() >= due) {
            self.settles_at = None;
            self.status = STATUS_ACTIVE.to_string();
        }
    }

    /// The modelled `ClusterSnapshot` for this record.
    pub fn as_snapshot(&self) -> ClusterSnapshot {
        ClusterSnapshot {
            subnet_ids: self.subnet_ids.clone(),
            snapshot_name: self.snapshot_name.clone(),
            snapshot_arn: self.snapshot_arn.clone(),
            snapshot_creation_time: self.snapshot_creation_time.clone(),
            cluster_arn: self.cluster_arn.clone(),
            cluster_creation_time: self.cluster_creation_time.clone(),
            status: self.status.clone(),
            vpc_security_group_ids: self.vpc_security_group_ids.clone(),
            admin_user_name: self.admin_user_name.clone(),
            kms_key_id: self.kms_key_id.clone(),
            snapshot_type: Some(self.snapshot_type.clone()),
        }
    }
}

/// `Status` values this crate names. The full value set is `AUTH_VALUES`' sibling
/// `STATUS_VALUES` in `ops::types`; these are the three the cluster lifecycle uses.
pub const STATUS_CREATING: &str = "CREATING";
pub const STATUS_ACTIVE: &str = "ACTIVE";
pub const STATUS_UPDATING: &str = "UPDATING";
pub const STATUS_DELETING: &str = "DELETING";

/// The `SnapshotType` a caller-requested snapshot carries (`CreateClusterSnapshot`);
/// AWS reserves `AUTOMATED` for the ones it takes on the backup window.
///
/// The spelling is the model's: `smithy_query(query="shape", shape="SnapshotType")`
/// reports the wire values as `MANUAL` / `AUTOMATED`, and the scaffold's own
/// `ops::types::SNAPSHOT_TYPE_VALUES` ("the values `SnapshotType` accepts on the
/// wire") agrees. The lowercase `manual` / `automated` in the member's prose is how
/// the documentation writes them in a sentence, not a second wire spelling.
pub const SNAPSHOT_TYPE_MANUAL: &str = "MANUAL";

/// The signing / ARN namespace for this service, as `OPERATIONS.md` records it.
pub const ARN_SERVICE: &str = "docdb-elastic";

/// The region this request is scoped to, read from its SigV4 credential scope.
///
/// A request that carries no parseable scope (an unsigned call to an emulator with
/// authentication off) is treated as `us-east-1`, the emulator-wide default.
pub fn request_region(parts: &::smithy_server_runtime::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| "us-east-1".to_string())
}

/// The instant a transition started now becomes due.
pub fn settle_instant() -> Instant {
    Instant::now() + SETTLE_AFTER
}

/// Now, as the UTC instant string every `*Time` member of this service carries.
///
/// The modelled members are `String`, not `timestamp`, and documented as "in
/// Universal Coordinated Time (UTC)", so this renders RFC 3339 / ISO 8601 with a `Z`
/// offset through `aws_smithy_types`' own formatter rather than inventing a format.
pub fn now_utc() -> String {
    ::aws_smithy_types::DateTime::from(SystemTime::now())
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// Monotonic counter mixed into every minted id, so two ids minted inside the same
/// clock tick still differ. Only [`new_resource_id`] reads it.
static MINT_COUNTER: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);

/// A fresh server-generated resource id, in the 8-4-4-4-12 lowercase-hex shape an
/// elastic cluster or snapshot id has.
///
/// Uniqueness comes from the monotonic counter, not from the clock, so two ids minted
/// within one tick (or after the clock steps back) are still distinct; the counter is
/// hashed only so a caller cannot read a sequence number out of an id and predict the
/// next one. This mirrors `mint_id` in the pinpoint service's `state.rs`.
pub fn new_resource_id() -> String {
    use ::sha2::{Digest, Sha256};
    use ::std::sync::atomic::Ordering;

    let counter = MINT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();
    let digest = Sha256::digest(format!("{counter}:{nanos}").as_bytes());
    // 16 bytes is exactly the 32 hex characters the five groups below consume.
    let rendered = ::hex::encode(&digest[..16]);
    format!(
        "{}-{}-{}-{}-{}",
        &rendered[0..8],
        &rendered[8..12],
        &rendered[12..16],
        &rendered[16..20],
        &rendered[20..32]
    )
}

/// `arn:aws:docdb-elastic:<region>:<account>:<resource>` for this emulator's account.
pub fn docdb_elastic_arn(region: &str, resource: &str) -> String {
    ::emulator_server::emulator_arn(ARN_SERVICE, region, resource)
}

/// The host name a client connects to this cluster on.
///
/// Derived from the cluster's own identity, never from anything the caller passed as
/// an endpoint: `<name>.<cluster id>.<region>.docdb-elastic.amazonaws.com`.
pub fn cluster_endpoint(cluster_name: &str, cluster_id: &str, region: &str) -> String {
    format!("{cluster_name}.{cluster_id}.{region}.{ARN_SERVICE}.amazonaws.com")
}

/// The tag this service stamps on every pagination cursor it issues, so a token it
/// did not issue is rejected rather than read as an arbitrary start key.
const PAGE_TOKEN_TAG: &str = "docdb-elastic/clusters#";

/// Encode a pagination cursor: the key the NEXT page starts at.
///
/// Tagged, then hex-encoded, so the token a caller gets back is opaque, round-trips
/// through a URI query parameter unchanged whatever the key contains, and is
/// distinguishable from any other string a caller might send.
pub fn encode_page_token(next_key: &str) -> String {
    ::hex::encode(format!("{PAGE_TOKEN_TAG}{next_key}").as_bytes())
}

/// Decode a cursor produced by [`encode_page_token`]; `None` when it is not one.
pub fn decode_page_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    decoded.strip_prefix(PAGE_TOKEN_TAG).map(str::to_string)
}

/// The wording every handler uses when the shared engine does not carry this
/// service's store. It is a server-side fault, so it is answered as the modelled
/// `InternalServerException` (HTTP 500) — never with a freshly built engine.
pub const STORE_UNREACHABLE: &str =
    "the DocDB Elastic store is not reachable from this engine instance";

/// The default `kmsKeyId` a cluster created without one reports.
///
/// `CreateClusterInput$kmsKeyId` documents that "If an encryption key is not
/// specified, Amazon DocumentDB uses the default encryption key that KMS creates for
/// your account"; DocumentDB reports that case as this sentinel rather than as an
/// account-specific key ARN.
pub const AWS_OWNED_KMS_KEY: &str = "AWS_OWNED_KMS_KEY";

/// The days `preferredMaintenanceWindow` may name.
///
/// Source: `CreateClusterInput$preferredMaintenanceWindow` doc — "Valid days: Mon,
/// Tue, Wed, Thu, Fri, Sat, Sun".
pub const MAINTENANCE_DAYS: &[&str] = &["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// The values `shardCapacity` accepts.
///
/// Source: `CreateClusterInput$shardCapacity` doc — "Maximum is 64. Allowed values
/// are 2, 4, 8, 16, 32, 64".
pub const SHARD_CAPACITY_VALUES: &[i32] = &[2, 4, 8, 16, 32, 64];

/// The largest `shardCount` a cluster may have.
///
/// Source: `CreateClusterInput$shardCount` doc — "Maximum is 32".
pub const SHARD_COUNT_MAX: i32 = 32;

/// The inclusive length bounds of `adminUserPassword`, and the characters it may not
/// contain.
///
/// Source: `CreateClusterInput$adminUserPassword` doc — "Must contain from 8 to 100
/// characters. Cannot contain a forward slash (/), double quote ("), or the "at"
/// symbol (@)".
pub const PASSWORD_LEN: (usize, usize) = (8, 100);
pub const PASSWORD_FORBIDDEN: &[char] = &['/', '"', '@'];

/// The inclusive length bound shared by `clusterName` and `adminUserName`.
///
/// Source: their `CreateClusterInput` docs — "from 1 to 63".
pub const NAME_LEN: (usize, usize) = (1, 63);

/// The page size `ListClusters` uses when the caller names no `maxResults`.
///
/// `ListClustersInput$maxResults` is modelled `range(min: 1, max: 100)`, so the
/// largest page the member can ask for is the natural default.
pub const DEFAULT_PAGE_SIZE: i32 = 100;

// ---------------------------------------------------------------------------
// Resource guards
//
// Every record below is created by a caller and reclaimed only by an explicit
// delete, so each growth site carries a cap: the store lives for the life of the
// process and a caller must not be able to grow it without limit. The record
// COUNTS are emulator resource guards (AWS publishes soft, account-level limits for
// elastic clusters that an emulator cannot meaningfully mirror); the per-request
// value bounds are the service's own documented or modelled limits where it has
// them.
// ---------------------------------------------------------------------------

/// The most elastic clusters this emulator holds at once. Emulator resource guard:
/// `CreateCluster` answers `ServiceQuotaExceededException` past it, which is the
/// error the operation models for a quota, and `DeleteCluster` frees a slot.
pub const MAX_CLUSTERS: usize = 100;

/// The most cluster snapshots this emulator holds at once. Emulator resource guard;
/// `DeleteClusterSnapshot` frees a slot.
pub const MAX_CLUSTER_SNAPSHOTS: usize = 500;

/// The most tags one resource carries.
///
/// Source: `UntagResourceInput$tagKeys` is modelled `length(min: 0, max: 50)`, so no
/// resource can hold more than 50 distinct tag keys.
pub const MAX_TAGS: usize = 50;

/// The inclusive length bounds of a tag key and a tag value (the AWS-wide tag limits).
pub const MAX_TAG_KEY_LEN: usize = 128;
pub const MAX_TAG_VALUE_LEN: usize = 256;

/// The most entries `subnetIds` or `vpcSecurityGroupIds` may carry, and the longest
/// one entry may be. Emulator resource guards: the model bounds neither, and both are
/// stored verbatim on the record.
pub const MAX_NETWORK_IDS: usize = 64;
pub const MAX_NETWORK_ID_LEN: usize = 255;

/// The inclusive length bounds of `snapshotName`.
///
/// Source: `CreateClusterSnapshotInput$snapshotName` carries a member-level
/// `length(min: 1, max: 63)` (`smithy_query(query="operation",
/// operation="CreateClusterSnapshot")`). The name is stored verbatim on the record
/// AND as the key of the name index, so it is bounded here too rather than relying on
/// the boundary validator alone.
pub const SNAPSHOT_NAME_LEN: (usize, usize) = (1, 63);

/// The longest `kmsKeyId` this service stores: an ARN, an alias or a key id, all of
/// which fit inside the AWS maximum ARN length.
pub const MAX_KMS_KEY_ID_LEN: usize = 2048;

/// A `ValidationException` (HTTP 400) for one member that failed a documented
/// constraint the model does not express.
///
/// The message is built from the request — it names the member and the constraint it
/// broke — and `reason` is the modelled `fieldValidationFailed`.
pub fn field_validation_failed(member: &str, constraint: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "1 validation error detected: Value at '{member}' failed to satisfy constraint: {constraint}"
        ),
        reason: "fieldValidationFailed".to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: constraint.to_string(),
        }]),
    }
}

/// Refuse a `shardCapacity` outside the documented allowed values.
pub fn validate_shard_capacity(member: &str, value: i32) -> Result<(), ValidationException> {
    if SHARD_CAPACITY_VALUES.contains(&value) {
        return Ok(());
    }
    Err(field_validation_failed(
        member,
        "Member must be one of the allowed values [2, 4, 8, 16, 32, 64]",
    ))
}

/// Refuse a `shardCount` outside 1..=[`SHARD_COUNT_MAX`].
pub fn validate_shard_count(member: &str, value: i32) -> Result<(), ValidationException> {
    if (1..=SHARD_COUNT_MAX).contains(&value) {
        return Ok(());
    }
    Err(field_validation_failed(
        member,
        &format!("Member must be between 1 and {SHARD_COUNT_MAX}, inclusive"),
    ))
}

/// Refuse a `shardInstanceCount` below 1. The docs name no upper bound, so none is
/// imposed here.
pub fn validate_shard_instance_count(member: &str, value: i32) -> Result<(), ValidationException> {
    if value >= 1 {
        return Ok(());
    }
    Err(field_validation_failed(
        member,
        "Member must be at least 1",
    ))
}

/// Refuse an `adminUserPassword` outside the documented length or character rules.
pub fn validate_admin_user_password(
    member: &str,
    value: &str,
) -> Result<(), ValidationException> {
    let (min, max) = PASSWORD_LEN;
    if !(min..=max).contains(&value.chars().count()) {
        return Err(field_validation_failed(
            member,
            &format!("Member must have length between {min} and {max}, inclusive"),
        ));
    }
    if value.chars().any(|c| PASSWORD_FORBIDDEN.contains(&c)) {
        return Err(field_validation_failed(
            member,
            "Member must not contain a forward slash (/), a double quote (\") or an at symbol (@)",
        ));
    }
    Ok(())
}

/// Refuse a `clusterName` outside the documented form.
pub fn validate_cluster_name(member: &str, value: &str) -> Result<(), ValidationException> {
    let (min, max) = NAME_LEN;
    let constraint = format!(
        "Member must have length between {min} and {max}, inclusive, contain only letters, numbers and hyphens, start with a letter, not end with a hyphen and not contain two consecutive hyphens"
    );
    let len = value.chars().count();
    let well_formed = (min..=max).contains(&len)
        && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && value.starts_with(|c: char| c.is_ascii_alphabetic())
        && !value.ends_with('-')
        && !value.contains("--");
    if well_formed {
        Ok(())
    } else {
        Err(field_validation_failed(member, &constraint))
    }
}

/// Refuse an `adminUserName` outside the documented form.
pub fn validate_admin_user_name(member: &str, value: &str) -> Result<(), ValidationException> {
    let (min, max) = NAME_LEN;
    let len = value.chars().count();
    let well_formed = (min..=max).contains(&len)
        && value.chars().all(|c| c.is_ascii_alphanumeric())
        && value.starts_with(|c: char| c.is_ascii_alphabetic());
    if well_formed {
        Ok(())
    } else {
        Err(field_validation_failed(
            member,
            &format!(
                "Member must have length between {min} and {max}, inclusive, contain only letters and numbers and start with a letter"
            ),
        ))
    }
}

/// A kind of resource this service owns, named the way AWS names it.
///
/// Each kind has TWO spellings, and they are not interchangeable:
///
/// * the RESOURCE TYPE NAME, which is the modelled resource shape's own name —
///   `Cluster` / `ClusterSnapshot` (`smithy_query(query="shape",
///   shape="Cluster,ClusterSnapshot")`). It is what the error shapes' `resourceType`
///   member carries: "The type of the resource that could not be found"
///   (`ResourceNotFoundException$resourceType`) and "The type of the resource where
///   there was an access conflict" (`ConflictException$resourceType`), both
///   `@required`;
/// * the ARN RESOURCE SEGMENT, which is lowercase and hyphenated — `cluster/<id>`,
///   `cluster-snapshot/<id>`.
///
/// Holding both on one value is what keeps an error body from reporting the ARN
/// segment (or this store's own key) where a client reads a resource type, and keeps
/// the ARN segment spelled in exactly one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    /// An elastic cluster: the modelled `Cluster` shape.
    Cluster,
    /// An elastic cluster snapshot: the modelled `ClusterSnapshot` shape.
    ClusterSnapshot,
}

impl ResourceKind {
    /// The AWS resource type NAME, as every `resourceType` member reports it.
    pub const fn type_name(self) -> &'static str {
        match self {
            Self::Cluster => "Cluster",
            Self::ClusterSnapshot => "ClusterSnapshot",
        }
    }

    /// The resource segment an ARN of this kind carries, ahead of the resource id.
    pub const fn arn_segment(self) -> &'static str {
        match self {
            Self::Cluster => "cluster",
            Self::ClusterSnapshot => "cluster-snapshot",
        }
    }

    /// The ARN of the resource of this kind with id `resource_id`, in `region`.
    pub fn arn(self, region: &str, resource_id: &str) -> String {
        docdb_elastic_arn(region, &format!("{}/{resource_id}", self.arn_segment()))
    }
}

/// The `ResourceNotFoundException` (HTTP 404) for an ARN this store does not hold.
///
/// The two structured members are what a client branches on: `resourceType` is the
/// AWS resource type name of the kind that was looked up (see [`ResourceKind`]) and
/// `resourceId` is the ARN the request named. The message restates them in prose and
/// is not where either is read from.
pub fn not_found(kind: ResourceKind, arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("{} {arn} not found", kind.type_name()),
        resource_id: arn.to_string(),
        resource_type: kind.type_name().to_string(),
    }
}

/// The `ConflictException` (HTTP 409) for a request that collides with the already
/// existing resource `arn` of kind `kind`.
///
/// Same contract as [`not_found`] for the two structured members: the type name of the
/// resource kind and the ARN of the resource the request collided with.
pub fn conflict(kind: ResourceKind, arn: &str, message: String) -> ConflictException {
    ConflictException {
        message,
        resource_id: arn.to_string(),
        resource_type: kind.type_name().to_string(),
    }
}

/// Refuse a create that would take the cluster count past [`MAX_CLUSTERS`].
pub fn ensure_cluster_capacity(current: usize) -> Result<(), ServiceQuotaExceededException> {
    if current < MAX_CLUSTERS {
        return Ok(());
    }
    Err(ServiceQuotaExceededException {
        message: format!(
            "The maximum number of elastic clusters ({MAX_CLUSTERS}) has been reached; delete one before creating another"
        ),
    })
}

/// Refuse a create that would take the snapshot count past [`MAX_CLUSTER_SNAPSHOTS`].
pub fn ensure_snapshot_capacity(current: usize) -> Result<(), ServiceQuotaExceededException> {
    if current < MAX_CLUSTER_SNAPSHOTS {
        return Ok(());
    }
    Err(ServiceQuotaExceededException {
        message: format!(
            "The maximum number of cluster snapshots ({MAX_CLUSTER_SNAPSHOTS}) has been reached; delete one before creating another"
        ),
    })
}

/// Refuse a tag map larger than [`MAX_TAGS`], or a key or value longer than the
/// AWS-wide tag limits. Checked before the map is stored, so one request's stored
/// footprint is bounded.
pub fn validate_tags(
    member: &str,
    tags: &::std::collections::HashMap<String, String>,
) -> Result<(), ValidationException> {
    if tags.len() > MAX_TAGS {
        return Err(field_validation_failed(
            member,
            &format!("Member must not have more than {MAX_TAGS} entries"),
        ));
    }
    for (key, value) in tags {
        if key.is_empty() || key.chars().count() > MAX_TAG_KEY_LEN {
            return Err(field_validation_failed(
                member,
                &format!("Member keys must have length between 1 and {MAX_TAG_KEY_LEN}, inclusive"),
            ));
        }
        if value.chars().count() > MAX_TAG_VALUE_LEN {
            return Err(field_validation_failed(
                member,
                &format!("Member values must have length at most {MAX_TAG_VALUE_LEN}"),
            ));
        }
    }
    Ok(())
}

/// Refuse a `subnetIds` / `vpcSecurityGroupIds` list longer than
/// [`MAX_NETWORK_IDS`], or an entry longer than [`MAX_NETWORK_ID_LEN`].
pub fn validate_id_list(member: &str, ids: &[String]) -> Result<(), ValidationException> {
    if ids.len() > MAX_NETWORK_IDS {
        return Err(field_validation_failed(
            member,
            &format!("Member must not have more than {MAX_NETWORK_IDS} entries"),
        ));
    }
    if ids
        .iter()
        .any(|id| id.is_empty() || id.chars().count() > MAX_NETWORK_ID_LEN)
    {
        return Err(field_validation_failed(
            member,
            &format!("Member entries must have length between 1 and {MAX_NETWORK_ID_LEN}, inclusive"),
        ));
    }
    Ok(())
}

/// Refuse a `snapshotName` outside [`SNAPSHOT_NAME_LEN`].
pub fn validate_snapshot_name(member: &str, value: &str) -> Result<(), ValidationException> {
    let (min, max) = SNAPSHOT_NAME_LEN;
    if (min..=max).contains(&value.chars().count()) {
        return Ok(());
    }
    Err(field_validation_failed(
        member,
        &format!("Member must have length between {min} and {max}, inclusive"),
    ))
}

/// Refuse a `kmsKeyId` longer than [`MAX_KMS_KEY_ID_LEN`].
pub fn validate_kms_key_id(member: &str, value: &str) -> Result<(), ValidationException> {
    if !value.is_empty() && value.chars().count() <= MAX_KMS_KEY_ID_LEN {
        return Ok(());
    }
    Err(field_validation_failed(
        member,
        &format!("Member must have length between 1 and {MAX_KMS_KEY_ID_LEN}, inclusive"),
    ))
}

/// Refuse a `preferredMaintenanceWindow` outside the documented
/// `ddd:hh24:mi-ddd:hh24:mi` form.
///
/// Source: `CreateClusterInput$preferredMaintenanceWindow` doc — "Format:
/// ddd:hh24:mi-ddd:hh24:mi … Valid days: Mon, Tue, Wed, Thu, Fri, Sat, Sun".
pub fn validate_maintenance_window(member: &str, value: &str) -> Result<(), ValidationException> {
    let well_formed = value
        .split_once('-')
        .is_some_and(|(start, end)| is_day_time(start) && is_day_time(end));
    if well_formed {
        Ok(())
    } else {
        Err(field_validation_failed(
            member,
            "Member must be of the form ddd:hh24:mi-ddd:hh24:mi, with ddd one of Mon, Tue, Wed, Thu, Fri, Sat, Sun",
        ))
    }
}

/// Refuse a `preferredBackupWindow` outside the documented `hh24:mi-hh24:mi` form.
///
/// Source: `CreateClusterInput$preferredBackupWindow` doc — it is "The daily time
/// range during which automated backups are created", so it names a time of day and
/// no day of the week.
pub fn validate_backup_window(member: &str, value: &str) -> Result<(), ValidationException> {
    let well_formed = value
        .split_once('-')
        .is_some_and(|(start, end)| is_time_of_day(start) && is_time_of_day(end));
    if well_formed {
        Ok(())
    } else {
        Err(field_validation_failed(
            member,
            "Member must be of the form hh24:mi-hh24:mi",
        ))
    }
}

/// `ddd:hh24:mi` — a valid day name followed by a wall-clock time.
fn is_day_time(value: &str) -> bool {
    match value.split_once(':') {
        Some((day, time)) => MAINTENANCE_DAYS.contains(&day) && is_time_of_day(time),
        None => false,
    }
}

/// `hh24:mi` — two digits of hour below 24 and two of minute below 60.
fn is_time_of_day(value: &str) -> bool {
    let Some((hour, minute)) = value.split_once(':') else {
        return false;
    };
    let hour_ok = hour.len() == 2 && matches!(hour.parse::<u32>(), Ok(h) if h < 24);
    let minute_ok = minute.len() == 2 && matches!(minute.parse::<u32>(), Ok(m) if m < 60);
    hour_ok && minute_ok
}

/// The `preferredMaintenanceWindow` a cluster created without one reports.
///
/// AWS picks "a 30-minute window selected at random from an 8-hour block of time for
/// each Amazon Web Services Region, occurring on a random day of the week"
/// (`CreateClusterInput$preferredMaintenanceWindow`). The emulator derives it from
/// the cluster's own name instead, so it is stable across reads of one cluster and
/// still differs between clusters; the shape is the documented
/// `ddd:hh24:mi-ddd:hh24:mi` and the span is the documented 30-minute minimum.
pub fn default_maintenance_window(cluster_name: &str) -> String {
    let digest = name_digest(cluster_name);
    let day_index = (digest % MAINTENANCE_DAYS.len() as u64) as usize;
    let start_hour = ((digest / 7) % 24) as u32;
    let start_minute = if (digest / 168).is_multiple_of(2) { 0 } else { 30 };
    let (end_hour, end_minute) = if start_minute == 0 {
        (start_hour, 30)
    } else {
        ((start_hour + 1) % 24, 0)
    };
    // Rolling past midnight rolls the day with it, so the window stays 30 minutes.
    let end_day_index = if end_hour < start_hour {
        (day_index + 1) % MAINTENANCE_DAYS.len()
    } else {
        day_index
    };
    format!(
        "{}:{start_hour:02}:{start_minute:02}-{}:{end_hour:02}:{end_minute:02}",
        MAINTENANCE_DAYS[day_index], MAINTENANCE_DAYS[end_day_index]
    )
}

/// A deterministic digest of a resource name, used only to spread a derived default
/// (the maintenance window) across names.
///
/// `DefaultHasher::new()` starts from fixed keys, so one name always yields one
/// window; the value is stored on the record when the cluster is created, so nothing
/// depends on the digest being the same in a different build of the emulator.
fn name_digest(name: &str) -> u64 {
    use ::std::hash::{DefaultHasher, Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    hasher.finish()
}

/// The shard list a cluster of `shard_count` shards has.
///
/// `existing` is kept as far as it goes, so resizing a cluster neither renames the
/// shards it keeps nor loses their creation times; shards beyond the new count are
/// dropped and new ones are minted at `created_at`. Shard STATUS is owned by
/// [`ClusterRecord::begin_transition`], which the caller runs after resizing, so a
/// new shard starts at the same status as the cluster it belongs to.
pub fn shards_for(
    shard_count: i32,
    created_at: &str,
    existing: &[ShardRecord],
) -> Vec<ShardRecord> {
    let wanted = shard_count.max(0) as usize;
    let mut shards: Vec<ShardRecord> = existing.iter().take(wanted).cloned().collect();
    while shards.len() < wanted {
        shards.push(ShardRecord {
            shard_id: new_resource_id(),
            create_time: created_at.to_string(),
            status: STATUS_CREATING.to_string(),
        });
    }
    shards
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDocdbelasticChimeradblionfishservicelambdaState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDocdbelasticChimeradblionfishservicelambdaState>)
}
