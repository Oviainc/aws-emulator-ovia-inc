// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-memorydb-amazonmemorydb service.
//!
//! One store per emulator, reached from a handler through [`service_state`]. Every
//! modelled shape a handler answers with is BUILT FROM this state (see
//! [`StoredCluster::to_api`] / [`StoredAcl::to_api`] / [`StoredUser::to_api`]), so a
//! describe can only report what a create or update actually persisted — or, for the
//! resources an AWS account already has, what
//! [`ComAmazonawsMemorydbAmazonmemorydbState::ensure_builtin_users`] materialises.
//!
//! Besides the store itself this module owns the small amount of logic several
//! operations share — the request's region, the ARN/endpoint naming, shard layout,
//! pagination and the clock-owed status settle — so that the per-operation files in
//! `src/ops/` stay one-operation-wide and cannot drift from one another.

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::{
    Authentication, AuthenticationMode, Cluster, Endpoint, EngineVersionInfo, Node, ParameterGroup,
    SecurityGroupMembership, Shard, Tag, User, ACL,
};

/// The region an ARN or endpoint carries when the request is unsigned, matching the
/// region every generated emulator defaults to (see `engine::emulator_arn`).
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace (`smithy_query query="service_info"` →
/// `awsService.arnNamespace`).
pub const ARN_NAMESPACE: &str = "memorydb";

/// `CreateCluster$Port`: "The port number on which each of the nodes accepts
/// connections." MemoryDB speaks the Redis OSS / Valkey protocol, whose registered
/// port is 6379, and that is the port a cluster created without `Port` listens on.
pub const DEFAULT_PORT: i32 = 6379;

/// `CreateCluster$NumShards`: "The number of shards the cluster will contain. The
/// default value is 1."
pub const DEFAULT_NUM_SHARDS: i32 = 1;

/// `CreateCluster$NumReplicasPerShard`: "The number of replicas to apply to each
/// shard. The default value is 1."
pub const DEFAULT_NUM_REPLICAS_PER_SHARD: i32 = 1;

/// The most shards one cluster may have (MemoryDB's per-cluster shard quota).
///
/// An UPPER BOUND, not decoration: `NumShards` and `ShardConfiguration$ShardCount`
/// arrive as a bare `i32` the model puts no `@range` on, and they size the stored
/// layout, so every write path refuses a value above this before any layout is built.
/// It is also at most [`TOTAL_SLOTS`], so each shard still owns at least one slot.
pub const MAX_SHARDS_PER_CLUSTER: i32 = 500;

/// The most replicas one shard may have:
/// `CreateCluster$NumReplicasPerShard` — "The maximum is 5."
///
/// The other half of the layout bound above: nodes per cluster can never exceed
/// `MAX_SHARDS_PER_CLUSTER * (MAX_REPLICAS_PER_SHARD + 1)`, so the layout cannot be
/// sized by an unbounded request field.
pub const MAX_REPLICAS_PER_SHARD: i32 = 5;

/// The most clusters, the most ACLs, and the most users this emulator keeps.
///
/// A ceiling THIS EMULATOR chooses (AWS's own cluster/ACL/user quotas are per-account
/// and adjustable) so that one caller cannot grow the store without bound; AWS answers
/// its own exceeded quota with the same modelled faults
/// (`ClusterQuotaForCustomerExceededFault`, `ACLQuotaExceededFault`,
/// `UserQuotaExceededFault`).
pub const MAX_RESOURCES_PER_KIND: usize = 500;

/// The most passwords one user may hold: `Authentication$PasswordCount` — "The number
/// of passwords belonging to the user. The maximum is two."
pub const MAX_PASSWORDS_PER_USER: usize = 2;

/// The shortest password MemoryDB accepts for a user, from the password requirements
/// AWS documents for RBAC users ("Authenticating users with Access Control Lists"):
/// "must be at least 16 characters and no more than 128 characters in length".
pub const MIN_PASSWORD_LEN: usize = 16;

/// The longest password MemoryDB accepts, from the same documented requirement.
pub const MAX_PASSWORD_LEN: usize = 128;

/// The characters the same documented requirement excludes from a user password:
/// "cannot contain any of the following characters: ',' '\"' '/' '@'". Everything else
/// that is printable ASCII — including the rest of the punctuation — is accepted.
pub const FORBIDDEN_PASSWORD_CHARS: &[char] = &[',', '"', '/', '@'];

/// The number of hash slots a cluster spreads over its shards. Fixed by the Redis
/// OSS / Valkey cluster protocol MemoryDB implements (slots `0..=16383`).
pub const TOTAL_SLOTS: i32 = 16_384;

/// How long after the call that starts it a cluster or ACL stays in its transient
/// status before the clock owes the terminal one.
///
/// A span THIS EMULATOR chooses (AWS takes minutes): long enough that a caller acting
/// straight after the create still observes `creating`, short enough that a test that
/// waits for the resource is quick. Never zero, and never slept on — the due instant
/// is stored and materialised by whichever later call observes the resource.
pub const SETTLE_AFTER: Duration = Duration::from_millis(500);

/// One engine version this service offers, as `DescribeEngineVersions` reports it.
///
/// The version is held as its NUMERIC parts rather than as strings, so every value the
/// catalog answers with — `EngineVersion`, `EnginePatchVersion` and the
/// `ParameterGroupFamily` the engine default parameters belong to — is DERIVED from the
/// same three numbers and no two of them can disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineVersion {
    /// `EngineVersionInfo$Engine`: "The name of the engine for which version
    /// information is provided."
    pub engine: &'static str,
    pub major: u32,
    pub minor: u32,
    /// The patch release of `<major>.<minor>` this service runs.
    pub patch: u32,
}

/// The engine versions MemoryDB offers, in catalog order.
///
/// SERVICE metadata, not per-caller state: the list is the same for every account (as a
/// region's availability zones are), so it is a catalog rather than a map in
/// [`ComAmazonawsMemorydbAmazonmemorydbState`]. It lives HERE, beside the store, so that
/// every operation which has to resolve an engine version reads the one list and no two
/// of them can publish different versions.
///
/// Source: the AWS MemoryDB *Supported engine versions* documentation — MemoryDB runs
/// Redis OSS 6.2, 7.0 and 7.1, and Valkey 7.2 and 8.0, each at the patch release below.
pub const ENGINE_VERSIONS: &[EngineVersion] = &[
    EngineVersion { engine: ENGINE_REDIS, major: 6, minor: 2, patch: 6 },
    EngineVersion { engine: ENGINE_REDIS, major: 7, minor: 0, patch: 7 },
    EngineVersion { engine: ENGINE_REDIS, major: 7, minor: 1, patch: 1 },
    EngineVersion { engine: ENGINE_VALKEY, major: 7, minor: 2, patch: 6 },
    EngineVersion { engine: ENGINE_VALKEY, major: 8, minor: 0, patch: 0 },
];

/// `CreateCluster$Engine`: "The name of the engine to be used for the cluster." MemoryDB
/// names its two engines with these lower-case tokens.
pub const ENGINE_REDIS: &str = "redis";
pub const ENGINE_VALKEY: &str = "valkey";

/// The prefix every MemoryDB parameter group family carries, in front of the engine name
/// and its major version (`memorydb_redis7`, `memorydb_valkey8`).
const PARAMETER_GROUP_FAMILY_PREFIX: &str = "memorydb_";

impl EngineVersion {
    /// `EngineVersionInfo$EngineVersion` — "The engine version": `<major>.<minor>`.
    pub fn version(&self) -> String {
        format!("{}.{}", self.major, self.minor)
    }

    /// `EngineVersionInfo$EnginePatchVersion` — "The patched engine version":
    /// `<major>.<minor>.<patch>`.
    pub fn patch_version(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }

    /// `EngineVersionInfo$ParameterGroupFamily` — "the name of the parameter group
    /// family to which the engine default parameters apply". A MemoryDB family covers
    /// one engine at one MAJOR version, so every minor version of an engine's major
    /// version shares `memorydb_<engine><major>`.
    pub fn parameter_group_family(&self) -> String {
        format!("{PARAMETER_GROUP_FAMILY_PREFIX}{}{}", self.engine, self.major)
    }

    /// Whether this is the DEFAULT version of its parameter group family: the greatest
    /// minor version the catalog publishes for this engine and major version. Default-ness
    /// is a property of the family (the engine defaults a parameter group inherits), not
    /// of one request, so `DescribeEngineVersions$DefaultOnly` narrows to these.
    pub fn is_family_default(&self) -> bool {
        !ENGINE_VERSIONS.iter().any(|other| {
            other.engine == self.engine && other.major == self.major && other.minor > self.minor
        })
    }

    /// This version's position in the catalog listing, for [`paginate`]'s token.
    ///
    /// Zero-padded, so the ordering the key gives is NUMERIC on the version parts: a
    /// future `10.0` sorts after `8.0` rather than before it, which a bare `"10.0"`
    /// string would not.
    pub fn catalog_key(&self) -> String {
        format!("{}/{:05}/{:05}", self.engine, self.major, self.minor)
    }

    /// The modelled `EngineVersionInfo` for this catalog entry.
    pub fn to_api(&self) -> EngineVersionInfo {
        EngineVersionInfo {
            engine: Some(self.engine.to_string()),
            engine_version: Some(self.version()),
            engine_patch_version: Some(self.patch_version()),
            parameter_group_family: Some(self.parameter_group_family()),
        }
    }
}

/// `Cluster$Status`: "The status of the cluster. For example, Available, Updating,
/// Creating." MemoryDB renders them lower-case on the wire.
pub mod cluster_status {
    pub const CREATING: &str = "creating";
    pub const AVAILABLE: &str = "available";
    pub const UPDATING: &str = "updating";
    pub const DELETING: &str = "deleting";
}

/// `ACL$Status`: "Indicates ACL status. Can be \"creating\", \"active\",
/// \"modifying\", \"deleting\"."
pub mod acl_status {
    pub const CREATING: &str = "creating";
    pub const ACTIVE: &str = "active";
}

/// `User$Status`: "Indicates the user status. Can be \"active\", \"modifying\" or
/// \"deleting\"."
///
/// `DELETING` is TERMINAL for this store: a deleted user is removed outright, so the
/// status only ever appears on the response that reports the deletion, never on a
/// user a later call can still read.
pub mod user_status {
    pub const ACTIVE: &str = "active";
    pub const MODIFYING: &str = "modifying";
    pub const DELETING: &str = "deleting";
}

/// The two `InputAuthenticationType` tokens a request may ASK for, named so the
/// handlers branch on a constant rather than a literal. The full accepted list stays
/// the GENERATED catalog `crate::ops::types::INPUT_AUTHENTICATION_TYPE_VALUES`, which
/// is what a refusal quotes, so neither can drift from the model.
pub mod input_authentication_type {
    pub const PASSWORD: &str = "password";
    pub const IAM: &str = "iam";
}

/// The user every MemoryDB account already has: MemoryDB creates a user named
/// `default` (an `open-access` member) in every account, so `DescribeUsers` answers
/// with it before any `CreateUser` call ever runs.
pub const DEFAULT_USER_NAME: &str = "default";

/// The access string MemoryDB gives that built-in user: full access to every key
/// (`~*`), every pub/sub channel (`&*`) and every command (`+@all`), in the Redis OSS /
/// Valkey ACL syntax `User$AccessString` carries.
pub const DEFAULT_USER_ACCESS_STRING: &str = "on ~* &* +@all";

/// `AuthenticationType` (`password` | `no-password` | `iam`): the built-in user
/// authenticates without a password, so it carries no password count.
pub const NO_PASSWORD_AUTHENTICATION: &str = "no-password";

/// The status a shard, a node and an attached security group report once the cluster
/// carrying them is serving.
const AVAILABLE: &str = "available";

/// `Cluster$SnsTopicStatus` of a cluster that was given a notification topic: the
/// topic is being notified. `UpdateCluster$SnsTopicStatus` lets a caller set another.
pub const SNS_TOPIC_ACTIVE: &str = "active";

/// `AZStatus` (`multiaz` | `singleaz`): a shard with at least one replica is spread
/// over more than one availability zone, a shard without replicas is not.
mod availability_mode {
    pub const MULTI_AZ: &str = "multiaz";
    pub const SINGLE_AZ: &str = "singleaz";
}

/// Per-service state for com-amazonaws-memorydb-amazonmemorydb. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMemorydbAmazonmemorydbState {
    /// Clusters by `ClusterName`. A `BTreeMap`, so every list is in name order and
    /// pagination over it is stable.
    pub clusters: BTreeMap<String, StoredCluster>,
    /// ACLs by `ACLName`.
    pub acls: BTreeMap<String, StoredAcl>,
    /// Users by `UserName`, including the built-in `default` user
    /// [`ComAmazonawsMemorydbAmazonmemorydbState::ensure_builtin_users`] materialises.
    /// A `BTreeMap`, so every listing is in name order and pagination over it is stable.
    pub users: BTreeMap<String, StoredUser>,
    /// Whether the built-in users have already been materialised in this store, so
    /// that materialising them is a one-off and never resurrects a deleted user.
    pub builtin_users_materialised: bool,
    /// Parameter groups by `ParameterGroupName`, in name order like the maps above,
    /// so a listing over them pages stably.
    pub parameter_groups: BTreeMap<String, StoredParameterGroup>,
    /// Tags by resource ARN, written by the `Tags` member of a create. The
    /// `ListTags`/`TagResource` slice (still scaffolded) reads the same map.
    pub tags: BTreeMap<String, Vec<(String, String)>>,
}

impl ComAmazonawsMemorydbAmazonmemorydbState {
    /// Materialise every status transition the CLOCK already owes at `now`.
    ///
    /// Called at the top of every handler. This is not a mutation the caller asked
    /// for: each transition was due at `now` whether or not anybody called, so a
    /// reporting operation may apply it and two reads at the same instant agree.
    pub fn settle(&mut self, now: Instant) {
        for cluster in self.clusters.values_mut() {
            if cluster.settles_at.is_some_and(|due| now >= due) {
                cluster.status = cluster_status::AVAILABLE.to_string();
                cluster.settles_at = None;
            }
        }
        for acl in self.acls.values_mut() {
            if acl.settles_at.is_some_and(|due| now >= due) {
                acl.status = acl_status::ACTIVE.to_string();
                acl.settles_at = None;
            }
        }
        for user in self.users.values_mut() {
            if user.settles_at.is_some_and(|due| now >= due) {
                user.status = user_status::ACTIVE.to_string();
                user.settles_at = None;
            }
        }
    }

    /// Materialise the users this account ALREADY HAD before any call: MemoryDB
    /// creates the `default` user in every account, so it is part of the store's
    /// initial contents rather than something a caller made.
    ///
    /// The engine hands every service an EMPTY store, so the built-ins are written on
    /// first touch instead of by a constructor; the flag makes that a one-off, so a
    /// later delete of a built-in user is not undone by the next call. `region` is the
    /// requesting caller's, exactly as every other resource in this store takes the
    /// region of the call that brought it into being.
    pub fn ensure_builtin_users(&mut self, region: &str) {
        if self.builtin_users_materialised {
            return;
        }
        self.builtin_users_materialised = true;
        self.users.insert(
            DEFAULT_USER_NAME.to_string(),
            StoredUser {
                name: DEFAULT_USER_NAME.to_string(),
                status: user_status::ACTIVE.to_string(),
                access_string: DEFAULT_USER_ACCESS_STRING.to_string(),
                authentication_type: NO_PASSWORD_AUTHENTICATION.to_string(),
                password_count: None,
                // The account's own user is ready before any call, so the clock owes
                // it no transition.
                settles_at: None,
                arn: user_arn(region, DEFAULT_USER_NAME),
            },
        );
    }

    /// The names of the clusters currently referencing `acl_name`, in name order —
    /// what `ACL$Clusters` ("A list of clusters associated with the ACL") reports.
    pub fn clusters_using_acl(&self, acl_name: &str) -> Vec<String> {
        self.clusters
            .values()
            .filter(|cluster| cluster.acl_name == acl_name)
            .map(|cluster| cluster.name.clone())
            .collect()
    }

    /// The same membership for EVERY ACL, in ONE pass over the clusters.
    ///
    /// A listing uses this rather than [`Self::clusters_using_acl`] per ACL, so the
    /// work of describing N ACLs is linear in the store rather than N scans of it.
    pub fn acl_membership(&self) -> BTreeMap<&str, Vec<String>> {
        let mut membership: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for cluster in self.clusters.values() {
            membership
                .entry(cluster.acl_name.as_str())
                .or_default()
                .push(cluster.name.clone());
        }
        membership
    }

    /// The names of the ACLs whose `UserNames` include each user, in name order —
    /// what `User$ACLNames` ("The names of the Access Control Lists to which the user
    /// belongs") reports. DERIVED from the ACLs, exactly as `ACL$Clusters` is derived
    /// from the clusters, so one membership cannot disagree with the other.
    ///
    /// ONE pass over the ACLs answers EVERY user, so describing N users costs one scan
    /// of the ACLs rather than N; a single-user describe takes its own entry out of
    /// the result rather than scanning again under a second copy of this rule.
    pub fn user_membership(&self) -> BTreeMap<&str, Vec<String>> {
        let mut membership: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for acl in self.acls.values() {
            for user_name in &acl.user_names {
                membership
                    .entry(user_name.as_str())
                    .or_default()
                    .push(acl.name.clone());
            }
        }
        membership
    }

    /// Take `user_name` out of the `UserNames` of every ACL that lists it, answering
    /// the names of those ACLs in name order.
    ///
    /// `DeleteUser` — "The user will be removed from all ACLs and in turn removed from
    /// all clusters" — is what owes this: membership lives ONCE, on the ACL
    /// ([`Self::user_membership`] derives `User$ACLNames` from it), so a user that is
    /// gone has to leave the ACLs rather than linger in a list no user resolves
    /// through. The returned names are the membership the user held UP TO this call,
    /// which is what the delete reports back.
    pub fn remove_user_from_every_acl(&mut self, user_name: &str) -> Vec<String> {
        let mut left_behind = Vec::new();
        for acl in self.acls.values_mut() {
            let before = acl.user_names.len();
            acl.user_names.retain(|name| name != user_name);
            if acl.user_names.len() != before {
                left_behind.push(acl.name.clone());
            }
        }
        left_behind
    }
}

/// A cluster as this emulator persists it: every member `CreateCluster` accepted, plus
/// the values the service itself derives (ARN, endpoint, shard layout, status).
#[derive(Debug, Clone)]
pub struct StoredCluster {
    pub name: String,
    /// The region this cluster was CREATED in, carried in its ARN, its configuration
    /// endpoint and every node endpoint and availability zone. Stored, so a later call
    /// signed for another region reports the cluster where it actually lives instead of
    /// rewriting half of it.
    pub region: String,
    pub description: Option<String>,
    pub status: String,
    /// When the clock owes this cluster `available`, or `None` once it is settled.
    pub settles_at: Option<Instant>,
    pub node_type: String,
    pub number_of_shards: i32,
    pub num_replicas_per_shard: i32,
    pub acl_name: String,
    pub multi_region_cluster_name: Option<String>,
    pub parameter_group_name: Option<String>,
    pub subnet_group_name: Option<String>,
    pub security_group_ids: Vec<String>,
    pub maintenance_window: Option<String>,
    pub snapshot_window: Option<String>,
    pub snapshot_retention_limit: Option<i32>,
    pub sns_topic_arn: Option<String>,
    pub sns_topic_status: Option<String>,
    pub tls_enabled: Option<bool>,
    pub kms_key_id: Option<String>,
    pub engine: Option<String>,
    pub engine_version: Option<String>,
    pub auto_minor_version_upgrade: Option<bool>,
    pub data_tiering: Option<bool>,
    pub network_type: Option<String>,
    pub ip_discovery: Option<String>,
    pub port: i32,
    pub endpoint_address: String,
    pub arn: String,
    pub shards: Vec<StoredShard>,
    pub created_at: SystemTime,
}

/// One shard of a stored cluster, laid out once at create time and persisted, so
/// every later call reports the same shards, slots and nodes.
#[derive(Debug, Clone)]
pub struct StoredShard {
    pub name: String,
    pub status: String,
    pub slots: String,
    pub nodes: Vec<StoredNode>,
}

/// One node of a stored shard.
#[derive(Debug, Clone)]
pub struct StoredNode {
    pub name: String,
    pub status: String,
    pub availability_zone: String,
    pub endpoint_address: String,
    pub port: i32,
    pub created_at: SystemTime,
}

/// A parameter group as this emulator persists it: the three members
/// `CreateParameterGroup` accepts plus the ARN the service itself mints.
///
/// A parameter group has NO status member in the model (`ParameterGroup` is
/// `Name`/`Family`/`Description`/`ARN`), so there is no transient status to settle and
/// nothing for the clock to owe it.
#[derive(Debug, Clone)]
pub struct StoredParameterGroup {
    pub name: String,
    /// `ParameterGroup$Family`: "The name of the parameter group family that this
    /// parameter group is compatible with." Stored verbatim — the model publishes no
    /// catalog of families, so the service records the caller's rather than matching it
    /// against a guessed list.
    pub family: String,
    pub description: Option<String>,
    pub arn: String,
}

/// An ACL as this emulator persists it. `Clusters` is NOT stored: it is derived from
/// the clusters currently referencing the ACL (see
/// [`ComAmazonawsMemorydbAmazonmemorydbState::clusters_using_acl`]).
#[derive(Debug, Clone)]
pub struct StoredAcl {
    pub name: String,
    pub status: String,
    /// When the clock owes this ACL `active`, or `None` once it is settled.
    pub settles_at: Option<Instant>,
    pub user_names: Vec<String>,
    pub arn: String,
}

/// A user as this emulator persists it. `ACLNames` is NOT stored: it is derived from
/// the ACLs currently listing the user (see
/// [`ComAmazonawsMemorydbAmazonmemorydbState::user_membership`]).
///
/// The user's PASSWORDS are deliberately absent too: `Authentication$PasswordCount` is
/// the only thing AWS reports about them, so only the count is kept and the secrets
/// themselves are never stored or returned.
#[derive(Debug, Clone)]
pub struct StoredUser {
    pub name: String,
    pub status: String,
    pub access_string: String,
    /// `Authentication$Type`, one of [`AUTHENTICATION_TYPE_VALUES`].
    ///
    /// [`AUTHENTICATION_TYPE_VALUES`]: crate::ops::types::AUTHENTICATION_TYPE_VALUES
    pub authentication_type: String,
    /// `Authentication$PasswordCount`: "The number of passwords belonging to the
    /// user", absent for a user that authenticates without one.
    pub password_count: Option<i32>,
    /// When the clock owes this user [`user_status::ACTIVE`] again, or `None` once it
    /// is settled. Set by the operation that moves the user off `active` (an update
    /// answers with it `modifying`), and materialised by [`settle`].
    ///
    /// [`settle`]: ComAmazonawsMemorydbAmazonmemorydbState::settle
    pub settles_at: Option<Instant>,
    pub arn: String,
}

impl StoredUser {
    /// The modelled `User` for this stored user. `acl_names` is the derived membership
    /// list, which only the state as a whole can compute.
    pub fn to_api(&self, acl_names: Vec<String>) -> User {
        User {
            name: Some(self.name.clone()),
            status: Some(self.status.clone()),
            access_string: Some(self.access_string.clone()),
            acl_names: Some(acl_names),
            // Left ABSENT deliberately: nothing in this slice tracks the engine
            // versions a user is usable with, and reporting one would be a
            // plausible-looking value for state nothing keeps — the same choice
            // `ACL$MinimumEngineVersion` makes.
            minimum_engine_version: None,
            authentication: Some(Authentication {
                r#type: Some(self.authentication_type.clone()),
                password_count: self.password_count,
            }),
            arn: Some(self.arn.clone()),
        }
    }
}

impl StoredCluster {
    /// The modelled `Cluster` for this stored cluster.
    ///
    /// `DescribeClusters$ShowShardDetails` is "An optional flag that can be included
    /// in the request to retrieve information about the individual shard(s)", so
    /// `Shards` is populated only when the caller asked for it.
    pub fn to_api(&self, show_shard_details: bool) -> Cluster {
        Cluster {
            name: Some(self.name.clone()),
            description: self.description.clone(),
            status: Some(self.status.clone()),
            pending_updates: None,
            multi_region_cluster_name: self.multi_region_cluster_name.clone(),
            number_of_shards: Some(self.number_of_shards),
            shards: show_shard_details
                .then(|| self.shards.iter().map(StoredShard::to_api).collect()),
            availability_mode: Some(
                if self.num_replicas_per_shard > 0 {
                    availability_mode::MULTI_AZ
                } else {
                    availability_mode::SINGLE_AZ
                }
                .to_string(),
            ),
            cluster_endpoint: Some(Endpoint {
                address: Some(self.endpoint_address.clone()),
                port: self.port,
            }),
            node_type: Some(self.node_type.clone()),
            engine: self.engine.clone(),
            engine_version: self.engine_version.clone(),
            engine_patch_version: None,
            parameter_group_name: self.parameter_group_name.clone(),
            // Left ABSENT deliberately: `ParameterGroupStatus` has its own vocabulary
            // (a parameter group is in sync with the cluster, or being applied to it),
            // and nothing tracks the application of a parameter group to a cluster —
            // the operations that would (`UpdateParameterGroup`, `UpdateCluster`'s
            // `ParameterGroupName`) do not record it. Reporting the cluster's own
            // status here would be a plausible-looking token for state nothing tracks.
            parameter_group_status: None,
            security_groups: Some(
                self.security_group_ids
                    .iter()
                    .map(|id| SecurityGroupMembership {
                        security_group_id: Some(id.clone()),
                        status: Some(AVAILABLE.to_string()),
                    })
                    .collect(),
            ),
            subnet_group_name: self.subnet_group_name.clone(),
            tls_enabled: self.tls_enabled,
            kms_key_id: self.kms_key_id.clone(),
            arn: Some(self.arn.clone()),
            sns_topic_arn: self.sns_topic_arn.clone(),
            sns_topic_status: self.sns_topic_status.clone(),
            snapshot_retention_limit: self.snapshot_retention_limit,
            maintenance_window: self.maintenance_window.clone(),
            snapshot_window: self.snapshot_window.clone(),
            acl_name: Some(self.acl_name.clone()),
            auto_minor_version_upgrade: self.auto_minor_version_upgrade,
            // `DataTieringStatus` is an enum of the strings "true"/"false", not a
            // boolean, so the stored flag renders as its wire token.
            data_tiering: self
                .data_tiering
                .map(|enabled| if enabled { "true" } else { "false" }.to_string()),
            network_type: self.network_type.clone(),
            ip_discovery: self.ip_discovery.clone(),
        }
    }
}

impl StoredShard {
    /// The modelled `Shard` for this stored shard.
    pub fn to_api(&self) -> Shard {
        Shard {
            name: Some(self.name.clone()),
            status: Some(self.status.clone()),
            slots: Some(self.slots.clone()),
            number_of_nodes: Some(i32::try_from(self.nodes.len()).unwrap_or(i32::MAX)),
            nodes: Some(self.nodes.iter().map(StoredNode::to_api).collect()),
        }
    }
}

impl StoredNode {
    /// The modelled `Node` for this stored node.
    pub fn to_api(&self) -> Node {
        Node {
            name: Some(self.name.clone()),
            status: Some(self.status.clone()),
            availability_zone: Some(self.availability_zone.clone()),
            create_time: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            endpoint: Some(Endpoint {
                address: Some(self.endpoint_address.clone()),
                port: self.port,
            }),
        }
    }
}

impl StoredParameterGroup {
    /// The modelled `ParameterGroup` for this stored parameter group.
    pub fn to_api(&self) -> ParameterGroup {
        ParameterGroup {
            name: Some(self.name.clone()),
            family: Some(self.family.clone()),
            description: self.description.clone(),
            arn: Some(self.arn.clone()),
        }
    }
}

impl StoredAcl {
    /// The modelled `ACL` for this stored ACL. `clusters` is the derived membership
    /// list, which only the state as a whole can compute.
    pub fn to_api(&self, clusters: Vec<String>) -> ACL {
        ACL {
            name: Some(self.name.clone()),
            status: Some(self.status.clone()),
            user_names: Some(self.user_names.clone()),
            minimum_engine_version: None,
            pending_changes: None,
            clusters: Some(clusters),
            arn: Some(self.arn.clone()),
        }
    }
}

/// Why an `AuthenticationMode` cannot be accepted, in the two classes MemoryDB models
/// separately: a member whose VALUE breaks a rule, and members that cannot be
/// COMBINED. The caller maps each onto the modelled error its own operation declares
/// (`InvalidParameterValueException` / `InvalidParameterCombinationException`), so one
/// rule serves every operation that takes an `AuthenticationMode`.
#[derive(Debug, Clone)]
pub enum AuthenticationModeRejection {
    /// A member's value is outside what the service accepts.
    Value(String),
    /// The members supplied cannot hold together.
    Combination(String),
}

/// The `Authentication` a stored user gets from the `AuthenticationMode` a request
/// supplies, or why the mode is refused.
///
/// The documented password requirements (AWS, "Authenticating users with Access
/// Control Lists") are enforced here rather than per operation, because `CreateUser`
/// and `UpdateUser` take the same member and so must refuse the same values:
///
/// * `Type` names an `InputAuthenticationType` (`password` or `iam`);
/// * `password` requires between 1 and [`MAX_PASSWORDS_PER_USER`] passwords, `iam`
///   accepts none at all;
/// * each password is printable ASCII, [`MIN_PASSWORD_LEN`]..=[`MAX_PASSWORD_LEN`]
///   characters long, and carries none of [`FORBIDDEN_PASSWORD_CHARS`].
pub fn authentication_from_mode(
    mode: &AuthenticationMode,
) -> Result<(String, Option<i32>), AuthenticationModeRejection> {
    let passwords = mode.passwords.as_deref().unwrap_or(&[]);
    let mode_type = mode.r#type.as_deref().unwrap_or_default();
    match mode_type {
        input_authentication_type::PASSWORD => {
            if passwords.is_empty() {
                return Err(AuthenticationModeRejection::Combination(format!(
                    "AuthenticationMode.Passwords is required when \
                     AuthenticationMode.Type is {}.",
                    input_authentication_type::PASSWORD
                )));
            }
            if passwords.len() > MAX_PASSWORDS_PER_USER {
                return Err(AuthenticationModeRejection::Value(format!(
                    "A user can have at most {MAX_PASSWORDS_PER_USER} passwords."
                )));
            }
            for password in passwords {
                validate_password(password)?;
            }
            // At most `MAX_PASSWORDS_PER_USER` (2) — anything larger was refused just
            // above — so the conversion is exact. The fallback is unreachable and is
            // there only to keep the handler panic-free.
            let count = i32::try_from(passwords.len()).unwrap_or(i32::MAX);
            Ok((mode_type.to_string(), Some(count)))
        }
        input_authentication_type::IAM => {
            if !passwords.is_empty() {
                return Err(AuthenticationModeRejection::Combination(format!(
                    "AuthenticationMode.Passwords cannot be supplied when \
                     AuthenticationMode.Type is {}.",
                    input_authentication_type::IAM
                )));
            }
            // An IAM-authenticated user holds no passwords, so there is no count to
            // report for it.
            Ok((mode_type.to_string(), None))
        }
        other => Err(AuthenticationModeRejection::Value(format!(
            "AuthenticationMode.Type must be one of {} — got {other:?}.",
            // The GENERATED catalog, so the refusal cannot name a different set from
            // the one the model declares.
            crate::ops::types::INPUT_AUTHENTICATION_TYPE_VALUES.join(", ")
        ))),
    }
}

/// One password against the documented requirements. The message names the broken
/// rule and never repeats the password itself.
fn validate_password(password: &str) -> Result<(), AuthenticationModeRejection> {
    if !(MIN_PASSWORD_LEN..=MAX_PASSWORD_LEN).contains(&password.chars().count()) {
        return Err(AuthenticationModeRejection::Value(format!(
            "A user password must be between {MIN_PASSWORD_LEN} and {MAX_PASSWORD_LEN} \
             characters long."
        )));
    }
    // "Must be only printable ASCII characters": the ASCII graphic range plus the
    // space, so a control character or any non-ASCII character is refused.
    if !password
        .chars()
        .all(|character| character.is_ascii_graphic() || character == ' ')
    {
        return Err(AuthenticationModeRejection::Value(
            "A user password must contain only printable ASCII characters.".to_string(),
        ));
    }
    if password
        .chars()
        .any(|character| FORBIDDEN_PASSWORD_CHARS.contains(&character))
    {
        return Err(AuthenticationModeRejection::Value(format!(
            "A user password must not contain any of the characters {}.",
            FORBIDDEN_PASSWORD_CHARS
                .iter()
                .map(|character| format!("{character:?}"))
                .collect::<Vec<_>>()
                .join(" ")
        )));
    }
    Ok(())
}

/// The region this request was signed for, or [`DEFAULT_REGION`] when it carries no
/// SigV4 credential scope. Every ARN and endpoint a handler mints goes through here,
/// so a resource's region is the caller's rather than a constant.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:memorydb:<region>:<account>:cluster/<name>`.
pub fn cluster_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("cluster/{name}"))
}

/// `arn:aws:memorydb:<region>:<account>:acl/<name>`.
pub fn acl_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("acl/{name}"))
}

/// `arn:aws:memorydb:<region>:<account>:user/<name>`.
pub fn user_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("user/{name}"))
}

/// `arn:aws:memorydb:<region>:<account>:parametergroup/<name>`.
pub fn parameter_group_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("parametergroup/{name}"))
}

/// The per-cluster label AWS puts between the cluster name and the service domain in
/// an endpoint. AWS generates it randomly; this emulator derives it from the cluster's
/// name and region so the address a create returns is the address every later describe
/// returns, in this process and the next.
fn endpoint_suffix(region: &str, cluster_name: &str) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(format!("{region}/{cluster_name}").as_bytes());
    hex::encode(&digest[..3])
}

/// The cluster's configuration endpoint host:
/// `<name>.<suffix>.clustercfg.memorydb.<region>.amazonaws.com`.
pub fn cluster_endpoint_address(region: &str, cluster_name: &str) -> String {
    let suffix = endpoint_suffix(region, cluster_name);
    format!("{cluster_name}.{suffix}.clustercfg.memorydb.{region}.amazonaws.com")
}

/// A node's own endpoint host:
/// `<node>.<suffix>.<cluster>.memorydb.<region>.amazonaws.com`.
fn node_endpoint_address(region: &str, cluster_name: &str, node_name: &str) -> String {
    let suffix = endpoint_suffix(region, cluster_name);
    format!("{node_name}.{suffix}.{cluster_name}.memorydb.{region}.amazonaws.com")
}

/// Lay out `number_of_shards` shards, each with one primary plus
/// `num_replicas_per_shard` replicas, over the cluster's hash slots.
///
/// Called once, by the operation that changes the shard count; the result is persisted
/// so no two calls can compute a different layout for the same cluster.
///
/// BOTH factors are CLAMPED to [`MAX_SHARDS_PER_CLUSTER`] / [`MAX_REPLICAS_PER_SHARD`]
/// here as well as refused by the write paths, so neither this function's single
/// allocation nor its loop can ever be sized by an out-of-range request field, however
/// it is reached. The clamped product is at most
/// `MAX_SHARDS_PER_CLUSTER * (MAX_REPLICAS_PER_SHARD + 1)` nodes, which no i32
/// arithmetic below can overflow. The `debug_assert!`s say what the write paths already
/// guarantee, so a future caller that relies on the clamp instead fails loudly in a
/// debug build rather than being silently corrected.
pub fn build_shards(
    region: &str,
    cluster_name: &str,
    number_of_shards: i32,
    num_replicas_per_shard: i32,
    port: i32,
    created_at: SystemTime,
) -> Vec<StoredShard> {
    debug_assert!(
        (1..=MAX_SHARDS_PER_CLUSTER).contains(&number_of_shards),
        "number_of_shards {number_of_shards} must be refused by the caller, not clamped"
    );
    debug_assert!(
        (0..=MAX_REPLICAS_PER_SHARD).contains(&num_replicas_per_shard),
        "num_replicas_per_shard {num_replicas_per_shard} must be refused by the caller, \
         not clamped"
    );
    let shard_count = number_of_shards.clamp(1, MAX_SHARDS_PER_CLUSTER);
    let replicas = num_replicas_per_shard.clamp(0, MAX_REPLICAS_PER_SHARD);
    let base = TOTAL_SLOTS / shard_count;
    let remainder = TOTAL_SLOTS % shard_count;
    let mut next_slot = 0;
    let mut shards = Vec::with_capacity(shard_count as usize);
    for shard_index in 0..shard_count {
        // The first `remainder` shards carry one extra slot, so the shards together
        // cover `0..=16383` exactly once however many there are. `shard_count` is at
        // most TOTAL_SLOTS, so `base` is at least 1 and no shard is empty.
        let width = base + i32::from(shard_index < remainder);
        let first_slot = next_slot;
        let last_slot = first_slot + width - 1;
        next_slot = last_slot + 1;
        let shard_name = format!("{:04}", shard_index + 1);
        let nodes = (0..=replicas)
            .map(|node_index| {
                let node_name = format!("{cluster_name}-{shard_name}-{:03}", node_index + 1);
                StoredNode {
                    endpoint_address: node_endpoint_address(region, cluster_name, &node_name),
                    name: node_name,
                    status: AVAILABLE.to_string(),
                    // Nodes of one shard are spread over the region's zones, the
                    // primary and each replica landing in a different one.
                    availability_zone: format!(
                        "{region}{}",
                        char::from(b'a' + u8::try_from(node_index % 3).unwrap_or(0))
                    ),
                    port,
                    created_at,
                }
            })
            .collect();
        shards.push(StoredShard {
            name: shard_name,
            status: AVAILABLE.to_string(),
            slots: format!("{first_slot}-{last_slot}"),
            nodes,
        });
    }
    shards
}

/// One page of a name-ordered listing, plus the token that resumes after it.
///
/// `@paginated` on this service's listings names `MaxResults` as the page size and
/// `NextToken` as both tokens. The token is the name the NEXT page starts at, so
/// resuming means skipping every name that sorts before the token.
///
/// `None` means the token names no position in this listing — every name sorts before
/// it, so it was not minted for this listing (or everything from its target on was
/// deleted between pages). The caller answers that with its modelled invalid-parameter
/// error rather than serving a silently empty page: a token this emulator minted always
/// names a resource that existed when the page was served, so a token past the end is
/// reported rather than quietly treated as the end of the listing.
///
/// `limit` is an already-validated page size (the callers refuse a `MaxResults` below
/// 1 with a modelled 400 before converting), and it only ever caps a `take` over an
/// existing list, so it never sizes an allocation.
pub fn paginate<T>(
    entries: Vec<(String, T)>,
    limit: Option<usize>,
    next_token: Option<&str>,
) -> Option<(Vec<T>, Option<String>)> {
    let start = match next_token {
        Some(token) => entries.iter().position(|(name, _)| name.as_str() >= token)?,
        None => 0,
    };
    let mut remaining = entries.into_iter().skip(start);
    let page: Vec<(String, T)> = match limit {
        Some(limit) => remaining.by_ref().take(limit).collect(),
        None => remaining.by_ref().collect(),
    };
    let next = remaining.next().map(|(name, _)| name);
    Some((page.into_iter().map(|(_, item)| item).collect(), next))
}

/// The already-validated page size of a listing request: `None` for "every remaining
/// item". Only reached once the caller has refused a `MaxResults` below 1.
pub fn page_limit(max_results: Option<i32>) -> Option<usize> {
    max_results.and_then(|max| usize::try_from(max).ok())
}

/// The `(key, value)` pairs of a request's `Tags` member, for the by-ARN tag store.
pub fn tag_pairs(tags: Option<&Vec<Tag>>) -> Vec<(String, String)> {
    tags.map(|tags| {
        tags.iter()
            .map(|tag| {
                (
                    tag.key.clone().unwrap_or_default(),
                    tag.value.clone().unwrap_or_default(),
                )
            })
            .collect()
    })
    .unwrap_or_default()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMemorydbAmazonmemorydbState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMemorydbAmazonmemorydbState>)
}
