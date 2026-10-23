// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-rds-amazonrdsv19 service.
//!
//! The DB instance, DB snapshot, DB cluster, option group, DB subnet group, cluster
//! parameter group, event subscription and global cluster records the implemented
//! handlers read and write, plus the projection of a stored record onto the modelled
//! `DBInstance` / `DBSnapshot` / `DBCluster` / `OptionGroup` / `DBSubnetGroup` /
//! `DBClusterParameterGroup` / `EventSubscription` /
//! `GlobalCluster` wire shapes. These live here rather than in one operation file because
//! every implemented operation shares them: `CreateDBInstance`, `DescribeDBInstances`,
//! `ModifyDBInstance`, `CreateDBSnapshot`, `DescribeDBSnapshots`, `DeleteDBInstance`,
//! `CreateDBCluster`, `DescribeDBClusters`,
//! `CreateOptionGroup`, `ModifyOptionGroup`, `DescribeOptionGroups`,
//! `DeleteOptionGroup`, `CreateDBSubnetGroup`, `DescribeDBSubnetGroups`, the six
//! cluster-parameter-group operations, `CreateEventSubscription`,
//! `DescribeEventSubscriptions`, `CreateGlobalCluster` and `DescribeGlobalClusters` all
//! project the same records.
//!
//! RDS is a REGIONAL service, so every map is keyed by `(region, identifier)`: the
//! region a request is signed for scopes what it can see and create, one identifier
//! can name a different instance in each region, and a describe signed for one region
//! never reports another's.
//!
//! Keying by region is also what bounds the store and every scan, which matters
//! because the region comes from the request's own SigV4 credential scope and this
//! emulator authenticates nobody. Three bounds hold together: [`check_region`] admits
//! only a value of the published AWS region-id form (so a region key is at most 32
//! bytes), [`MAX_REGIONS`] caps how many distinct regions hold records at all, and the
//! per-region quotas cap the records in each. The store therefore holds at most
//! `MAX_REGIONS * DB_INSTANCE_QUOTA_PER_REGION` instances,
//! `MAX_REGIONS * DB_SNAPSHOT_QUOTA_PER_REGION` snapshots,
//! `MAX_REGIONS * DB_CLUSTER_QUOTA_PER_REGION` DB clusters — each carrying at most
//! [`MAX_CLUSTER_LIST_VALUES`] values in each of its stored string lists —
//! `MAX_REGIONS * OPTION_GROUP_QUOTA_PER_REGION` option groups and
//! `MAX_REGIONS * DB_SUBNET_GROUP_QUOTA_PER_REGION` DB subnet groups,
//! `MAX_REGIONS * DB_CLUSTER_PARAMETER_GROUP_QUOTA_PER_REGION` cluster parameter groups
//! — each of those carrying at most [`MAX_PARAMETERS_PER_GROUP`] parameters of at most
//! [`MAX_FREE_FORM_LEN`] characters, a family string of at most [`MAX_FREE_FORM_LEN`]
//! characters and at most [`MAX_TAGS_PER_RESOURCE`] tags of at most [`MAX_TAG_KEY_LEN`] /
//! [`MAX_TAG_VALUE_LEN`] characters — and at most one CA certificate override per region,
//! whatever sequence of requests arrives, and every
//! per-request scan reads one region's ordered range rather than the whole map.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::{
    Certificate, CertificateDetails, ClusterPendingModifiedValues, DBCluster, DBClusterMember,
    DBClusterOptionGroupStatus, DBClusterParameterGroup, DBClusterRole, DBInstance,
    DBParameterGroupStatus, DBSecurityGroupMembership, DBSnapshot, DBSubnetGroup,
    DomainMembership, Endpoint, EventCategoriesMap, EventSubscription, GlobalCluster,
    GlobalClusterMember, Option2, OptionGroup, OptionGroupMembership, OptionSetting, Parameter,
    PendingModifiedValues, ScalingConfigurationInfo, ServerlessV2ScalingConfigurationInfo, Subnet,
    Tag, VpcSecurityGroupMembership,
};

// The catalog of engine parameters each family publishes, and the rules over it, are
// SHARED with the Amazon DocumentDB and Amazon Neptune models of the same front door —
// this service serves their SDK clients' traffic too, since all three sign `rds` — so they
// live once in `service_common::rds_cluster_parameters` and are re-exported here: a handler
// reads them as `crate::state::<name>`.
pub use service_common::rds_cluster_parameters::{
    change_is_pending, default_apply_method, parameter_source_filter, published_parameter,
    published_parameters, validate_parameter_group_family, validate_parameter_group_name,
    validate_parameter_name, validate_parameter_setting, ClusterParameterSpec,
    ParameterSourceFilter, APPLY_METHOD_IMMEDIATE, APPLY_METHOD_PENDING_REBOOT,
    APPLY_TYPE_DYNAMIC, APPLY_TYPE_STATIC, DOCDB_CLUSTER_PARAMETERS,
    DOCDB_PARAMETER_GROUP_FAMILIES, MAX_PARAMETERS_PER_MODIFY, MAX_PARAMETER_GROUP_NAME_LEN,
    PARAMETER_SOURCE_ENGINE_DEFAULT, PARAMETER_SOURCE_SYSTEM, PARAMETER_SOURCE_USER,
};

pub mod option_catalog;

/// How long after the call that starts it a status transition this service owes
/// becomes due.
///
/// RDS takes minutes to bring a DB instance up; a generated emulator picks its own
/// span, and this one is deliberately non-zero so a caller that reads straight after
/// `CreateDBInstance` still observes the `creating` status (and the state errors that
/// go with it) exactly as it would against the real service. The due instant is
/// stored on the record and materialised by the next call — nothing sleeps.
pub const STATUS_TRANSITION_DELAY: Duration = Duration::from_millis(300);

/// The status a DB instance reports once a delete has been accepted for it: "If you
/// request a final DB snapshot, the status of the Amazon RDS DB instance is deleting
/// until the DB snapshot is created" (`DeleteDBInstance`). It is a transient status —
/// the instance is removed [`STATUS_TRANSITION_DELAY`] later, and then it is not found.
pub const DB_INSTANCE_STATUS_DELETING: &str = "deleting";

/// The ARN namespace and sigv4 signing name of this service, as its Smithy model's
/// `aws.api#service` trait declares them (`arnNamespace: rds`).
const ARN_NAMESPACE: &str = "rds";

/// How many DB instances one region may hold: Amazon RDS's published default quota
/// ("DB instances": 40 per Region). Reaching it is the modelled
/// `InstanceQuotaExceededFault` — "The request would result in the user exceeding the
/// allowed number of DB instances".
pub const DB_INSTANCE_QUOTA_PER_REGION: usize = 40;

/// How many manual DB snapshots one region may hold: Amazon RDS's published default
/// quota ("Manual DB snapshots": 100 per Region). Reaching it is the modelled
/// `SnapshotQuotaExceededFault`, which both `CreateDBSnapshot` and `DeleteDBInstance`
/// declare.
pub const DB_SNAPSHOT_QUOTA_PER_REGION: usize = 100;

/// How many cluster parameter groups one region may hold.
///
/// Emulator resource guard, NOT an AWS quota: AWS publishes none for cluster parameter
/// groups, and every group is created by a caller and reclaimed only by an explicit delete,
/// so the growth site carries a cap. Reaching it is the modelled
/// `DBParameterGroupQuotaExceededFault` — "this request would cause you to exceed the
/// allowed number of parameter groups" — rather than an unbounded store.
pub const DB_CLUSTER_PARAMETER_GROUP_QUOTA_PER_REGION: usize = 100;

/// How many parameters one cluster parameter group may carry.
///
/// Emulator resource guard, NOT an AWS quota: a group's parameter map is a growth site a
/// caller drives (`ModifyDBClusterParameterGroup` sets up to 20 parameters per call, and a
/// family this emulator publishes no catalog for admits any parameter name), so the group's
/// TOTAL is capped as well as each request's. It is comfortably above the seven parameters
/// the DocumentDB families publish; a modify that would leave a group past it is refused
/// with a 400 rather than stored.
pub const MAX_PARAMETERS_PER_GROUP: usize = 64;

/// How many option groups one region may hold: the allowance `CreateOptionGroup`
/// documents ("You can create up to 20 option groups"), which its modelled
/// `OptionGroupQuotaExceededFault` names in turn ("The quota of 20 option groups was
/// exceeded for this Amazon Web Services account"). AWS counts that allowance per
/// account and Region; this store is keyed by region, so it is enforced per region.
pub const OPTION_GROUP_QUOTA_PER_REGION: usize = 20;

/// How many DB subnet groups one region may hold: Amazon RDS's published default quota
/// ("DB subnet groups": 50 per Region). Reaching it is the modelled
/// `DBSubnetGroupQuotaExceededFault` — "The request would result in the user exceeding
/// the allowed number of DB subnet groups".
pub const DB_SUBNET_GROUP_QUOTA_PER_REGION: usize = 50;

/// How many subnets ONE DB subnet group may name: Amazon RDS's published default quota
/// ("Subnets per DB subnet group": 20). Reaching it is the modelled
/// `DBSubnetQuotaExceededFault` — "The request would result in the user exceeding the
/// allowed number of subnets in a DB subnet groups".
pub const MAX_SUBNETS_PER_DB_SUBNET_GROUP: usize = 20;

/// How many event notification subscriptions one region may hold: Amazon RDS's published
/// default quota ("Event subscriptions": 20 per Region). Reaching it is the modelled
/// `EventSubscriptionQuotaExceededFault`, which `CreateEventSubscription` declares ("You
/// have reached the maximum number of event subscriptions").
pub const EVENT_SUBSCRIPTION_QUOTA_PER_REGION: usize = 20;

/// How many Aurora global database clusters this account may hold: the allowance Amazon
/// Aurora publishes for global databases (5). Reaching it is the modelled
/// `GlobalClusterQuotaExceededFault` — "The number of global database clusters for this
/// account is already at the maximum allowed" — which is worded per ACCOUNT, as is the
/// map this bounds: a global cluster is account-scoped and carries no region.
pub const GLOBAL_CLUSTER_QUOTA_PER_ACCOUNT: usize = 5;

/// How many DB clusters one region may hold: Amazon RDS's published default quota ("DB
/// clusters": 40 per Region). Reaching it is the modelled `DBClusterQuotaExceededFault` —
/// "The user attempted to create a new DB cluster and the user has already reached the
/// maximum allowed DB cluster quota".
pub const DB_CLUSTER_QUOTA_PER_REGION: usize = 40;

/// The status a DB cluster reports from the create that was accepted for it until the
/// clock makes its transition to [`DB_CLUSTER_STATUS_AVAILABLE`] due.
///
/// `CreateDBCluster` "creates a new Amazon Aurora DB cluster" and answers before the
/// cluster is usable, which is why the transition below is owed at all; the value is the
/// `creating` status RDS reports for a cluster it is still provisioning, as
/// `DBCluster$Status` ("the current state of this DB cluster") carries it.
pub const DB_CLUSTER_STATUS_CREATING: &str = "creating";

/// The status a DB cluster reports once it is usable, as `DBCluster$Status` carries it.
pub const DB_CLUSTER_STATUS_AVAILABLE: &str = "available";

/// The status an event notification subscription reports once it is set up and
/// delivering, from `EventSubscription$Status`'s published list ("creating | modifying |
/// deleting | active | no-permission | topic-not-exist").
///
/// It is the status a subscription has from its create onwards: RDS answers
/// `CreateEventSubscription` with the subscription already active — creating one writes a
/// notification rule and provisions no resource — so no transient status is owed for it
/// and the clock owes it no transition.
pub const EVENT_SUBSCRIPTION_STATUS_ACTIVE: &str = "active";

/// The status an Aurora global database cluster reports once it is usable, as
/// `GlobalCluster$Status` carries it. A global database created EMPTY provisions no DB
/// cluster — "You can create a global database that is initially empty, and then create
/// the primary and secondary DB clusters in the global database"
/// (`CreateGlobalCluster`) — so there is nothing to wait for and no transient status is
/// owed for it.
pub const GLOBAL_CLUSTER_STATUS_AVAILABLE: &str = "available";

/// The status a global database cluster reports in the answer to the delete that was
/// accepted for it, as `GlobalCluster$Status` carries it — the same `deleting` status the
/// other delete paths of this service report.
pub const GLOBAL_CLUSTER_STATUS_DELETING: &str = "deleting";

/// The status an event notification subscription reports in the answer to the delete that
/// was accepted for it, from `EventSubscription$Status`'s published list.
pub const EVENT_SUBSCRIPTION_STATUS_DELETING: &str = "deleting";

/// `GlobalCluster$StorageEncryptionType` for one encrypted under the key the service
/// applies itself: "`sse-rds` - The global database cluster is encrypted using an Amazon
/// Web Services owned KMS key."
pub const STORAGE_ENCRYPTION_TYPE_SSE_RDS: &str = "sse-rds";

/// `GlobalCluster$StorageEncryptionType` for one encrypted under a managed key:
/// "`sse-kms` - The global database cluster is encrypted using a customer managed KMS key
/// or Amazon Web Services managed KMS key."
pub const STORAGE_ENCRYPTION_TYPE_SSE_KMS: &str = "sse-kms";

/// `DBCluster$StorageEncryptionType` for a cluster whose data at rest is not encrypted at
/// all: "`none` - The DB cluster is not encrypted." Aurora storage is always encrypted, so
/// only a Multi-AZ DB cluster created without `StorageEncrypted` reports it.
pub const STORAGE_ENCRYPTION_TYPE_NONE: &str = "none";

/// The lifecycle types a global database cluster may be enrolled in, as
/// `CreateGlobalClusterMessage$EngineLifecycleSupport` publishes them ("Valid Values:
/// open-source-rds-extended-support | open-source-rds-extended-support-disabled").
pub const ENGINE_LIFECYCLE_SUPPORT_VALUES: [&str; 2] = [
    DEFAULT_ENGINE_LIFECYCLE_SUPPORT,
    "open-source-rds-extended-support-disabled",
];

/// Refuse an `EngineLifecycleSupport` outside the two values the member publishes,
/// naming the member, the value rejected and the set it is not in.
pub fn check_engine_lifecycle_support(value: &str) -> Result<(), String> {
    if ENGINE_LIFECYCLE_SUPPORT_VALUES.contains(&value) {
        return Ok(());
    }
    Err(format!(
        "invalid value {value} for EngineLifecycleSupport: must be one of {}",
        ENGINE_LIFECYCLE_SUPPORT_VALUES.join(", ")
    ))
}

/// The engines an Aurora global database cluster may run, as
/// `CreateGlobalClusterMessage$Engine` publishes them ("Valid Values: aurora-mysql |
/// aurora-postgresql"). A global database is an Aurora resource — "This operation applies
/// only to Aurora DB clusters" — so the list is narrower than the DB instance engines in
/// [`DB_INSTANCE_ENGINES`].
pub const GLOBAL_CLUSTER_ENGINES: [&str; 2] = ["aurora-mysql", "aurora-postgresql"];

/// The longest `EngineVersion` this service accepts as a version string at all. AWS's
/// longest published version strings are the SQL Server ones (`15.00.4382.1.v1`) and the
/// Aurora MySQL ones (`8.0.mysql_aurora.3.08.0`); 64 leaves room for any form AWS adds.
pub const MAX_ENGINE_VERSION_LEN: usize = 64;

/// Refuse a value that is not an engine version at all: AWS writes every published
/// engine version as a dotted number, optionally carrying a lettered qualifier
/// (`16.6`, `15.00.4382.1.v1`, `8.0.mysql_aurora.3.08.0`, `16.4-limitless`), so a
/// version starts with a digit and is drawn from that alphabet.
///
/// This is a check on the FORM, for a member whose value this service cannot resolve
/// against a catalogue of its own. It is deliberately not a claim that the version
/// exists: the engine-version catalogue this emulator publishes is explicitly not the
/// real service's live list, so a version it does not carry is not a version AWS refuses.
pub fn check_engine_version_form(value: &str) -> Result<(), String> {
    let length = value.chars().count();
    let shaped = value.starts_with(|c: char| c.is_ascii_digit())
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !shaped || length > MAX_ENGINE_VERSION_LEN {
        return Err(format!(
            "invalid value {value} for EngineVersion: an engine version starts with a digit \
             and is written as a dotted number, optionally with a qualifier (16.6, \
             8.0.mysql_aurora.3.08.0), of at most {MAX_ENGINE_VERSION_LEN} characters"
        ));
    }
    Ok(())
}

/// Refuse an `Engine` a global database cluster cannot run, naming the member, the value
/// rejected and the set it is not in.
pub fn check_global_cluster_engine(engine: &str) -> Result<(), String> {
    if GLOBAL_CLUSTER_ENGINES.contains(&engine) {
        return Ok(());
    }
    Err(format!(
        "invalid value {engine} for Engine: a global database cluster runs one of {}",
        GLOBAL_CLUSTER_ENGINES.join(", ")
    ))
}

/// The engines an option group may be associated with, as
/// `CreateOptionGroupMessage$EngineName` lists them — the identical list
/// `DescribeOptionGroupsMessage$EngineName` publishes for its filter
/// (`smithy_query(query="doc",shape="CreateOptionGroupMessage",member="EngineName")`:
/// "Valid Values: db2-ae db2-ce db2-se mariadb mysql oracle-ee oracle-ee-cdb
/// oracle-se2 oracle-se2-cdb postgres sqlserver-ee sqlserver-se sqlserver-ex
/// sqlserver-web"). A value AWS allocated, so the only honest source is the model's
/// own list; extend it when the model does.
pub const OPTION_GROUP_ENGINE_NAMES: [&str; 14] = [
    "db2-ae",
    "db2-ce",
    "db2-se",
    "mariadb",
    "mysql",
    "oracle-ee",
    "oracle-ee-cdb",
    "oracle-se2",
    "oracle-se2-cdb",
    "postgres",
    "sqlserver-ee",
    "sqlserver-se",
    "sqlserver-ex",
    "sqlserver-web",
];

/// Refuse an `EngineName` outside the value list its member publishes
/// ([`OPTION_GROUP_ENGINE_NAMES`]). Returns the message to refuse the request with,
/// naming the member and the constraint it broke.
pub fn check_option_group_engine_name(engine_name: &str) -> Result<(), String> {
    if OPTION_GROUP_ENGINE_NAMES.contains(&engine_name) {
        return Ok(());
    }
    Err(format!(
        "invalid value '{engine_name}' for EngineName: valid values are {}",
        OPTION_GROUP_ENGINE_NAMES.join(", ")
    ))
}

/// The engines a DB instance can run, as `CreateDBInstanceMessage$Engine` lists them
/// (`smithy_query(query="doc",shape="CreateDBInstanceMessage",member="Engine")`:
/// "Valid Values: aurora-mysql aurora-postgresql custom-oracle-ee custom-oracle-ee-cdb
/// custom-oracle-se2 custom-oracle-se2-cdb custom-sqlserver-ee custom-sqlserver-se
/// custom-sqlserver-web custom-sqlserver-dev db2-ae db2-ce db2-se mariadb mysql
/// oracle-ee oracle-ee-cdb oracle-se2 oracle-se2-cdb postgres sqlserver-dev-ee
/// sqlserver-ee sqlserver-se sqlserver-ex sqlserver-web"). These are values AWS
/// allocated, so the only honest source is the model's own list; extend it when the
/// model does.
pub const DB_INSTANCE_ENGINES: [&str; 25] = [
    "aurora-mysql",
    "aurora-postgresql",
    "custom-oracle-ee",
    "custom-oracle-ee-cdb",
    "custom-oracle-se2",
    "custom-oracle-se2-cdb",
    "custom-sqlserver-dev",
    "custom-sqlserver-ee",
    "custom-sqlserver-se",
    "custom-sqlserver-web",
    "db2-ae",
    "db2-ce",
    "db2-se",
    "mariadb",
    "mysql",
    "oracle-ee",
    "oracle-ee-cdb",
    "oracle-se2",
    "oracle-se2-cdb",
    "postgres",
    "sqlserver-dev-ee",
    "sqlserver-ee",
    "sqlserver-ex",
    "sqlserver-se",
    "sqlserver-web",
];

/// The engines the two sibling control planes this same front door serves publish for
/// their own `CreateDBInstance` (`src/service.rs` claims the Amazon DocumentDB and
/// Amazon Neptune Query surfaces too, and `DescribeOrderableDBInstanceOptions` reports
/// their classes): `com.amazonaws.docdb#CreateDBInstanceMessage$Engine` ("Valid value:
/// docdb") and Neptune's ("neptune"). A request for one of them is a request to THAT
/// service, so its engine is as published as the ones above.
pub const SIBLING_CONTROL_PLANE_ENGINES: [&str; 2] = ["docdb", "neptune"];

/// The engines a DB CLUSTER can run, as `CreateDBClusterMessage$Engine` lists them
/// (`smithy_query(query="doc",shape="CreateDBClusterMessage",member="Engine")`: "Valid
/// Values: aurora-mysql aurora-postgresql mysql postgres neptune"). It is a different,
/// much shorter list than [`DB_INSTANCE_ENGINES`] — only the engines that have a cluster
/// form — which is why `CreateDBCluster` resolves its `Engine` against this one.
///
/// `docdb` joins them for the same reason it joins [`SIBLING_CONTROL_PLANE_ENGINES`]: this
/// front door serves the Amazon DocumentDB control plane too, and
/// `com.amazonaws.docdb#CreateDBClusterMessage$Engine` publishes "Valid values: docdb".
pub const DB_CLUSTER_ENGINES: [&str; 6] = [
    "aurora-mysql",
    "aurora-postgresql",
    "docdb",
    "mysql",
    "neptune",
    "postgres",
];

/// Refuse an `Engine` outside the value list `CreateDBClusterMessage$Engine` publishes
/// ([`DB_CLUSTER_ENGINES`]).
pub fn check_db_cluster_engine(engine: &str) -> Result<(), String> {
    if DB_CLUSTER_ENGINES
        .iter()
        .any(|known| known.eq_ignore_ascii_case(engine))
    {
        return Ok(());
    }
    Err(format!(
        "Invalid DB engine: {engine} is not one of {}",
        DB_CLUSTER_ENGINES.join(", ")
    ))
}

/// Whether a cluster of `engine` is a MULTI-AZ DB CLUSTER rather than an Aurora-style one.
///
/// `CreateDBCluster` documents two cluster TYPES and gates most of its members on which
/// one is being created ("Valid for Cluster Type: Aurora DB clusters only", "This setting
/// is required to create a Multi-AZ DB cluster"). The type follows from the engine: the
/// Aurora engines — and the `docdb` and `neptune` engines of the two sibling control
/// planes this front door serves, whose clusters are Aurora-style too — are Aurora DB
/// clusters, while the RDS engines that have a cluster form (`mysql`, `postgres`) are
/// Multi-AZ DB clusters, "Multi-AZ deployments with two readable standby DB instances".
pub fn is_multi_az_db_cluster(engine: &str) -> bool {
    !is_aurora(engine)
        && !SIBLING_CONTROL_PLANE_ENGINES
            .iter()
            .any(|sibling| sibling.eq_ignore_ascii_case(engine))
}

/// The longest `DBClusterIdentifier` a cluster of `engine` may carry.
///
/// `DBClusterIdentifier`: "Must contain from 1 to 63 (for Aurora DB clusters) or 1 to 52
/// (for Multi-AZ DB clusters) letters, numbers, or hyphens."
pub fn db_cluster_identifier_max_len(engine: &str) -> usize {
    if is_multi_az_db_cluster(engine) {
        52
    } else {
        63
    }
}

/// The storage types a cluster of `engine` may be created with, as
/// `CreateDBClusterMessage$StorageType` publishes them ("Aurora DB clusters - aurora |
/// aurora-iopt1", "Multi-AZ DB clusters - io1 | io2 | gp3").
pub fn db_cluster_storage_types(engine: &str) -> &'static [&'static str] {
    if is_multi_az_db_cluster(engine) {
        &["io1", "io2", "gp3"]
    } else {
        &[AURORA_DEFAULT_STORAGE_TYPE, "aurora-iopt1"]
    }
}

/// The storage type an Aurora DB cluster gets when the request names none
/// (`StorageType` "Default: Aurora DB clusters - `aurora`").
///
/// It is also the one value the member documents as NOT reported back: "When you create an
/// Aurora DB cluster with the storage type set to `aurora-iopt1`, the storage type is
/// returned in the response. The storage type isn't returned when you set it to
/// `aurora`."
pub const AURORA_DEFAULT_STORAGE_TYPE: &str = "aurora";

/// The engine modes a DB cluster may be created in, as
/// `CreateDBClusterMessage$EngineMode` publishes them: "The DB engine mode of the DB
/// cluster, either `provisioned` or `serverless`."
pub const DB_CLUSTER_ENGINE_MODES: [&str; 2] = [DEFAULT_DB_CLUSTER_ENGINE_MODE, "serverless"];

/// The engine mode a DB cluster created without one runs in.
///
/// `EngineMode` names two values and says the `serverless` one "only applies for Aurora
/// Serverless v1 DB clusters. Aurora Serverless v2 DB clusters use the `provisioned`
/// engine mode" — so a cluster the request named no mode for is a provisioned one, which
/// is also the mode every version of this service's engine-version catalogue publishes.
pub const DEFAULT_DB_CLUSTER_ENGINE_MODE: &str = "provisioned";

/// The scalability mode an Aurora DB cluster created without one runs in
/// (`ClusterScalabilityType`: "When set to `standard` (the default), the cluster uses
/// normal DB instance creation").
pub const DEFAULT_CLUSTER_SCALABILITY_TYPE: &str = "standard";

/// `DBCluster$LocalWriteForwardingStatus` for a cluster that forwards writes from a reader
/// to its writer, and for one that does not — the two settled values of the published
/// `LocalWriteForwardingStatus` list ("enabled | disabled | enabling | disabling |
/// requested"), which is what `CreateDBCluster$EnableLocalWriteForwarding` selects between.
pub const LOCAL_WRITE_FORWARDING_ENABLED: &str = "enabled";

/// See [`LOCAL_WRITE_FORWARDING_ENABLED`].
pub const LOCAL_WRITE_FORWARDING_DISABLED: &str = "disabled";

/// The status an IAM role associated with a DB cluster reports once the association is in
/// place, as `DBClusterRole$Status` publishes it ("ACTIVE | PENDING | INVALID").
pub const DB_CLUSTER_ROLE_STATUS_ACTIVE: &str = "ACTIVE";

/// Refuse a `MasterUsername` outside the constraints its member documents: "Must be 1 to
/// 16 letters or numbers. First character must be a letter."
///
/// The third documented constraint — "Can't be a reserved word for the chosen database
/// engine" — is NOT enforced: the reserved words are each engine's own SQL keyword list,
/// which this service does not hold, and refusing a name no published list carries would
/// refuse names AWS accepts.
pub fn check_master_username(value: &str) -> Result<(), String> {
    if !value.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(format!(
            "MasterUsername must be 1 to 16 letters or numbers; got '{value}'"
        ));
    }
    let length = value.chars().count();
    if length == 0 || length > MAX_MASTER_USERNAME_LEN {
        return Err(format!(
            "MasterUsername must be 1 to {MAX_MASTER_USERNAME_LEN} letters or numbers; \
             got {length} characters"
        ));
    }
    if !value
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
    {
        return Err(format!(
            "the first character of MasterUsername must be a letter; got '{value}'"
        ));
    }
    Ok(())
}

/// The longest `MasterUsername` a DB cluster accepts ("Must be 1 to 16 letters or
/// numbers").
pub const MAX_MASTER_USERNAME_LEN: usize = 16;

/// The longest `RoleArn` an `AssociatedRoles` entry may carry: the bound the member's own
/// `@length` publishes (`DBClusterAssociatedRole$RoleArn`, 20..2048). The boundary
/// validator refuses a longer one before a handler runs; a handler that STORES the value
/// bounds it again where it enters the store, so the stored size is bounded locally rather
/// than only upstream.
pub const MAX_IAM_ROLE_ARN_LEN: usize = 2048;

/// Refuse a `MasterUserPassword` outside the constraints its member documents: "Must
/// contain from 8 to 41 characters. Can contain any printable ASCII character except \"/\",
/// \"\"\", or \"@\"."
///
/// The password itself is never stored or reported — the member is `@sensitive` and RDS
/// reports no member carrying it — so this is the only place it is looked at at all.
pub fn check_master_user_password(value: &str) -> Result<(), String> {
    let length = value.chars().count();
    if !MASTER_USER_PASSWORD_LEN.contains(&length) {
        return Err(format!(
            "MasterUserPassword must contain from {} to {} characters; got {length}",
            MASTER_USER_PASSWORD_LEN.start(),
            MASTER_USER_PASSWORD_LEN.end()
        ));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_graphic() && !matches!(c, '/' | '"' | '@'))
    {
        return Err(
            "MasterUserPassword can contain any printable ASCII character except '/', '\"' \
             or '@'"
                .to_string(),
        );
    }
    Ok(())
}

/// How long a `MasterUserPassword` may be ("Must contain from 8 to 41 characters").
pub const MASTER_USER_PASSWORD_LEN: std::ops::RangeInclusive<usize> = 8..=41;

/// The `BackupRetentionPeriod` a DB cluster accepts ("Constraints: Must be a value from 1
/// to 35") — a different lower bound than a DB instance's
/// [`BACKUP_RETENTION_RANGE`], whose own member documents "0 to 35".
pub const CLUSTER_BACKUP_RETENTION_RANGE: std::ops::RangeInclusive<i32> = 1..=35;

/// The `BacktrackWindow` an Aurora MySQL DB cluster accepts, in seconds ("If specified,
/// this value must be set to a number from 0 to 259,200 (72 hours)"). Zero disables
/// backtracking, which is also the documented default.
pub const BACKTRACK_WINDOW_RANGE: std::ops::RangeInclusive<i64> = 0..=259_200;

/// How long an Aurora Serverless v2 DB instance may be idle before Aurora pauses it
/// (`ServerlessV2ScalingConfiguration$SecondsUntilAutoPause`: "Specify a value between 300
/// seconds (five minutes) and 86,400 seconds (one day)").
pub const SECONDS_UNTIL_AUTO_PAUSE_RANGE: std::ops::RangeInclusive<i32> = 300..=86_400;

/// The `PerformanceInsightsRetentionPeriod` the `advanced` Database Insights mode requires
/// (`DatabaseInsightsMode`: "If you set this value to advanced, you must also set the
/// PerformanceInsightsEnabled parameter to true and the
/// PerformanceInsightsRetentionPeriod parameter to 465").
pub const ADVANCED_INSIGHTS_RETENTION_PERIOD: i32 = 465;

/// The `DatabaseInsightsMode` that requires Performance Insights and the 465-day retention
/// period above.
pub const DATABASE_INSIGHTS_MODE_ADVANCED: &str = "advanced";

/// Refuse an `Engine` outside the value list its member publishes
/// ([`DB_INSTANCE_ENGINES`], plus the sibling control planes'
/// [`SIBLING_CONTROL_PLANE_ENGINES`]). A value that is not an engine names no database
/// engine at all, which is what the real service refuses with `InvalidParameterValue`.
pub fn check_engine(engine: &str) -> Result<(), String> {
    if DB_INSTANCE_ENGINES
        .iter()
        .chain(SIBLING_CONTROL_PLANE_ENGINES.iter())
        .any(|known| known.eq_ignore_ascii_case(engine))
    {
        return Ok(());
    }
    Err("Invalid DB engine".to_string())
}

/// Refuse a `MajorEngineVersion` the engine-version catalogue does not carry for
/// `engine_name`.
///
/// The major version is resolved against the SAME catalogue `DescribeDBEngineVersions`
/// reports, so the two cannot disagree: a major version no entry of that engine carries
/// is one this service cannot associate an option group with, and the group must not be
/// stored under it. The message is the real service's own wording for this refusal,
/// built from the request's engine name and major version.
pub fn check_major_engine_version(engine_name: &str, major: &str) -> Result<(), String> {
    if crate::ops::describe_db_engine_versions::offers_major_engine_version(engine_name, major) {
        return Ok(());
    }
    Err(format!(
        "Cannot find major version {major} for {engine_name}"
    ))
}

/// Refuse a `DBInstanceClass` that is not of the published RDS DB instance class form.
///
/// A DB instance class is written `db.<family>.<size>` — `db.m5.large`,
/// `db.t4g.micro`, `db.r5b.4xlarge.tpc2.mem8x` — plus the one exception
/// [`SERVERLESS_INSTANCE_CLASS`], the Aurora Serverless v2 capacity class. The SIZE
/// token is drawn from the published instance-size vocabulary ([`INSTANCE_SIZES`], or
/// `<n>xlarge`), so a value whose size token is not one of those names no class at all
/// and is what the real service refuses with `InvalidParameterValue`.
///
/// The FORM is the rule rather than a list of classes: which classes exist is AWS's
/// per-region, per-engine matrix, which moves with every instance-type launch and which
/// this service does not hold — `DescribeOrderableDBInstanceOptions` reports an
/// explicitly partial subset of it, so refusing everything outside that subset would
/// refuse classes AWS offers (`db.m6i.large`, `db.m5.2xlarge`).
pub fn check_db_instance_class(db_instance_class: &str) -> Result<(), String> {
    let malformed = || format!("Invalid DB Instance class: {db_instance_class}");
    if db_instance_class == SERVERLESS_INSTANCE_CLASS {
        return Ok(());
    }
    let Some(rest) = db_instance_class.strip_prefix("db.") else {
        return Err(malformed());
    };
    let parts: Vec<&str> = rest.split('.').collect();
    let [family, size, ..] = parts.as_slice() else {
        return Err(malformed());
    };
    let family_ok = !family.is_empty()
        && family
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    if !family_ok || !is_instance_size(size) {
        return Err(malformed());
    }
    Ok(())
}

/// The Aurora Serverless v2 capacity class, the one DB instance class that is not
/// `db.<family>.<size>` (`DescribeOrderableDBInstanceOptions` offers it for the Aurora
/// engines).
pub const SERVERLESS_INSTANCE_CLASS: &str = "db.serverless";

/// The named instance sizes a DB instance class's size token may be, as AWS publishes
/// them for its instance types. The remaining sizes are written `<n>xlarge`, which
/// [`is_instance_size`] accepts on the form rather than by listing every `n`.
pub const INSTANCE_SIZES: [&str; 6] = ["nano", "micro", "small", "medium", "large", "xlarge"];

/// Whether `size` is a published instance-size token: one of [`INSTANCE_SIZES`] or
/// `<n>xlarge` for a one- to three-digit `n` that is not zero-led (`2xlarge` …
/// `112xlarge` — there is no `0xlarge` and no `04xlarge`).
fn is_instance_size(size: &str) -> bool {
    if INSTANCE_SIZES.contains(&size) {
        return true;
    }
    match size.strip_suffix("xlarge") {
        Some(count) => {
            (1..=3).contains(&count.len())
                && count.bytes().all(|b| b.is_ascii_digit())
                && !count.starts_with('0')
        }
        None => false,
    }
}

/// How many distinct regions this store keeps records for.
///
/// The per-region quotas alone would not bound the store: the region keying them is
/// the request's own SigV4 credential-scope region, which this emulator does not
/// authenticate, so without this cap a caller could open a fresh quota bucket per
/// request. With it the store holds at most `MAX_REGIONS` buckets, and so at most
/// `MAX_REGIONS * DB_INSTANCE_QUOTA_PER_REGION` DB instances and
/// `MAX_REGIONS * DB_SNAPSHOT_QUOTA_PER_REGION` DB snapshots in total. AWS operates
/// on the order of 36 commercial regions, so this bound is this emulator's and is
/// comfortably above the real surface; reaching it answers the same modelled quota
/// fault, which is what AWS answers when a create would exceed an allowance.
pub const MAX_REGIONS: usize = 64;

/// How many filter VALUES one describe request may carry in total, across all its
/// filters.
///
/// RDS documents no bound on `Filters`, and the scan runs while this service's one
/// state lock is held, so the work must be capped on both factors: the record count
/// is capped by the quotas above and the term count by this. A request past it is
/// refused with a 400 rather than served slowly; the bound is this emulator's, and
/// generous next to the five filter names either describe operation recognises.
pub const MAX_FILTER_TERMS: usize = 256;

/// A record's key: the region the request that created it was signed for, and the
/// resource's own identifier.
pub type RegionalKey = (String, String);

/// Per-service state for com-amazonaws-rds-amazonrdsv19. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsRdsAmazonrdsv19State {
    /// DB instances by `(region, DBInstanceIdentifier)` — the identifier always stored
    /// lowercased, as the model documents. Ordered, so `DescribeDBInstances` pages
    /// deterministically within a region.
    pub db_instances: BTreeMap<RegionalKey, DbInstanceRecord>,
    /// DB snapshots by `(region, DBSnapshotIdentifier)`. Ordered, for the same reason.
    pub db_snapshots: BTreeMap<RegionalKey, DbSnapshotRecord>,
    /// DB clusters by `(region, DBClusterIdentifier)` — the identifier always stored
    /// lowercased, as the model documents. Ordered, so `DescribeDBClusters` pages
    /// deterministically within a region.
    ///
    /// `CreateDBCluster` inserts here — bounded by [`DB_CLUSTER_QUOTA_PER_REGION`] per
    /// region and by [`MAX_REGIONS`] regions — and this is also what a request that
    /// REFERENCES a cluster (`CreateDBInstance$DBClusterIdentifier`,
    /// `CreateGlobalCluster$SourceDBClusterIdentifier`) is resolved against, so a
    /// reference can only name a cluster some create really stored.
    pub db_clusters: BTreeMap<RegionalKey, DbClusterRecord>,
    /// Cluster parameter groups by `(region, DBClusterParameterGroupName)` — the name
    /// always stored lowercased, as the model documents. Ordered, so
    /// `DescribeDBClusterParameterGroups` pages deterministically within a region.
    pub db_cluster_parameter_groups: BTreeMap<RegionalKey, DbClusterParameterGroupRecord>,
    /// Event notification subscriptions by `(region, SubscriptionName)`. Ordered, so
    /// `DescribeEventSubscriptions` pages deterministically within a region.
    ///
    /// `CreateEventSubscription` inserts here — bounded by
    /// [`EVENT_SUBSCRIPTION_QUOTA_PER_REGION`] per region, and by [`MAX_REGIONS`] regions
    /// — and `DescribeEventSubscriptions` reports what is here.
    pub event_subscriptions: BTreeMap<RegionalKey, EventSubscriptionRecord>,
    /// Aurora global database clusters by `GlobalClusterIdentifier`.
    ///
    /// Keyed by the identifier ALONE, not by `(region, identifier)` like the maps above: a
    /// global cluster is an ACCOUNT-scoped resource whose ARN carries no region
    /// (`arn:aws:rds::<account>:global-cluster:<id>`) and spans the regions of its member
    /// clusters, so `DescribeGlobalClusters` reports it whatever region the request is
    /// signed for. `CreateGlobalCluster` inserts here, bounded by
    /// [`GLOBAL_CLUSTER_QUOTA_PER_ACCOUNT`] — the account scope the map itself has.
    pub global_clusters: BTreeMap<String, GlobalClusterRecord>,
    /// Option groups by `(region, OptionGroupName)`. Ordered, so
    /// `DescribeOptionGroups` pages deterministically within a region.
    pub option_groups: BTreeMap<RegionalKey, OptionGroupRecord>,
    /// DB subnet groups by `(region, DBSubnetGroupName)` — the name always stored
    /// lowercased, as `CreateDBSubnetGroup` documents. These are what a request that
    /// NAMES a DB subnet group (`CreateDBInstance$DBSubnetGroupName`) is resolved
    /// against, so a name no group was created under is a not-found rather than a
    /// silently accepted reference.
    pub db_subnet_groups: BTreeMap<RegionalKey, DbSubnetGroupRecord>,
    /// The account's CA certificate OVERRIDE, by region: the `CertificateIdentifier`
    /// `ModifyCertificates` pinned as the certificate new DB instances are created with,
    /// for the regions where one is set.
    ///
    /// Keyed by region because that is the scope `ModifyCertificates` documents ("RDS has
    /// already moved to a new default CA certificate for the specified Amazon Web Services
    /// Region"), so an override set in one region leaves another region's default alone.
    /// An absent entry is the published system default ([`DEFAULT_CA_CERTIFICATE_IDENTIFIER`]),
    /// which is why this is an override map rather than a per-region copy of the default:
    /// at most ONE entry per region, and a region that holds no record at all cannot get
    /// one without passing [`ComAmazonawsRdsAmazonrdsv19State::has_room_for_region`], so
    /// the map is bounded by [`MAX_REGIONS`] like every other store here.
    pub certificate_overrides: BTreeMap<String, String>,
    /// Monotonic counter behind every server-assigned id this service mints, so
    /// resource ids and endpoint hosts are unique and reproducible.
    pub mint_counter: u64,
}

impl ComAmazonawsRdsAmazonrdsv19State {
    /// The next value of the server-side id counter.
    pub fn next_mint(&mut self) -> u64 {
        self.mint_counter += 1;
        self.mint_counter
    }

    /// The `CertificateIdentifier` the account's override names in `region`, or `None`
    /// where no `ModifyCertificates` call set one.
    pub fn certificate_override(&self, region: &str) -> Option<&str> {
        self.certificate_overrides.get(region).map(String::as_str)
    }

    /// The CA certificate a DB instance created in `region` without a
    /// `CACertificateIdentifier` is issued under, which is also what
    /// `DescribeCertificates` reports as `DefaultCertificateForNewLaunches`: "either the
    /// root CA override set on your Amazon Web Services account or the system default CA
    /// for the Region if no override exists".
    pub fn default_ca_certificate(&self, region: &str) -> &str {
        self.certificate_override(region)
            .unwrap_or(DEFAULT_CA_CERTIFICATE_IDENTIFIER)
    }

    /// Materialise the status transitions the clock has already made due IN `region`.
    ///
    /// Call it at the top of every handler, reporting ones included: this writes only
    /// changes that were owed at this instant even if nobody had called, so a read
    /// cannot move a status that was not already due. It walks one region's ordered
    /// range — bounded by that region's quota — never the whole store, so the work one
    /// request does under the state lock does not grow with what other callers stored.
    pub fn settle_due_transitions(&mut self, region: &str) {
        let now = Instant::now();
        // A record whose pending change is its REMOVAL leaves the store here: the
        // keys are collected first because the range borrows the map mutably.
        let mut gone: Vec<RegionalKey> = Vec::new();
        for (key, instance) in self
            .db_instances
            .range_mut((region.to_string(), String::new())..)
        {
            if key.0 != region {
                break;
            }
            if instance.settle(now) {
                gone.push(key.clone());
            }
        }
        for key in gone.drain(..) {
            self.db_instances.remove(&key);
        }
        for (key, snapshot) in self
            .db_snapshots
            .range_mut((region.to_string(), String::new())..)
        {
            if key.0 != region {
                break;
            }
            if snapshot.settle(now) {
                gone.push(key.clone());
            }
        }
        for key in gone.drain(..) {
            self.db_snapshots.remove(&key);
        }
        for (key, cluster) in self
            .db_clusters
            .range_mut((region.to_string(), String::new())..)
        {
            if key.0 != region {
                break;
            }
            if cluster.settle(now) {
                gone.push(key.clone());
            }
        }
        for key in gone {
            self.db_clusters.remove(&key);
        }
    }

    /// The DB clusters stored in `region`, in identifier order.
    pub fn clusters_in(&self, region: &str) -> Vec<&DbClusterRecord> {
        range_in(&self.db_clusters, region)
    }

    /// The DB cluster `named` resolves to in `region`, or `None` when nothing of that name
    /// exists — what a request that NAMES a DB cluster is resolved against.
    ///
    /// `named` is "the user-supplied DB cluster identifier or the Amazon Resource Name
    /// (ARN) of the DB cluster", and "this parameter isn't case-sensitive": both forms are
    /// folded to the identifier the store is keyed by before the lookup, so the two name
    /// one and the same cluster.
    pub fn resolve_db_cluster(&self, region: &str, named: &str) -> Option<&DbClusterRecord> {
        let identifier = db_cluster_identifier_of(named).to_ascii_lowercase();
        self.db_clusters.get(&(region.to_string(), identifier))
    }

    /// What a DB cluster reports about the records AROUND it — see [`ClusterMembership`].
    fn cluster_membership(&self, record: &DbClusterRecord) -> ClusterMembership {
        // The member DB instances, earliest-created first: the cluster's writer is the
        // instance that was created in it first, and every later one is a reader.
        let mut members: Vec<&DbInstanceRecord> = self
            .instances_in(&record.region)
            .into_iter()
            .filter(|instance| {
                instance
                    .db_cluster_identifier
                    .as_ref()
                    .is_some_and(|named| named.eq_ignore_ascii_case(&record.identifier))
            })
            .collect();
        members.sort_by(|left, right| {
            left.instance_create_time
                .as_secs_f64()
                .total_cmp(&right.instance_create_time.as_secs_f64())
                .then_with(|| left.identifier.cmp(&right.identifier))
        });
        ClusterMembership {
            // "Indicates whether the DB cluster has instances in multiple Availability
            // Zones" — read off the zones its members are actually in, so it cannot
            // disagree with them.
            multi_az: members
                .iter()
                .map(|instance| instance.availability_zone.as_str())
                .collect::<BTreeSet<&str>>()
                .len()
                > 1,
            members: members
                .iter()
                .enumerate()
                .map(|(position, instance)| DBClusterMember {
                    db_instance_identifier: Some(instance.identifier.clone()),
                    is_cluster_writer: Some(position == 0),
                    db_cluster_parameter_group_status: Some(MEMBERSHIP_IN_SYNC.to_string()),
                    // `PromotionTier` is a property of the MEMBER instance that no
                    // implemented operation stores, so it is left out rather than invented.
                    ..DBClusterMember::default()
                })
                .collect(),
            // "Contains one or more identifiers of the read replicas associated with this
            // DB cluster": the clusters in this region whose replication source is this
            // one, read from those records rather than copied onto this one.
            read_replica_identifiers: self
                .clusters_in(&record.region)
                .into_iter()
                .filter(|other| {
                    other
                        .replication_source_arn
                        .as_ref()
                        .is_some_and(|source| *source == record.arn())
                })
                .map(|other| other.identifier.clone())
                .collect(),
        }
    }

    /// Project a stored DB cluster onto the modelled `DBCluster` output shape, with the
    /// members and read replicas read from the records that carry them.
    pub fn cluster_to_wire(&self, record: &DbClusterRecord) -> DBCluster {
        record.to_wire(&self.cluster_membership(record))
    }

    /// The DB instances stored in `region`, in identifier order.
    ///
    /// An ordered range rather than a scan of the whole map, so the work is bounded by
    /// [`DB_INSTANCE_QUOTA_PER_REGION`] however many regions the store holds.
    pub fn instances_in(&self, region: &str) -> Vec<&DbInstanceRecord> {
        range_in(&self.db_instances, region)
    }

    /// The DB snapshots stored in `region`, in identifier order.
    pub fn snapshots_in(&self, region: &str) -> Vec<&DbSnapshotRecord> {
        range_in(&self.db_snapshots, region)
    }

    /// The cluster parameter groups stored in `region`, in name order.
    pub fn cluster_parameter_groups_in(&self, region: &str) -> Vec<&DbClusterParameterGroupRecord> {
        range_in(&self.db_cluster_parameter_groups, region)
    }

    /// The event notification subscriptions stored in `region`, in subscription-name
    /// order.
    pub fn event_subscriptions_in(&self, region: &str) -> Vec<&EventSubscriptionRecord> {
        range_in(&self.event_subscriptions, region)
    }

    /// The option groups stored in `region`, in name order.
    pub fn option_groups_in(&self, region: &str) -> Vec<&OptionGroupRecord> {
        range_in(&self.option_groups, region)
    }

    /// The DB subnet groups stored in `region`, in name order.
    pub fn db_subnet_groups_in(&self, region: &str) -> Vec<&DbSubnetGroupRecord> {
        range_in(&self.db_subnet_groups, region)
    }

    /// The DB subnet group `name` resolves to in `region`, or `None` when nothing of
    /// that name exists — what a request that NAMES a DB subnet group is resolved
    /// against.
    ///
    /// Two things resolve: a group `CreateDBSubnetGroup` stored, and the `default`
    /// group the real service provides in every region alongside the account's default
    /// VPC. The default one is not stored because nobody creates it —
    /// `CreateDBSubnetGroup` refuses that name ("Must not be default") for exactly that
    /// reason — and `CreateDBInstance$PubliclyAccessible` documents a default that
    /// branches on it, so a request naming it is resolving a group that exists.
    pub fn resolve_db_subnet_group(
        &self,
        region: &str,
        name: &str,
    ) -> Option<DbSubnetGroupRecord> {
        if let Some(stored) = self
            .db_subnet_groups
            .get(&(region.to_string(), name.to_string()))
        {
            return Some(stored.clone());
        }
        (name == DEFAULT_DB_SUBNET_GROUP)
            .then(|| DbSubnetGroupRecord::account_default(region))
    }

    /// Every DB subnet group a request signed for `region` can see, in name order: the
    /// groups stored there plus the account's `default` group.
    pub fn visible_db_subnet_groups(&self, region: &str) -> Vec<DbSubnetGroupRecord> {
        let mut groups: Vec<DbSubnetGroupRecord> = self
            .db_subnet_groups_in(region)
            .into_iter()
            .cloned()
            .collect();
        groups.push(DbSubnetGroupRecord::account_default(region));
        groups.sort_by(|left, right| left.name.cmp(&right.name));
        groups
    }

    /// Whether a record may be stored for `region`: either it already holds records, or
    /// the store has room for one more region ([`MAX_REGIONS`]).
    ///
    /// The count walks the keys of every map that opens a region bucket, which the
    /// bounds above already cap in total (and the override map holds at most one entry
    /// per region), so the work is bounded by that total rather than by anything a
    /// caller chooses. Every operation that would create a bucket for a region holding
    /// no record yet gates on this, so a caller cannot open an unbounded number of them
    /// by naming a fresh region on each request.
    pub fn has_room_for_region(&self, region: &str) -> bool {
        let known: BTreeSet<&str> = self
            .db_instances
            .keys()
            .chain(self.db_snapshots.keys())
            .chain(self.db_clusters.keys())
            .chain(self.db_cluster_parameter_groups.keys())
            .chain(self.event_subscriptions.keys())
            .chain(self.option_groups.keys())
            .chain(self.db_subnet_groups.keys())
            .map(|(stored, _)| stored.as_str())
            .chain(self.certificate_overrides.keys().map(String::as_str))
            .collect();
        known.contains(region) || known.len() < MAX_REGIONS
    }
}

/// The values of `map` whose key names `region`, in key order.
fn range_in<'a, T>(map: &'a BTreeMap<RegionalKey, T>, region: &str) -> Vec<&'a T> {
    map.range((region.to_string(), String::new())..)
        .take_while(|((stored, _), _)| stored == region)
        .map(|(_, record)| record)
        .collect()
}

/// What the clock owes a record once its due instant passes.
#[derive(Debug, Clone)]
pub enum PendingChange {
    /// The resource reaches this status (`creating` -> `available`, say).
    Status(String),
    /// The resource is gone from the service: the end of a delete that was already
    /// accepted, which "can't be canceled or reverted after it begins".
    Removal,
}

/// A change this service owes: what becomes of the resource, and when that is due.
#[derive(Debug, Clone)]
pub struct PendingStatus {
    pub change: PendingChange,
    pub due_at: Instant,
}

impl PendingStatus {
    /// A transition to `status`, due [`STATUS_TRANSITION_DELAY`] from now.
    pub fn in_a_moment(status: &str) -> Self {
        Self {
            change: PendingChange::Status(status.to_string()),
            due_at: Instant::now() + STATUS_TRANSITION_DELAY,
        }
    }

    /// The record's removal from the store, due [`STATUS_TRANSITION_DELAY`] from now.
    pub fn removal_in_a_moment() -> Self {
        Self {
            change: PendingChange::Removal,
            due_at: Instant::now() + STATUS_TRANSITION_DELAY,
        }
    }
}

/// A stored DB instance: everything the service persists about it.
///
/// The ARN, the endpoint DNS name and the hosted zone id are NOT stored as text:
/// they are functions of the stored region, identifier and minted endpoint host, and
/// [`DbInstanceRecord::to_wire`] computes them. That is what keeps them consistent
/// after `ModifyDBInstance` renames an instance — a stored ARN would still name the
/// old identifier.
#[derive(Debug, Clone)]
pub struct DbInstanceRecord {
    pub identifier: String,
    /// The region the instance was created in; the scope of its ARN and endpoint.
    pub region: String,
    pub dbi_resource_id: String,
    /// The server-assigned middle label of the endpoint DNS name, minted once.
    pub endpoint_host: String,
    pub db_instance_class: String,
    pub engine: String,
    pub engine_version: Option<String>,
    pub status: String,
    pub pending_status: Option<PendingStatus>,
    pub db_name: Option<String>,
    pub allocated_storage: Option<i32>,
    pub max_allocated_storage: Option<i32>,
    pub master_username: Option<String>,
    pub endpoint_port: i32,
    pub multi_az: bool,
    pub storage_type: String,
    pub storage_encrypted: bool,
    pub kms_key_id: Option<String>,
    pub iops: Option<i32>,
    pub license_model: Option<String>,
    pub availability_zone: String,
    pub backup_retention_period: i32,
    pub preferred_backup_window: Option<String>,
    pub preferred_maintenance_window: Option<String>,
    pub publicly_accessible: bool,
    pub auto_minor_version_upgrade: bool,
    pub copy_tags_to_snapshot: bool,
    pub deletion_protection: bool,
    pub db_subnet_group_name: Option<String>,
    /// The DB parameter group applied to the instance: the one the request named, or
    /// the default group of the engine version it was created with.
    pub db_parameter_group_name: Option<String>,
    /// The option group applied to the instance: the one the request named, or the
    /// default group of its engine and major engine version. It is a REFERENCE, which
    /// is what keeps a group an instance uses from being deleted under it
    /// (`DeleteOptionGroup`).
    pub option_group_name: Option<String>,
    pub vpc_security_group_ids: Vec<String>,
    /// The legacy EC2-Classic DB security groups the request named, which is normally
    /// none: "This setting applies to the legacy EC2-Classic platform, which is no
    /// longer used to create new DB instances."
    pub db_security_group_names: Vec<String>,
    pub db_cluster_identifier: Option<String>,
    pub domain: Option<String>,
    /// The CA the instance's server certificate is issued under.
    pub ca_certificate_identifier: String,
    pub monitoring_interval: i32,
    pub performance_insights_enabled: bool,
    pub iam_database_authentication_enabled: bool,
    pub database_insights_mode: String,
    pub dedicated_log_volume: bool,
    pub customer_owned_ip_enabled: bool,
    pub network_type: String,
    pub backup_target: String,
    pub engine_lifecycle_support: String,
    /// The storage throughput provisioned for the instance, in MiBps.
    pub storage_throughput: i32,
    pub instance_create_time: ::aws_smithy_types::DateTime,
    pub tags: Vec<Tag>,
}

impl DbInstanceRecord {
    /// Apply a change whose due instant has passed. Returns whether the record is now
    /// gone — an accepted delete that has run its course — so the caller drops it from
    /// the store.
    fn settle(&mut self, now: Instant) -> bool {
        if self.pending_status.as_ref().is_some_and(|p| now >= p.due_at) {
            let pending = self.pending_status.take().expect("checked just above");
            match pending.change {
                PendingChange::Status(status) => self.status = status,
                PendingChange::Removal => return true,
            }
        }
        false
    }

    /// Whether a delete has already been accepted for this instance, so it is on its
    /// way out and nothing further can be done to it.
    pub fn is_deleting(&self) -> bool {
        self.status == DB_INSTANCE_STATUS_DELETING
    }

    /// Accept a delete: the instance reports `deleting` from now on, and the clock
    /// owes its removal from the store [`STATUS_TRANSITION_DELAY`] from now, after
    /// which every read of the identifier is a `DBInstanceNotFoundFault`. Nothing
    /// sleeps: the due instant is stored and materialised by
    /// [`ComAmazonawsRdsAmazonrdsv19State::settle_due_transitions`].
    pub fn begin_deletion(&mut self) {
        self.status = DB_INSTANCE_STATUS_DELETING.to_string();
        self.pending_status = Some(PendingStatus::removal_in_a_moment());
    }

    /// `arn:aws:rds:<region>:<account>:db:<identifier>`, from the stored region and
    /// the instance's current identifier.
    pub fn arn(&self) -> String {
        db_instance_arn(&self.region, &self.identifier)
    }

    /// The DNS name this instance's endpoint resolves to:
    /// `<identifier>.<minted host>.<region>.rds.amazonaws.com`.
    pub fn endpoint_address(&self) -> String {
        format!(
            "{}.{}.{}.rds.amazonaws.com",
            self.identifier, self.endpoint_host, self.region
        )
    }

    /// When the server certificate issued at creation expires:
    /// [`SERVER_CERTIFICATE_VALIDITY`] after the instance was created. A function of
    /// the stored creation time rather than a stored instant, so it cannot drift from
    /// it.
    pub fn certificate_valid_till(&self) -> ::aws_smithy_types::DateTime {
        ::aws_smithy_types::DateTime::from_secs_f64(
            self.instance_create_time.as_secs_f64() + SERVER_CERTIFICATE_VALIDITY.as_secs_f64(),
        )
    }

    /// `DbInstancePort`: "The port that the DB instance listens on. If the DB instance
    /// is part of a DB cluster, this can be a different port than the DB cluster port."
    ///
    /// The member reports the port of a CLUSTER MEMBER, which is why it is separate
    /// from `Endpoint.Port` (the port a client connects to, which this service reports
    /// from the same stored port): an instance that belongs to no DB cluster has no
    /// such port, and RDS reports zero for it.
    pub fn db_instance_port(&self) -> i32 {
        match self.db_cluster_identifier {
            Some(_) => self.endpoint_port,
            None => 0,
        }
    }

    /// Project the stored record onto the modelled `DBInstance` output shape.
    ///
    /// Every member RDS reports for a DB instance it holds is projected here, from the
    /// stored record or from the documented default the record was created with; the
    /// nested `Endpoint`, `DBSubnetGroup`, `CertificateDetails`, `PendingModifiedValues`
    /// and membership lists are projected from the same record, so no two of them can
    /// disagree. A member whose value this service has no state for at all is left out
    /// rather than invented — a list it does have state for is reported EMPTY, which is
    /// what RDS reports for an instance that has none of those associations.
    pub fn to_wire(&self) -> DBInstance {
        DBInstance {
            db_instance_identifier: Some(self.identifier.clone()),
            db_instance_class: Some(self.db_instance_class.clone()),
            engine: Some(self.engine.clone()),
            db_instance_status: Some(self.status.clone()),
            master_username: self.master_username.clone(),
            db_name: self.db_name.clone(),
            endpoint: Some(Endpoint {
                address: Some(self.endpoint_address()),
                port: Some(self.endpoint_port),
                hosted_zone_id: hosted_zone_id(&self.region).map(str::to_string),
            }),
            allocated_storage: self.allocated_storage,
            instance_create_time: Some(self.instance_create_time),
            preferred_backup_window: self.preferred_backup_window.clone(),
            backup_retention_period: Some(self.backup_retention_period),
            // The legacy EC2-Classic memberships: empty for every instance created
            // through the VPC platform, which is every instance this service holds
            // unless a request named one.
            db_security_groups: Some(
                self.db_security_group_names
                    .iter()
                    .map(|name| DBSecurityGroupMembership {
                        db_security_group_name: Some(name.clone()),
                        status: Some(MEMBERSHIP_ACTIVE.to_string()),
                    })
                    .collect(),
            ),
            vpc_security_groups: Some(
                self.vpc_security_group_ids
                    .iter()
                    .map(|id| VpcSecurityGroupMembership {
                        vpc_security_group_id: Some(id.clone()),
                        status: Some(MEMBERSHIP_ACTIVE.to_string()),
                    })
                    .collect(),
            ),
            db_parameter_groups: Some(
                self.db_parameter_group_name
                    .iter()
                    .map(|name| DBParameterGroupStatus {
                        db_parameter_group_name: Some(name.clone()),
                        parameter_apply_status: Some(MEMBERSHIP_IN_SYNC.to_string()),
                    })
                    .collect(),
            ),
            option_group_memberships: Some(
                self.option_group_name
                    .iter()
                    .map(|name| OptionGroupMembership {
                        option_group_name: Some(name.clone()),
                        status: Some(MEMBERSHIP_IN_SYNC.to_string()),
                    })
                    .collect(),
            ),
            availability_zone: Some(self.availability_zone.clone()),
            // Only the name and the status of the subnet group: its VPC, its subnets
            // and its description are properties of a DB subnet group record, which
            // only `CreateDBSubnetGroup` mints — still a stub, and its VPC and subnets
            // are EC2 resources this emulator's EC2 service does not mint either. A
            // made-up VPC or subnet id would name a network nothing can be reached on.
            db_subnet_group: self.db_subnet_group_name.clone().map(|name| DBSubnetGroup {
                db_subnet_group_name: Some(name),
                subnet_group_status: Some("Complete".to_string()),
                ..DBSubnetGroup::default()
            }),
            preferred_maintenance_window: self.preferred_maintenance_window.clone(),
            // Every change `ModifyDBInstance` accepts is applied to the stored record
            // at once, so no change is ever pending: the element is reported EMPTY,
            // which says exactly that, rather than omitted.
            pending_modified_values: Some(PendingModifiedValues::default()),
            upgrade_rollout_order: Some(DEFAULT_UPGRADE_ROLLOUT_ORDER.to_string()),
            multi_az: Some(self.multi_az),
            engine_version: self.engine_version.clone(),
            auto_minor_version_upgrade: Some(self.auto_minor_version_upgrade),
            // No implemented operation creates a read replica of an instance
            // (`CreateDBInstanceReadReplica` is still a stub), so no instance this
            // service holds has one: the list is EMPTY, not absent.
            read_replica_db_instance_identifiers: Some(Vec::new()),
            license_model: self.license_model.clone(),
            iops: self.iops,
            storage_throughput: Some(self.storage_throughput),
            publicly_accessible: Some(self.publicly_accessible),
            storage_type: Some(self.storage_type.clone()),
            db_instance_port: Some(self.db_instance_port()),
            db_cluster_identifier: self.db_cluster_identifier.clone(),
            storage_encrypted: Some(self.storage_encrypted),
            kms_key_id: self.kms_key_id.clone(),
            dbi_resource_id: Some(self.dbi_resource_id.clone()),
            ca_certificate_identifier: Some(self.ca_certificate_identifier.clone()),
            certificate_details: Some(CertificateDetails {
                ca_identifier: Some(self.ca_certificate_identifier.clone()),
                valid_till: Some(self.certificate_valid_till()),
            }),
            domain_memberships: Some(
                self.domain
                    .iter()
                    .map(|domain| DomainMembership {
                        domain: Some(domain.clone()),
                        status: Some("joined".to_string()),
                        ..DomainMembership::default()
                    })
                    .collect(),
            ),
            copy_tags_to_snapshot: Some(self.copy_tags_to_snapshot),
            monitoring_interval: Some(self.monitoring_interval),
            db_instance_arn: Some(self.arn()),
            iam_database_authentication_enabled: Some(self.iam_database_authentication_enabled),
            database_insights_mode: Some(self.database_insights_mode.clone()),
            performance_insights_enabled: Some(self.performance_insights_enabled),
            deletion_protection: Some(self.deletion_protection),
            // No implemented operation associates an IAM role with an instance
            // (`AddRoleToDBInstance` is still a stub): EMPTY for the same reason as the
            // read-replica list above.
            associated_roles: Some(Vec::new()),
            max_allocated_storage: self.max_allocated_storage,
            tag_list: Some(self.tags.clone()),
            customer_owned_ip_enabled: Some(self.customer_owned_ip_enabled),
            network_type: Some(self.network_type.clone()),
            activity_stream_status: Some(ACTIVITY_STREAM_STOPPED.to_string()),
            backup_target: Some(self.backup_target.clone()),
            dedicated_log_volume: Some(self.dedicated_log_volume),
            // Nothing this service does changes an instance's storage file system
            // configuration, so no upgrade of it is ever recommended.
            is_storage_config_upgrade_available: Some(false),
            engine_lifecycle_support: Some(self.engine_lifecycle_support.clone()),
            ..DBInstance::default()
        }
    }
}

/// What a DB cluster reports about the records AROUND it.
///
/// A cluster's members and read replicas are other records — the DB instances that name
/// it and the clusters that replicate from it — so they are READ from those records when
/// the cluster is projected, never copied onto the cluster record: a copy would still
/// claim a member after the instance was deleted. Built by
/// [`ComAmazonawsRdsAmazonrdsv19State::cluster_to_wire`], which is the only way a handler
/// projects a cluster.
#[derive(Debug, Default)]
pub struct ClusterMembership {
    /// `DBClusterMembers`: the DB instances that make up the cluster, writer first.
    pub members: Vec<DBClusterMember>,
    /// `ReadReplicaIdentifiers`: the clusters whose replication source is this one.
    pub read_replica_identifiers: Vec<String>,
    /// `MultiAZ`: whether the member instances sit in more than one Availability Zone.
    pub multi_az: bool,
}

/// A stored DB cluster: everything the service persists about it.
///
/// As with a DB instance record, the ARN, the two cluster endpoints and the hosted zone
/// id are NOT stored as text: each is a function of the stored region, identifier and
/// minted endpoint host, so none of them can be left naming an identifier the cluster no
/// longer has. The cluster's members are not stored either — see [`ClusterMembership`].
#[derive(Debug, Clone)]
pub struct DbClusterRecord {
    /// `DBClusterIdentifier`, stored lowercased: "this parameter is stored as a lowercase
    /// string".
    pub identifier: String,
    /// The region the cluster was created in; the scope of its ARN and endpoints.
    pub region: String,
    /// `DbClusterResourceId`: the immutable, server-minted id.
    pub resource_id: String,
    /// The server-assigned middle label of the cluster endpoint DNS names, minted once.
    pub endpoint_host: String,
    /// `Status`.
    pub status: String,
    /// The transition the clock owes this cluster, if any.
    pub pending_status: Option<PendingStatus>,
    /// `Engine`.
    pub engine: String,
    /// `EngineVersion`: the version the cluster RUNS, resolved against the engine-version
    /// catalogue rather than the string the request carried.
    pub engine_version: Option<String>,
    /// `EngineMode`.
    pub engine_mode: String,
    /// `DatabaseName`.
    pub database_name: Option<String>,
    /// `MasterUsername`. The password is never stored: the member is `@sensitive` and RDS
    /// reports nothing carrying it.
    pub master_username: Option<String>,
    /// `Port`: the port the instances in the cluster accept connections on.
    pub port: i32,
    /// `AvailabilityZones`: where instances in the cluster can be created.
    pub availability_zones: Vec<String>,
    /// `BackupRetentionPeriod`.
    pub backup_retention_period: i32,
    /// `PreferredBackupWindow`.
    pub preferred_backup_window: String,
    /// `PreferredMaintenanceWindow`.
    pub preferred_maintenance_window: String,
    /// `DBClusterParameterGroup`: the group the request named, or the default group of the
    /// engine version the cluster was created with; `None` where this service publishes no
    /// version for the engine at all, so there is no family to name a default group after.
    pub db_cluster_parameter_group_name: Option<String>,
    /// `DBSubnetGroup`: the subnet group the cluster was created in, resolved in the store
    /// before the cluster was stored.
    pub db_subnet_group_name: Option<String>,
    /// The option group the request named, reported as the cluster's one option group
    /// membership.
    pub option_group_name: Option<String>,
    /// `CharacterSetName`.
    pub character_set_name: Option<String>,
    /// `VpcSecurityGroups`: the EC2 VPC security group ids the request named.
    pub vpc_security_group_ids: Vec<String>,
    /// `StorageEncrypted`.
    pub storage_encrypted: bool,
    /// `KmsKeyId`.
    pub kms_key_id: Option<String>,
    /// The storage type the cluster was created with, or `None` for an Aurora cluster on
    /// the default [`AURORA_DEFAULT_STORAGE_TYPE`], which the member documents as not
    /// reported.
    pub storage_type: Option<String>,
    /// `Iops`.
    pub iops: Option<i32>,
    /// `AllocatedStorage`.
    pub allocated_storage: Option<i32>,
    /// `DBClusterInstanceClass`, for a Multi-AZ DB cluster.
    pub db_cluster_instance_class: Option<String>,
    /// `PubliclyAccessible`, for a Multi-AZ DB cluster; `None` for an Aurora cluster,
    /// whose member is "only for Multi-AZ DB clusters".
    pub publicly_accessible: Option<bool>,
    /// `AutoMinorVersionUpgrade`.
    pub auto_minor_version_upgrade: bool,
    /// `DeletionProtection`.
    pub deletion_protection: bool,
    /// `CopyTagsToSnapshot`.
    pub copy_tags_to_snapshot: bool,
    /// `HttpEndpointEnabled`: whether the RDS Data API endpoint is on for this cluster.
    pub http_endpoint_enabled: bool,
    /// `IAMDatabaseAuthenticationEnabled`.
    pub iam_database_authentication_enabled: bool,
    /// `BacktrackWindow`, in seconds; zero disables backtracking.
    pub backtrack_window: i64,
    /// `EnabledCloudwatchLogsExports`.
    pub enabled_cloudwatch_logs_exports: Vec<String>,
    /// `NetworkType`, for an Aurora cluster; `None` for a Multi-AZ DB cluster, whose
    /// member is "only for Aurora DB clusters".
    pub network_type: Option<String>,
    /// `MonitoringInterval`.
    pub monitoring_interval: i32,
    /// `MonitoringRoleArn`.
    pub monitoring_role_arn: Option<String>,
    /// `DatabaseInsightsMode`.
    pub database_insights_mode: String,
    /// `PerformanceInsightsEnabled`.
    pub performance_insights_enabled: bool,
    /// `PerformanceInsightsKMSKeyId`.
    pub performance_insights_kms_key_id: Option<String>,
    /// `PerformanceInsightsRetentionPeriod`.
    pub performance_insights_retention_period: Option<i32>,
    /// `ServerlessV2ScalingConfiguration`: the Aurora Serverless v2 capacity range the
    /// request named, resolved against the capacity range its engine version publishes.
    pub serverless_v2_scaling: Option<ServerlessV2ScalingConfigurationInfo>,
    /// `ScalingConfigurationInfo`: the Aurora Serverless v1 scaling configuration the
    /// request named, for a cluster in the `serverless` engine mode.
    pub scaling_configuration: Option<ScalingConfigurationInfo>,
    /// `AssociatedRoles`: the IAM roles the request associated with the cluster.
    pub associated_roles: Vec<DBClusterRole>,
    /// `Domain` and `DomainIAMRoleName`, reported as the cluster's one domain membership.
    pub domain: Option<String>,
    /// See [`Self::domain`].
    pub domain_iam_role_name: Option<String>,
    /// `GlobalClusterIdentifier`: the global database this cluster is a member of, resolved
    /// in the store before the cluster was stored.
    pub global_cluster_identifier: Option<String>,
    /// `ReplicationSourceIdentifier`: the ARN of the cluster or instance this cluster
    /// replicates from, resolved in the store before the cluster was stored.
    pub replication_source_arn: Option<String>,
    /// The CA the cluster's server certificate is issued under.
    pub ca_certificate_identifier: String,
    /// `EngineLifecycleSupport`.
    pub engine_lifecycle_support: String,
    /// `ClusterScalabilityType`.
    pub cluster_scalability_type: String,
    /// `DBSystemId`: "reserved for future use".
    pub db_system_id: Option<String>,
    /// Whether `EnableLocalWriteForwarding` turned in-cluster write forwarding on.
    pub local_write_forwarding_enabled: bool,
    /// Whether `EnableGlobalWriteForwarding` asked for write forwarding to a global
    /// database's primary.
    pub global_write_forwarding_requested: bool,
    /// `ClusterCreateTime`.
    pub cluster_create_time: ::aws_smithy_types::DateTime,
    /// `TagList`.
    pub tags: Vec<Tag>,
}

impl DbClusterRecord {
    /// Apply a change whose due instant has passed. Returns whether the record is now
    /// gone, so the caller drops it from the store — the same contract as
    /// [`DbInstanceRecord::settle`].
    fn settle(&mut self, now: Instant) -> bool {
        if self.pending_status.as_ref().is_some_and(|p| now >= p.due_at) {
            let pending = self.pending_status.take().expect("checked just above");
            match pending.change {
                PendingChange::Status(status) => self.status = status,
                PendingChange::Removal => return true,
            }
        }
        false
    }

    /// `arn:aws:rds:<region>:<account>:cluster:<identifier>`, from the stored region and
    /// the cluster's current identifier.
    pub fn arn(&self) -> String {
        db_cluster_arn(&self.region, &self.identifier)
    }

    /// The cluster endpoint: "the connection endpoint for the primary instance of the DB
    /// cluster", whose published form is
    /// `<identifier>.cluster-<server-assigned label>.<region>.rds.amazonaws.com` (Amazon
    /// Aurora User Guide, "Amazon Aurora connection management").
    pub fn endpoint(&self) -> String {
        format!(
            "{}.cluster-{}.{}.rds.amazonaws.com",
            self.identifier, self.endpoint_host, self.region
        )
    }

    /// The reader endpoint, which "load-balances connections across the Aurora Replicas
    /// that are available in a DB cluster": the same form as [`Self::endpoint`] with the
    /// published `cluster-ro-` label.
    pub fn reader_endpoint(&self) -> String {
        format!(
            "{}.cluster-ro-{}.{}.rds.amazonaws.com",
            self.identifier, self.endpoint_host, self.region
        )
    }

    /// When the server certificate issued at creation expires:
    /// [`SERVER_CERTIFICATE_VALIDITY`] after the cluster was created, a function of the
    /// stored creation time so the two cannot drift.
    pub fn certificate_valid_till(&self) -> ::aws_smithy_types::DateTime {
        ::aws_smithy_types::DateTime::from_secs_f64(
            self.cluster_create_time.as_secs_f64() + SERVER_CERTIFICATE_VALIDITY.as_secs_f64(),
        )
    }

    /// `StorageEncryptionType`: "the type of encryption used to protect data at rest in
    /// the DB cluster".
    ///
    /// `sse-kms` when the request asked for encryption — "encrypted using a customer
    /// managed KMS key or Amazon Web Services managed KMS key". Otherwise it depends on
    /// the cluster type: Aurora storage is encrypted at rest either way, so an Aurora
    /// cluster reports `sse-rds`, "encrypted using an Amazon Web Services owned KMS key"
    /// (the member's own note: "If the DB cluster is encrypted with an Amazon Web Services
    /// owned key, this value is false and StorageEncryptionType is sse-rds"), while a
    /// Multi-AZ DB cluster created without encryption is not encrypted at all and reports
    /// `none`.
    pub fn storage_encryption_type(&self) -> &'static str {
        if self.storage_encrypted {
            STORAGE_ENCRYPTION_TYPE_SSE_KMS
        } else if is_multi_az_db_cluster(&self.engine) {
            STORAGE_ENCRYPTION_TYPE_NONE
        } else {
            STORAGE_ENCRYPTION_TYPE_SSE_RDS
        }
    }

    /// Project the stored record onto the modelled `DBCluster` output shape.
    ///
    /// Every member RDS reports for a cluster it holds is projected from the stored record
    /// or from the records `membership` was read out of; a member this service has no
    /// state for at all (the restorable-time window, the progress percentage, the clone
    /// group, the backtrack change-record count) is LEFT OUT rather than invented, while a
    /// list it does have state for is reported empty, which is what RDS reports for a
    /// cluster that has none of those associations.
    pub fn to_wire(&self, membership: &ClusterMembership) -> DBCluster {
        DBCluster {
            db_cluster_identifier: Some(self.identifier.clone()),
            db_cluster_arn: Some(self.arn()),
            db_cluster_resource_id: Some(self.resource_id.clone()),
            status: Some(self.status.clone()),
            engine: Some(self.engine.clone()),
            engine_version: self.engine_version.clone(),
            engine_mode: Some(self.engine_mode.clone()),
            database_name: self.database_name.clone(),
            master_username: self.master_username.clone(),
            port: Some(self.port),
            endpoint: Some(self.endpoint()),
            reader_endpoint: Some(self.reader_endpoint()),
            // Only `CreateDBClusterEndpoint` adds a custom endpoint, and it is still a
            // stub, so no cluster this service holds has one.
            custom_endpoints: Some(Vec::new()),
            hosted_zone_id: hosted_zone_id(&self.region).map(str::to_string),
            availability_zones: Some(self.availability_zones.clone()),
            backup_retention_period: Some(self.backup_retention_period),
            preferred_backup_window: Some(self.preferred_backup_window.clone()),
            preferred_maintenance_window: Some(self.preferred_maintenance_window.clone()),
            db_cluster_parameter_group: self.db_cluster_parameter_group_name.clone(),
            db_subnet_group: self.db_subnet_group_name.clone(),
            character_set_name: self.character_set_name.clone(),
            db_cluster_option_group_memberships: Some(
                self.option_group_name
                    .iter()
                    .map(|name| DBClusterOptionGroupStatus {
                        db_cluster_option_group_name: Some(name.clone()),
                        status: Some(MEMBERSHIP_IN_SYNC.to_string()),
                    })
                    .collect(),
            ),
            vpc_security_groups: Some(
                self.vpc_security_group_ids
                    .iter()
                    .map(|id| VpcSecurityGroupMembership {
                        vpc_security_group_id: Some(id.clone()),
                        status: Some(MEMBERSHIP_ACTIVE.to_string()),
                    })
                    .collect(),
            ),
            storage_encrypted: Some(self.storage_encrypted),
            storage_encryption_type: Some(self.storage_encryption_type().to_string()),
            kms_key_id: self.kms_key_id.clone(),
            storage_type: self.storage_type.clone(),
            iops: self.iops,
            allocated_storage: self.allocated_storage,
            db_cluster_instance_class: self.db_cluster_instance_class.clone(),
            publicly_accessible: self.publicly_accessible,
            auto_minor_version_upgrade: Some(self.auto_minor_version_upgrade),
            deletion_protection: Some(self.deletion_protection),
            copy_tags_to_snapshot: Some(self.copy_tags_to_snapshot),
            http_endpoint_enabled: Some(self.http_endpoint_enabled),
            iam_database_authentication_enabled: Some(self.iam_database_authentication_enabled),
            backtrack_window: Some(self.backtrack_window),
            enabled_cloudwatch_logs_exports: Some(self.enabled_cloudwatch_logs_exports.clone()),
            network_type: self.network_type.clone(),
            monitoring_interval: Some(self.monitoring_interval),
            monitoring_role_arn: self.monitoring_role_arn.clone(),
            database_insights_mode: Some(self.database_insights_mode.clone()),
            performance_insights_enabled: Some(self.performance_insights_enabled),
            performance_insights_kms_key_id: self.performance_insights_kms_key_id.clone(),
            performance_insights_retention_period: self.performance_insights_retention_period,
            serverless_v2_scaling_configuration: self.serverless_v2_scaling.clone(),
            scaling_configuration_info: self.scaling_configuration.clone(),
            associated_roles: Some(self.associated_roles.clone()),
            domain_memberships: Some(
                self.domain
                    .iter()
                    .map(|domain| DomainMembership {
                        domain: Some(domain.clone()),
                        status: Some("joined".to_string()),
                        iam_role_name: self.domain_iam_role_name.clone(),
                        ..DomainMembership::default()
                    })
                    .collect(),
            ),
            global_cluster_identifier: self.global_cluster_identifier.clone(),
            global_write_forwarding_requested: Some(self.global_write_forwarding_requested),
            local_write_forwarding_status: Some(
                if self.local_write_forwarding_enabled {
                    LOCAL_WRITE_FORWARDING_ENABLED
                } else {
                    LOCAL_WRITE_FORWARDING_DISABLED
                }
                .to_string(),
            ),
            replication_source_identifier: self.replication_source_arn.clone(),
            read_replica_identifiers: Some(membership.read_replica_identifiers.clone()),
            db_cluster_members: Some(membership.members.clone()),
            multi_az: Some(membership.multi_az),
            certificate_details: Some(CertificateDetails {
                ca_identifier: Some(self.ca_certificate_identifier.clone()),
                valid_till: Some(self.certificate_valid_till()),
            }),
            cluster_create_time: Some(self.cluster_create_time),
            cluster_scalability_type: Some(self.cluster_scalability_type.clone()),
            engine_lifecycle_support: Some(self.engine_lifecycle_support.clone()),
            db_system_id: self.db_system_id.clone(),
            upgrade_rollout_order: Some(DEFAULT_UPGRADE_ROLLOUT_ORDER.to_string()),
            // `StartActivityStream` is still a stub, so no cluster this service holds has
            // a database activity stream running.
            activity_stream_status: Some(ACTIVITY_STREAM_STOPPED.to_string()),
            // "Indicates whether the DB cluster is a clone of a DB cluster owned by a
            // different Amazon Web Services account": nothing here clones one.
            cross_account_clone: Some(false),
            tag_list: Some(self.tags.clone()),
            // Every change this service accepts for a cluster is applied to the stored
            // record at once, so no change is ever pending: the element is reported EMPTY,
            // which says exactly that, rather than omitted.
            pending_modified_values: Some(ClusterPendingModifiedValues::default()),
            ..DBCluster::default()
        }
    }
}

/// A stored DB snapshot: the source instance's settings as they stood when the
/// snapshot was taken, which is why they are copied rather than re-read later.
#[derive(Debug, Clone)]
pub struct DbSnapshotRecord {
    pub snapshot_identifier: String,
    /// The region the snapshot lives in; the scope of its ARN.
    pub region: String,
    pub db_instance_identifier: String,
    pub dbi_resource_id: String,
    pub engine: String,
    pub engine_version: Option<String>,
    pub allocated_storage: Option<i32>,
    pub master_username: Option<String>,
    pub availability_zone: String,
    pub port: i32,
    pub storage_type: String,
    pub encrypted: bool,
    pub kms_key_id: Option<String>,
    pub iops: Option<i32>,
    pub license_model: Option<String>,
    pub status: String,
    pub pending_status: Option<PendingStatus>,
    pub snapshot_type: String,
    pub instance_create_time: ::aws_smithy_types::DateTime,
    pub snapshot_create_time: ::aws_smithy_types::DateTime,
    pub tags: Vec<Tag>,
}

impl DbSnapshotRecord {
    /// Apply a change whose due instant has passed. Returns whether the record is now
    /// gone, exactly as [`DbInstanceRecord::settle`] does.
    fn settle(&mut self, now: Instant) -> bool {
        if self.pending_status.as_ref().is_some_and(|p| now >= p.due_at) {
            let pending = self.pending_status.take().expect("checked just above");
            match pending.change {
                PendingChange::Status(status) => self.status = status,
                PendingChange::Removal => return true,
            }
        }
        false
    }

    /// Snapshot the source instance's settings. `snapshot_type` is `manual` for a
    /// `CreateDBSnapshot` and for the final snapshot `DeleteDBInstance` takes —
    /// `automated` is reserved for the backups RDS itself schedules.
    pub fn from_instance(
        instance: &DbInstanceRecord,
        snapshot_identifier: &str,
        tags: Vec<Tag>,
    ) -> Self {
        Self {
            snapshot_identifier: snapshot_identifier.to_string(),
            region: instance.region.clone(),
            db_instance_identifier: instance.identifier.clone(),
            dbi_resource_id: instance.dbi_resource_id.clone(),
            engine: instance.engine.clone(),
            engine_version: instance.engine_version.clone(),
            allocated_storage: instance.allocated_storage,
            master_username: instance.master_username.clone(),
            availability_zone: instance.availability_zone.clone(),
            port: instance.endpoint_port,
            storage_type: instance.storage_type.clone(),
            encrypted: instance.storage_encrypted,
            kms_key_id: instance.kms_key_id.clone(),
            iops: instance.iops,
            license_model: instance.license_model.clone(),
            status: "creating".to_string(),
            pending_status: Some(PendingStatus::in_a_moment("available")),
            snapshot_type: "manual".to_string(),
            instance_create_time: instance.instance_create_time,
            snapshot_create_time: now_as_wire_timestamp(),
            tags,
        }
    }

    /// `arn:aws:rds:<region>:<account>:snapshot:<identifier>`.
    pub fn arn(&self) -> String {
        emulator_server::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("snapshot:{}", self.snapshot_identifier),
        )
    }

    /// Project the stored record onto the modelled `DBSnapshot` output shape.
    pub fn to_wire(&self) -> DBSnapshot {
        DBSnapshot {
            db_snapshot_identifier: Some(self.snapshot_identifier.clone()),
            db_instance_identifier: Some(self.db_instance_identifier.clone()),
            snapshot_create_time: Some(self.snapshot_create_time),
            engine: Some(self.engine.clone()),
            allocated_storage: self.allocated_storage,
            status: Some(self.status.clone()),
            port: Some(self.port),
            availability_zone: Some(self.availability_zone.clone()),
            instance_create_time: Some(self.instance_create_time),
            master_username: self.master_username.clone(),
            engine_version: self.engine_version.clone(),
            license_model: self.license_model.clone(),
            snapshot_type: Some(self.snapshot_type.clone()),
            iops: self.iops,
            // 0 while the snapshot is still being taken, 100 once it is available:
            // the progress of the copy this record stands for, not a stored field.
            percent_progress: Some(if self.status == "available" { 100 } else { 0 }),
            storage_type: Some(self.storage_type.clone()),
            encrypted: Some(self.encrypted),
            kms_key_id: self.kms_key_id.clone(),
            db_snapshot_arn: Some(self.arn()),
            dbi_resource_id: Some(self.dbi_resource_id.clone()),
            tag_list: Some(self.tags.clone()),
            ..DBSnapshot::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Cluster parameter groups
//
// A cluster parameter group is a named, family-scoped set of engine parameters that
// `CreateDBClusterParameterGroup` mints, `ModifyDBClusterParameterGroup` writes into,
// `ResetDBClusterParameterGroup` clears, the two describes report and
// `DeleteDBClusterParameterGroup` removes. The group carries the parameters a CALLER has
// set; the engine's own defaults belong to the FAMILY, which publishes them
// (`published_parameters`), and are not materialised into a group — which is why a freshly
// created group reports an EMPTY `Parameters` list.
//
// This is the control plane an AWS SDK Amazon RDS, Amazon DocumentDB or Amazon Neptune
// client reaches: all three sign `rds`, which resolves to this service (see
// `src/service.rs`), exactly as all three APIs are served on the RDS endpoint by AWS.
// ---------------------------------------------------------------------------

/// One parameter value a caller set on a group.
#[derive(Debug, Clone)]
pub struct ParameterSetting {
    /// `Parameter$ParameterValue`, as the caller set it.
    pub value: String,
    /// `Parameter$ApplyMethod`, as the caller asked for it (see
    /// [`APPLY_METHOD_PENDING_REBOOT`] for what an absent one becomes).
    pub apply_method: String,
}

/// A persisted cluster parameter group.
///
/// The name is stored lowercased and is half of this record's key:
/// `CreateDBClusterParameterGroupMessage$DBClusterParameterGroupName` states "This value is
/// stored as a lowercase string."
#[derive(Debug, Clone)]
pub struct DbClusterParameterGroupRecord {
    /// The lowercased `DBClusterParameterGroupName`.
    pub name: String,
    /// `DBParameterGroupFamily`, as the caller named it.
    pub family: String,
    /// `Description`.
    pub description: String,
    /// The region whose credential scope the create was signed with.
    pub region: String,
    /// The parameters a caller has set, by `ParameterName`.
    pub parameters: BTreeMap<String, ParameterSetting>,
    /// The group's tags.
    pub tags: Vec<Tag>,
}

impl DbClusterParameterGroupRecord {
    /// `DBClusterParameterGroupArn`. The resource type is `cluster-pg`, the one AWS uses
    /// for a cluster parameter group under this service's `rds` ARN namespace.
    pub fn arn(&self) -> String {
        emulator_server::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("cluster-pg:{}", self.name),
        )
    }

    /// This record as the modelled `DBClusterParameterGroup`.
    pub fn to_wire(&self) -> DBClusterParameterGroup {
        DBClusterParameterGroup {
            db_cluster_parameter_group_name: Some(self.name.clone()),
            db_parameter_group_family: Some(self.family.clone()),
            description: Some(self.description.clone()),
            db_cluster_parameter_group_arn: Some(self.arn()),
        }
    }

    /// Whether the change to `name` is still waiting on a reboot of the cluster's
    /// instances; see `service_common::rds_cluster_parameters::change_is_pending` for the
    /// documented rule.
    pub fn parameter_is_pending(&self, name: &str) -> bool {
        let Some(setting) = self.parameters.get(name) else {
            return false;
        };
        change_is_pending(&self.family, name, &setting.apply_method)
    }

    /// The names of the parameter changes this group is still waiting on a reboot for, in
    /// name order; see [`Self::parameter_is_pending`].
    pub fn pending_parameters(&self) -> Vec<String> {
        self.parameters
            .keys()
            .filter(|name| self.parameter_is_pending(name))
            .cloned()
            .collect()
    }

    /// The parameter `name` as the modelled `Parameter`: the value and apply method the
    /// caller set, plus the metadata the family publishes for it when it publishes any.
    pub fn parameter_to_wire(&self, name: &str) -> Parameter {
        let setting = self.parameters.get(name);
        let spec = published_parameter(&self.family, name);
        Parameter {
            parameter_name: Some(name.to_string()),
            parameter_value: setting.map(|setting| setting.value.clone()),
            description: spec.map(|spec| spec.description.to_string()),
            source: Some(PARAMETER_SOURCE_USER.to_string()),
            apply_type: spec.map(|spec| spec.apply_type.to_string()),
            data_type: spec.map(|spec| spec.data_type.to_string()),
            allowed_values: spec.map(|spec| spec.allowed_values.to_string()),
            is_modifiable: spec.map(|spec| spec.is_modifiable),
            minimum_engine_version: spec.map(|spec| spec.minimum_engine_version.to_string()),
            apply_method: setting.map(|setting| setting.apply_method.clone()),
            supported_engine_modes: None,
        }
    }
}

/// A parameter the engine publishes for a family, as the modelled `Parameter` with its
/// default value and `Source` [`PARAMETER_SOURCE_ENGINE_DEFAULT`]: what a describe reports
/// when the caller asks for the engine's own parameters rather than a group's.
pub fn engine_default_parameter_to_wire(spec: &ClusterParameterSpec) -> Parameter {
    Parameter {
        parameter_name: Some(spec.name.to_string()),
        parameter_value: Some(spec.default_value.to_string()),
        description: Some(spec.description.to_string()),
        source: Some(PARAMETER_SOURCE_ENGINE_DEFAULT.to_string()),
        apply_type: Some(spec.apply_type.to_string()),
        data_type: Some(spec.data_type.to_string()),
        allowed_values: Some(spec.allowed_values.to_string()),
        is_modifiable: Some(spec.is_modifiable),
        minimum_engine_version: Some(spec.minimum_engine_version.to_string()),
        apply_method: None,
        supported_engine_modes: None,
    }
}

/// A stored RDS event notification subscription: the record `DescribeEventSubscriptions`
/// reports.
///
/// Regional, like the DB instances and snapshots above: a subscription is created in one
/// region and `DescribeEventSubscriptions` lists that region's subscriptions.
#[derive(Debug, Clone)]
pub struct EventSubscriptionRecord {
    /// The caller's `SubscriptionName`, and the second half of this record's key.
    pub subscription_name: String,
    /// The region the subscription was created in; the scope of its ARN.
    pub region: String,
    /// `SnsTopicArn`: the topic RDS publishes the events to.
    pub sns_topic_arn: String,
    /// `Status`: "Can be one of the following: creating | modifying | deleting | active |
    /// no-permission | topic-not-exist" (`EventSubscription$Status`).
    pub status: String,
    /// `SubscriptionCreationTime`, which the model types as a STRING rather than a
    /// timestamp, so the record carries the rendered form its writer chose.
    pub subscription_creation_time: String,
    /// `SourceType`, absent when the subscription covers every source type.
    pub source_type: Option<String>,
    /// `SourceIdsList`.
    pub source_ids: Vec<String>,
    /// `EventCategoriesList`.
    pub event_categories: Vec<String>,
    /// `Enabled`.
    pub enabled: bool,
    /// The tags `CreateEventSubscription$Tags` assigned to the subscription. The modelled
    /// `EventSubscription` shape carries no tag member, so nothing reports them yet —
    /// `ListTagsForResource`, which does, is still scaffolded — but the caller's input is
    /// kept rather than dropped, so it is here when that operation lands.
    pub tags: Vec<Tag>,
}

impl EventSubscriptionRecord {
    /// A subscription as `CreateEventSubscription` mints it: RDS answers the create with
    /// the subscription already [`EVENT_SUBSCRIPTION_STATUS_ACTIVE`], so there is no
    /// transient status to owe and nothing for the clock to settle later.
    #[allow(clippy::too_many_arguments)]
    pub fn active(
        subscription_name: String,
        region: String,
        sns_topic_arn: String,
        subscription_creation_time: String,
        source_type: Option<String>,
        source_ids: Vec<String>,
        event_categories: Vec<String>,
        enabled: bool,
        tags: Vec<Tag>,
    ) -> Self {
        Self {
            subscription_name,
            region,
            sns_topic_arn,
            status: EVENT_SUBSCRIPTION_STATUS_ACTIVE.to_string(),
            subscription_creation_time,
            source_type,
            source_ids,
            event_categories,
            enabled,
            tags,
        }
    }

    /// Accept a delete: the subscription's own answer reports the `deleting` status that
    /// says the delete was accepted. A notification rule has nothing provisioned to tear
    /// down, so the record leaves the store with that call rather than a span later — see
    /// `DeleteEventSubscription`.
    pub fn deleting(&self) -> Self {
        Self {
            status: EVENT_SUBSCRIPTION_STATUS_DELETING.to_string(),
            ..self.clone()
        }
    }

    /// `arn:aws:rds:<region>:<account>:es:<subscription name>` — the `es` resource type
    /// RDS gives an event subscription.
    pub fn arn(&self) -> String {
        emulator_server::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("es:{}", self.subscription_name),
        )
    }

    /// Project the stored record onto the modelled `EventSubscription` output shape.
    pub fn to_wire(&self) -> EventSubscription {
        EventSubscription {
            customer_aws_id: Some(emulator_server::emulator_account_id().to_string()),
            cust_subscription_id: Some(self.subscription_name.clone()),
            sns_topic_arn: Some(self.sns_topic_arn.clone()),
            status: Some(self.status.clone()),
            subscription_creation_time: Some(self.subscription_creation_time.clone()),
            source_type: self.source_type.clone(),
            source_ids_list: Some(self.source_ids.clone()),
            event_categories_list: Some(self.event_categories.clone()),
            enabled: Some(self.enabled),
            event_subscription_arn: Some(self.arn()),
        }
    }
}

/// A stored option group: everything `CreateOptionGroup` persists about one, plus the
/// options `ModifyOptionGroup` has added to it.
///
/// The ARN is NOT stored as text: it is a function of the stored region and name,
/// which [`OptionGroupRecord::to_wire`] computes, so it cannot drift from the record
/// the way a stored copy could.
#[derive(Debug, Clone)]
pub struct OptionGroupRecord {
    pub name: String,
    /// The region the group was created in; the scope of its ARN.
    pub region: String,
    pub description: String,
    pub engine_name: String,
    pub major_engine_version: String,
    pub tags: Vec<Tag>,
    /// The options `ModifyOptionGroup` added, by the catalogue's own spelling of each
    /// option's name. Ordered, so the group reports its options the same way on every
    /// read, and keyed by name, so adding an option twice updates its configuration
    /// instead of storing a second copy of it.
    pub options: BTreeMap<String, OptionRecord>,
}

impl OptionGroupRecord {
    /// `arn:aws:rds:<region>:<account>:og:<name>` — `og` is the resource type RDS
    /// gives an option group in its ARN format.
    pub fn arn(&self) -> String {
        emulator_server::emulator_arn(ARN_NAMESPACE, &self.region, &format!("og:{}", self.name))
    }

    /// Project the stored record onto the modelled `OptionGroup` output shape.
    pub fn to_wire(&self) -> OptionGroup {
        let options: Vec<Option2> = self
            .options
            .values()
            .map(|option| option.to_wire(&self.engine_name))
            .collect();
        OptionGroup {
            option_group_name: Some(self.name.clone()),
            option_group_description: Some(self.description.clone()),
            engine_name: Some(self.engine_name.clone()),
            major_engine_version: Some(self.major_engine_version.clone()),
            // A group nothing has added an option to reports an EMPTY list rather than
            // no list at all: the group exists and has nothing in it.
            // `AllowsVpcAndNonVpcInstanceMemberships` — "Indicates whether this option
            // group can be applied to both VPC and non-VPC instances" — is ALWAYS
            // reported, so a caller can tell a group it may apply anywhere from one it
            // may not. A group holding no option constrains nothing and can be applied
            // to both; once it holds an option, that option's own placement
            // requirements bind the group to instances of one network type, so the
            // answer is `false`.
            allows_vpc_and_non_vpc_instance_memberships: Some(options.is_empty()),
            options: Some(options),
            option_group_arn: Some(self.arn()),
            // `VpcId` belongs to a group bound to ONE VPC, which nothing here creates:
            // "If AllowsVpcAndNonVpcInstanceMemberships is false, this field is blank.
            // If AllowsVpcAndNonVpcInstanceMemberships is true and this field is blank,
            // then this option group can be applied to both VPC and non-VPC
            // instances." `SourceOptionGroup`/`SourceAccountId`/`CopyTimestamp` belong
            // to a group `CopyOptionGroup` produced. None describes a group created
            // here, so none is reported rather than carrying a made-up value.
            ..OptionGroup::default()
        }
    }
}

/// Emulator resource guard, NOT an AWS bound: the longest caller-supplied free-form string
/// this service stores for a member whose length the API publishes no limit for (a
/// parameter group's `Description` or `DBParameterGroupFamily`, a parameter's name or
/// value). Nothing caller-sized reaches the store unchecked.
pub const MAX_FREE_FORM_LEN: usize = 512;

/// Refuse a caller-supplied free-form string longer than [`MAX_FREE_FORM_LEN`].
pub fn check_free_form(member: &str, value: &str) -> Result<(), String> {
    let length = value.chars().count();
    if length > MAX_FREE_FORM_LEN {
        return Err(format!(
            "{member} may be at most {MAX_FREE_FORM_LEN} characters, got {length}"
        ));
    }
    Ok(())
}

/// The name AWS gives the DB subnet group it creates alongside an account's default
/// VPC. `CreateDBSubnetGroup$DBSubnetGroupName` refuses it ("Must not be default")
/// because the service owns that name, and `CreateDBInstance$PubliclyAccessible`
/// documents a default that branches on a request naming it.
pub const DEFAULT_DB_SUBNET_GROUP: &str = "default";

/// A stored DB subnet group: the name, the description and the EC2 subnets the request
/// named.
///
/// It exists so a request that NAMES a DB subnet group can be resolved: an instance
/// created against a group that was never created is a reference to nothing, which the
/// real service refuses rather than accepts. The ARN is computed from the stored region
/// and name, like the other records', so it cannot drift from the record.
#[derive(Debug, Clone)]
pub struct DbSubnetGroupRecord {
    pub name: String,
    /// The region the group was created in; the scope of its ARN.
    pub region: String,
    pub description: String,
    /// The EC2 subnet ids the request named, in the order it named them.
    pub subnet_ids: Vec<String>,
    pub tags: Vec<Tag>,
}

impl DbSubnetGroupRecord {
    /// The account's `default` DB subnet group in `region`: the group the real service
    /// provides with the account's default VPC, which no request creates and which
    /// therefore is not stored. Its subnets are the default VPC's subnets — EC2 facts
    /// this service does not hold — so it names none rather than inventing any.
    pub fn account_default(region: &str) -> Self {
        Self {
            name: DEFAULT_DB_SUBNET_GROUP.to_string(),
            region: region.to_string(),
            description: DEFAULT_DB_SUBNET_GROUP.to_string(),
            subnet_ids: Vec::new(),
            tags: Vec::new(),
        }
    }
    /// `arn:aws:rds:<region>:<account>:subgrp:<name>` — `subgrp` is the resource type
    /// RDS gives a DB subnet group in its ARN format.
    pub fn arn(&self) -> String {
        emulator_server::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("subgrp:{}", self.name),
        )
    }

    /// Project the stored record onto the modelled `DBSubnetGroup` output shape.
    pub fn to_wire(&self) -> DBSubnetGroup {
        DBSubnetGroup {
            db_subnet_group_name: Some(self.name.clone()),
            db_subnet_group_description: Some(self.description.clone()),
            subnet_group_status: Some("Complete".to_string()),
            // A group that names no subnet reports NO `Subnets` member rather than an
            // empty list: an empty list would claim the group covers nothing, which is
            // a different statement from "this service does not hold its subnets".
            subnets: (!self.subnet_ids.is_empty()).then(|| {
                self.subnet_ids
                    .iter()
                    .map(|id| Subnet {
                        subnet_identifier: Some(id.clone()),
                        subnet_status: Some("Active".to_string()),
                        // The Availability Zone a subnet lies in, and the VPC it
                        // belongs to, are EC2 facts: RDS reads them from the subnet,
                        // and this service holds no subnets. Reported ABSENT rather
                        // than carrying a made-up zone or VPC id, which a caller that
                        // placed an instance by zone would be misled by.
                        subnet_availability_zone: None,
                        // `SubnetOutpost` describes a subnet on an Outpost, which
                        // nothing here is.
                        subnet_outpost: None,
                    })
                    .collect()
            }),
            db_subnet_group_arn: Some(self.arn()),
            // `VpcId` and `SupportedNetworkTypes` are read off the subnets too, for the
            // same reason.
            ..DBSubnetGroup::default()
        }
    }
}

/// A stored Aurora global database cluster: the record `DescribeGlobalClusters` reports.
///
/// Account-scoped rather than regional — see
/// [`ComAmazonawsRdsAmazonrdsv19State::global_clusters`].
#[derive(Debug, Clone)]
pub struct GlobalClusterRecord {
    /// The caller's `GlobalClusterIdentifier`, and this record's key.
    pub identifier: String,
    /// `GlobalClusterResourceId`: the immutable, server-minted id.
    pub resource_id: String,
    /// `Status`.
    pub status: String,
    /// `Engine`.
    pub engine: Option<String>,
    /// `EngineVersion`.
    pub engine_version: Option<String>,
    /// `EngineLifecycleSupport`: the lifecycle type the global cluster is enrolled in —
    /// the value the request named, or [`DEFAULT_ENGINE_LIFECYCLE_SUPPORT`].
    pub engine_lifecycle_support: String,
    /// `DatabaseName`.
    pub database_name: Option<String>,
    /// `StorageEncrypted`.
    pub storage_encrypted: bool,
    /// `DeletionProtection`.
    pub deletion_protection: bool,
    /// The ARNs of the member DB clusters, the PRIMARY (writer) first: that order is what
    /// [`Self::to_wire`] reports as `GlobalClusterMember$IsWriter`, and the regions in
    /// these ARNs are what the documented `region` filter of `DescribeGlobalClusters`
    /// matches on.
    pub member_cluster_arns: Vec<String>,
    /// The DNS label AWS assigns the global database's own writer endpoint between the
    /// identifier and the `global` suffix: a server-minted token, unique per global
    /// cluster — see [`Self::endpoint`].
    pub endpoint_host: String,
    /// `TagList`: the tags `CreateGlobalCluster$Tags` assigned to the global cluster.
    pub tags: Vec<Tag>,
}

impl GlobalClusterRecord {
    /// `arn:aws:rds::<account>:global-cluster:<identifier>`. A global cluster spans
    /// regions, so — as AWS does — the ARN's region segment is EMPTY.
    pub fn arn(&self) -> String {
        emulator_server::emulator_arn(
            ARN_NAMESPACE,
            "",
            &format!("global-cluster:{}", self.identifier),
        )
    }

    /// Whether any member cluster of this global cluster lives in `region`, which is what
    /// the one documented filter of `DescribeGlobalClusters` selects on: "the only
    /// supported filter is `region` … any global cluster with at least one member (primary
    /// or secondary) in the specified Amazon Web Services Regions".
    pub fn has_member_in(&self, region: &str) -> bool {
        self.member_cluster_arns
            .iter()
            .any(|arn| arn_region(arn) == Some(region))
    }

    /// The global database's own writer endpoint, `<identifier>.global-<minted
    /// host>.global.rds.amazonaws.com`.
    ///
    /// `GlobalCluster$Endpoint` is "the writer endpoint for the new global database
    /// cluster. This endpoint always points to the writer DB instance in the current
    /// primary cluster" — the Aurora global database writer endpoint, whose published form
    /// is the global cluster's identifier, a server-assigned label and the
    /// `global.rds.amazonaws.com` suffix (Amazon Aurora User Guide, "Using the writer
    /// endpoint of an Amazon Aurora global database"). It is a function of the stored
    /// identifier and minted host rather than stored text, so it cannot drift from the
    /// record; the label itself is server-assigned, as the DB instance endpoint's is.
    pub fn endpoint(&self) -> String {
        format!(
            "{}.global-{}.global.rds.amazonaws.com",
            self.identifier, self.endpoint_host
        )
    }

    /// `StorageEncryptionType`: "the type of encryption used to protect data at rest IN
    /// the global database cluster".
    ///
    /// Aurora storage is encrypted at rest either way, so the member reports WHICH key
    /// class protects it rather than whether anything does: `sse-kms` — "encrypted using a
    /// customer managed KMS key or Amazon Web Services managed KMS key" — when
    /// `StorageEncrypted` asked for KMS encryption (`CreateGlobalCluster` names no key of
    /// its own, so the key is the Amazon Web Services managed one RDS encrypts with by
    /// default), and otherwise `sse-rds`, "encrypted using an Amazon Web Services owned
    /// KMS key", the key the service applies itself. The third published value, `none`, is
    /// for a global database whose data is not encrypted at all, which this service
    /// creates none of.
    pub fn storage_encryption_type(&self) -> &'static str {
        if self.storage_encrypted {
            STORAGE_ENCRYPTION_TYPE_SSE_KMS
        } else {
            STORAGE_ENCRYPTION_TYPE_SSE_RDS
        }
    }

    /// Accept a delete: the global cluster's own answer reports the `deleting` status that
    /// says the delete was accepted. Nothing is provisioned for a global database that
    /// holds no member cluster, so the record leaves the store with this call rather than
    /// a span later — see `DeleteGlobalCluster`.
    pub fn deleting(&self) -> Self {
        Self {
            status: GLOBAL_CLUSTER_STATUS_DELETING.to_string(),
            ..self.clone()
        }
    }

    /// Project the stored record onto the modelled `GlobalCluster` output shape.
    pub fn to_wire(&self) -> GlobalCluster {
        GlobalCluster {
            global_cluster_identifier: Some(self.identifier.clone()),
            global_cluster_resource_id: Some(self.resource_id.clone()),
            global_cluster_arn: Some(self.arn()),
            status: Some(self.status.clone()),
            engine: self.engine.clone(),
            engine_version: self.engine_version.clone(),
            engine_lifecycle_support: Some(self.engine_lifecycle_support.clone()),
            database_name: self.database_name.clone(),
            storage_encrypted: Some(self.storage_encrypted),
            // "The type of encryption used to protect data at rest in the global database
            // cluster" — see [`Self::storage_encryption_type`].
            storage_encryption_type: Some(self.storage_encryption_type().to_string()),
            // The global database's own writer endpoint — see [`Self::endpoint`].
            endpoint: Some(self.endpoint()),
            deletion_protection: Some(self.deletion_protection),
            global_cluster_members: Some(
                self.member_cluster_arns
                    .iter()
                    .enumerate()
                    .map(|(position, arn)| GlobalClusterMember {
                        db_cluster_arn: Some(arn.clone()),
                        // The primary is stored first; every other member is a secondary.
                        is_writer: Some(position == 0),
                        readers: Some(Vec::new()),
                        ..GlobalClusterMember::default()
                    })
                    .collect(),
            ),
            tag_list: Some(self.tags.clone()),
            // `FailoverState` "is empty unless the SwitchoverGlobalCluster or
            // FailoverGlobalCluster operation was called on this global cluster", and
            // neither is implemented in this crate, so nothing can have set one.
            ..GlobalCluster::default()
        }
    }
}

/// One option a `ModifyOptionGroup` call added to a group: the caller's CONFIGURATION
/// of it.
///
/// The option's own facts — its description, whether it is permanent or persistent,
/// the settings it publishes and each setting's default, data type and allowed values
/// — are NOT copied in here. They are catalogue data ([`option_catalog`]), which
/// [`OptionRecord::to_wire`] reads at projection time, so a stored group cannot drift
/// from the catalogue the way a stored copy could.
#[derive(Debug, Clone, Default)]
pub struct OptionRecord {
    /// The catalogue's own spelling of the option's name.
    pub name: String,
    /// `OptionConfiguration$Port`, when the request named one; otherwise the option's
    /// published port, if it has one.
    pub port: Option<i32>,
    /// `OptionConfiguration$OptionVersion`, when the request named one.
    pub version: Option<String>,
    /// `OptionConfiguration$DBSecurityGroupMemberships`.
    pub db_security_group_memberships: Vec<String>,
    /// `OptionConfiguration$VpcSecurityGroupMemberships`.
    pub vpc_security_group_memberships: Vec<String>,
    /// The values the caller set, by setting name — only for the settings it set. A
    /// setting absent here reports the catalogue's default, which is what makes
    /// `Value` fall back to `DefaultValue`.
    pub settings: BTreeMap<String, String>,
}

impl OptionRecord {
    /// Project the stored option onto the modelled `Option` wire shape, in the context
    /// of the group's engine: the engine is half of the catalogue key, because an
    /// option one engine publishes is a different catalogue entry than another's.
    pub fn to_wire(&self, engine_name: &str) -> Option2 {
        let facts = option_catalog::option_facts(engine_name, &self.name);
        let settings = facts
            .map(|facts| {
                facts
                    .settings
                    .iter()
                    .map(|setting| OptionSetting {
                        name: Some(setting.name.to_string()),
                        // The value the caller set, else the published default: that
                        // is what "The current value of the option setting" is for a
                        // setting nobody has set.
                        value: Some(
                            self.settings
                                .get(setting.name)
                                .cloned()
                                .unwrap_or_else(|| setting.default_value.to_string()),
                        ),
                        default_value: Some(setting.default_value.to_string()),
                        description: Some(setting.description.to_string()),
                        apply_type: Some(setting.apply_type.to_string()),
                        data_type: Some(setting.data_type.to_string()),
                        // A free-form setting publishes no value list, and reports no
                        // `AllowedValues` member rather than an empty one.
                        allowed_values: (!setting.allowed_values.is_empty())
                            .then(|| setting.allowed_values.to_string()),
                        is_modifiable: Some(setting.is_modifiable),
                        is_collection: Some(setting.is_collection),
                    })
                    .collect()
            })
            // Unreachable with a record this service stored: `ModifyOptionGroup`
            // refuses an option the group's engine does not publish, so every stored
            // option has catalogue facts. An option whose entry has since been
            // dropped from the catalogue reports what the record itself holds rather
            // than invented metadata.
            .unwrap_or_default();
        Option2 {
            option_name: Some(self.name.clone()),
            option_description: facts.map(|facts| facts.description.to_string()),
            persistent: facts.map(|facts| facts.persistent),
            permanent: facts.map(|facts| facts.permanent),
            // "If required, the port configured for this option to use" — so no port
            // at all for an option that requires none.
            port: self.port.or_else(|| facts.and_then(|facts| facts.port)),
            option_version: self.version.clone(),
            option_settings: Some(settings),
            // Both membership lists are reported even when the option was configured
            // with none: an empty list says the option opened access to nothing, which
            // is not the same claim as no list at all.
            db_security_group_memberships: Some(
                self.db_security_group_memberships
                    .iter()
                    .map(|name| DBSecurityGroupMembership {
                        db_security_group_name: Some(name.clone()),
                        status: Some("authorized".to_string()),
                    })
                    .collect(),
            ),
            vpc_security_group_memberships: Some(
                self.vpc_security_group_memberships
                    .iter()
                    .map(|id| VpcSecurityGroupMembership {
                        vpc_security_group_id: Some(id.clone()),
                        status: Some("active".to_string()),
                    })
                    .collect(),
            ),
        }
    }
}

/// Refuse a value that is not of the published EC2 subnet id form: `subnet-` followed
/// by 8 or 17 lowercase hexadecimal characters. A value that is not a subnet id at all
/// names no subnet, which is the modelled `InvalidSubnet`.
pub fn check_subnet_id(subnet_id: &str) -> Result<(), String> {
    let suffix = subnet_id.strip_prefix("subnet-").unwrap_or_default();
    if (suffix.len() == 8 || suffix.len() == 17)
        && suffix
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Ok(());
    }
    Err(format!(
        "'{subnet_id}' is not an EC2 subnet id (expected subnet- followed by 8 or 17 \
         hexadecimal characters)"
    ))
}

/// The region segment of an ARN (`arn:aws:<service>:<region>:<account>:<resource>`), or
/// `None` when `value` does not have that shape. Empty for a region-less ARN, which is
/// how a global resource's ARN reads.
fn arn_region(value: &str) -> Option<&str> {
    let mut fields = value.split(':');
    let (Some("arn"), Some(_partition), Some(_service), Some(region)) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return None;
    };
    Some(region)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRdsAmazonrdsv19State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRdsAmazonrdsv19State>)
}

/// The SHARED data-source directory in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// A monitoring service addresses a DB resource by the immutable resource id THIS
/// service minted for it (`DbiResourceId`), and the form of that id does not say which
/// service minted it — Amazon DocumentDB mints ids of the same form. So a minted id is
/// recorded here, in ONE directory under one shared type
/// ([`::service_common::db_data_sources::DataSourceDirectory`]), which is how
/// Performance Insights can decide whether its request's `ServiceType` is the service
/// that owns the resource the request names. The engine keys state by TYPE, so a copy
/// of its own inside [`ComAmazonawsRdsAmazonrdsv19State`] would be invisible to the
/// readers.
///
/// LOCK ORDER: a writer takes [`service_state`] first and this directory second, and
/// no reader holds this one while taking another store, so the order cannot invert.
///
/// Answer `None` by leaving the directory unwritten: the record is an aid to another
/// service, never part of this operation's own answer.
pub fn data_source_directory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::db_data_sources::DataSourceDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::db_data_sources::DataSourceDirectory,
            >,
        )
}

/// The region this request was signed for: every ARN and endpoint host this service
/// mints is scoped to it, and it is half of every stored record's key.
///
/// Falls back to `us-east-1`, the region a generated emulator defaults to, when the
/// request carries no SigV4 credential scope to read it from. A scope that names
/// something that is not an AWS region id is refused rather than stored: the value is
/// request data this emulator does not authenticate, and it keys the store.
pub fn request_region(
    ctx: &smithy_server_runtime::OperationContext<'_>,
) -> Result<String, String> {
    let region = emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| "us-east-1".to_string());
    check_region(&region)?;
    Ok(region)
}

/// The longest AWS region id this service accepts. The longest one AWS publishes is
/// 14 characters (`ap-southeast-4`); 32 leaves room for any it adds.
const MAX_REGION_LEN: usize = 32;

/// Refuse a credential-scope region that is not of the published AWS region-id form:
/// lowercase letters in two or more hyphen-separated groups, then a one- or
/// two-digit number — `us-east-1`, `eu-central-1`, `ap-southeast-4`, `us-gov-west-1`,
/// `cn-north-1`.
pub fn check_region(region: &str) -> Result<(), String> {
    let malformed = || {
        format!(
            "the credential scope names '{region}', which is not an AWS region id \
             (expected a form like us-east-1, at most {MAX_REGION_LEN} characters)"
        )
    };
    if region.is_empty() || region.len() > MAX_REGION_LEN {
        return Err(malformed());
    }
    let mut parts: Vec<&str> = region.split('-').collect();
    let number = parts.pop().ok_or_else(malformed)?;
    if parts.len() < 2
        || !(1..=2).contains(&number.len())
        || !number.bytes().all(|b| b.is_ascii_digit())
        || !parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_lowercase()))
    {
        return Err(malformed());
    }
    Ok(())
}

/// `arn:aws:rds:<region>:<account>:db:<identifier>`.
pub fn db_instance_arn(region: &str, identifier: &str) -> String {
    emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("db:{identifier}"))
}

/// `arn:aws:rds:<region>:<account>:cluster:<identifier>` — the ARN form RDS gives a DB
/// cluster, which is also what `DescribeDBClusters$DBClusterIdentifier` and the
/// `db-cluster-id` filter accept in place of the identifier.
pub fn db_cluster_arn(region: &str, identifier: &str) -> String {
    emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("cluster:{identifier}"))
}

/// The DB cluster identifier `named` carries: `named` itself, or the resource part of it
/// when it is a DB cluster ARN.
///
/// Several members accept either form — "the user-supplied DB cluster identifier or the
/// Amazon Resource Name (ARN) of the DB cluster" — so both are folded to the identifier
/// the store is keyed by. A value that is an ARN of something else keeps its last segment,
/// which names no cluster and so resolves to nothing.
pub fn db_cluster_identifier_of(named: &str) -> &str {
    match named.strip_prefix("arn:") {
        None => named,
        Some(rest) => rest.rsplit(':').next().unwrap_or(named),
    }
}

/// The id one response reports as its own: the value of the `<RequestId>` element an
/// awsQuery `<ErrorResponse>` envelope carries, the counterpart of the
/// `ResponseMetadata/RequestId` the framework's success renderer writes.
///
/// A correlation id, not a security token, and per-response rather than per-resource: two
/// responses must be distinguishable, which is what a client logs and an SDK's retry and
/// telemetry layers read. It needs no state, so it is a free function rather than a method
/// on the store — a request refused before any store is reached still answers with one.
///
/// The form is the 8-4-4-4-12 version-4 UUID every AWS request id carries, the same shape
/// the framework's own success-path minter produces, so a client cannot tell the two
/// apart. Unique because a monotone counter goes into the digest; the digest is the
/// maintained `sha2` crate's, so nothing here is hand-rolled.
pub fn new_request_id() -> String {
    use ::sha2::{Digest, Sha256};
    use ::std::sync::atomic::{AtomicU64, Ordering};

    /// The monotone part of a request id, so two ids minted inside one clock tick still
    /// differ.
    static MINTED: AtomicU64 = AtomicU64::new(0);

    let counter = MINTED.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_nanos());
    let digest = Sha256::digest(format!("request:{counter}:{nanos}"));
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // The version (4) and variant (RFC 4122) nibbles, so the id is well formed and not
    // mistaken for a name-based or time-based one.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// The current instant as the modelled timestamp shapes carry it.
pub fn now_as_wire_timestamp() -> ::aws_smithy_types::DateTime {
    let seconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    ::aws_smithy_types::DateTime::from_secs_f64(seconds)
}

/// `value` in base 36 (digits, then uppercase letters), left-padded with `0` to
/// `width` characters — the alphabet RDS's server-assigned ids are drawn from.
fn base36(value: u64, width: usize) -> String {
    const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut out = Vec::with_capacity(width);
    let mut remaining = value;
    loop {
        out.push(DIGITS[(remaining % 36) as usize]);
        remaining /= 36;
        if remaining == 0 {
            break;
        }
    }
    while out.len() < width {
        out.push(b'0');
    }
    out.reverse();
    String::from_utf8(out).expect("base-36 digits are ASCII")
}

/// The immutable resource id RDS reports as `DbiResourceId`: `db-` plus 26
/// uppercase characters, minted once per instance and never reused.
pub fn mint_dbi_resource_id(mint: u64) -> String {
    format!("db-{}", base36(mint, 26))
}

/// The DNS label RDS assigns an instance's endpoint between the identifier and the
/// region: a server-minted, 12-character token, unique per instance.
pub fn mint_endpoint_host(mint: u64) -> String {
    base36(mint, 12).to_ascii_lowercase()
}

/// The immutable resource id RDS reports as `GlobalClusterResourceId`: the
/// "partition-unique, immutable identifier for the global database cluster", written
/// `cluster-` plus 26 characters of the same server-minted alphabet
/// [`mint_dbi_resource_id`] draws on, and never reused.
pub fn mint_global_cluster_resource_id(mint: u64) -> String {
    format!("cluster-{}", base36(mint, 26))
}

/// The immutable resource id RDS reports as `DbClusterResourceId`: "the Amazon Web
/// Services Region-unique, immutable identifier for the DB cluster", of the same
/// `cluster-` plus 26 server-minted characters form as
/// [`mint_global_cluster_resource_id`]. Both draw on the one store-wide counter, so no two
/// resources of this service can be minted the same id.
pub fn mint_db_cluster_resource_id(mint: u64) -> String {
    format!("cluster-{}", base36(mint, 26))
}

/// The Route 53 hosted zone id RDS reports for the endpoints in one region, when it
/// is a region this emulator can cite one for.
///
/// RDS publishes ONE zone per region, so this is a function of the region alone. The
/// published ids are values AWS ALLOCATED — nothing computes them — so the only honest
/// source is a table, and a region the table does not carry gets no `HostedZoneId` at
/// all rather than a made-up value dressed in the published form: a client that reads
/// this member to build a DNS alias would be misled by an id no zone answers to.
/// Extend the table as published ids are cited.
pub fn hosted_zone_id(region: &str) -> Option<&'static str> {
    /// The published RDS hosted zone ids this emulator carries, as
    /// `(region, hosted zone id)`.
    const PUBLISHED: &[(&str, &str)] = &[("us-east-1", "Z2R2ITUGPM61AM")];

    PUBLISHED
        .iter()
        .find(|(name, _)| *name == region)
        .map(|(_, published)| *published)
}

/// The port a DB instance accepts connections on when the request names none.
///
/// From `CreateDBInstanceMessage$Port`: "Default: RDS for Db2 - 50000, RDS for
/// MariaDB - 3306, RDS for Microsoft SQL Server - 1433, RDS for MySQL - 3306, RDS
/// for Oracle - 1521, RDS for PostgreSQL - 5432". The engine names are the prefixes
/// the `Engine` member's own value list uses (`custom-sqlserver-ee`, `oracle-se2`,
/// `aurora-postgresql`, …), so the family is matched on the prefix.
pub fn default_port(engine: &str) -> i32 {
    let engine = engine.to_ascii_lowercase();
    if engine.contains("db2") {
        50000
    } else if engine.contains("sqlserver") {
        1433
    } else if engine.contains("oracle") {
        1521
    } else if engine.contains("postgres") {
        5432
    } else {
        // mysql, mariadb, aurora-mysql and the Aurora MySQL aliases.
        3306
    }
}

/// The CA certificate an instance's server certificate is issued under when the
/// request names none, and so the system default `DescribeCertificates` reports as
/// `DefaultCertificateForNewLaunches` where no override is set.
///
/// `CACertificateIdentifier` documents no default, so the value comes from what AWS
/// publishes: `rds-ca-rsa2048-g1` is the default certificate authority for a new DB
/// instance ("Using SSL/TLS to encrypt a connection to a DB instance", Amazon RDS User
/// Guide). It is a value AWS allocated — nothing computes it — so a table of one is
/// the only honest source; extend it when the published default moves.
///
/// An account can pin a different one per Region with `ModifyCertificates`, which is
/// what [`ComAmazonawsRdsAmazonrdsv19State::default_ca_certificate`] answers from: reach
/// for that where a request is being served, and for this constant only as the default
/// it falls back to.
pub const DEFAULT_CA_CERTIFICATE_IDENTIFIER: &str = "rds-ca-rsa2048-g1";

/// How long the server certificate issued at creation stays valid, which is what
/// `CertificateDetails$ValidTill` ("The expiration date of the DB instance's server
/// certificate") reports.
///
/// Neither the model nor the RDS API documents a lifetime for the issued certificate,
/// so this span is this emulator's choice: one year from the instant the instance was
/// created. It is a function of the stored creation time, so it never moves between
/// two reads of the same instance.
pub const SERVER_CERTIFICATE_VALIDITY: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// The daily backup window an instance gets when the request names none.
///
/// `PreferredBackupWindow`: "The default is a 30-minute window selected at random from
/// an 8-hour block of time for each Amazon Web Services Region." This emulator picks
/// ONE window rather than a random one, so two reads of an instance report the same
/// window; it keeps the documented `hh24:mi-hh24:mi` UTC format and the documented
/// 30-minute minimum.
pub const DEFAULT_PREFERRED_BACKUP_WINDOW: &str = "07:00-07:30";

/// The weekly maintenance window an instance gets when the request names none.
///
/// `PreferredMaintenanceWindow`: "The default is a 30-minute window selected at random
/// from an 8-hour block of time for each Amazon Web Services Region, occurring on a
/// random day of the week" — again one fixed window here, in the documented
/// `ddd:hh24:mi-ddd:hh24:mi` form, on one of the documented day values, at the
/// documented 30-minute minimum, and "not conflict[ing] with the preferred backup
/// window" ([`DEFAULT_PREFERRED_BACKUP_WINDOW`] ends half an hour before it starts).
pub const DEFAULT_PREFERRED_MAINTENANCE_WINDOW: &str = "mon:08:00-mon:08:30";

/// The Database Insights mode an instance is created in when the request names none:
/// the `standard` member of `DatabaseInsightsMode` (`standard | advanced`), advanced
/// being the mode a request asks for.
pub const DEFAULT_DATABASE_INSIGHTS_MODE: &str = "standard";

/// The network type an instance is created with when the request names none.
///
/// `NetworkType`: "Valid Values: `IPV4 | DUAL`" — a subnet group "can support only the
/// IPv4 protocol or the IPv4 and the IPv6 protocols (`DUAL`)", so IPv4 only unless the
/// request asks for both.
pub const DEFAULT_NETWORK_TYPE: &str = "IPV4";

/// The network types a resource of this service may be created with, as
/// `CreateDBClusterMessage$NetworkType` publishes them ("Valid Values: IPV4 | DUAL"), the
/// first being the default [`DEFAULT_NETWORK_TYPE`].
pub const NETWORK_TYPES: [&str; 2] = [DEFAULT_NETWORK_TYPE, "DUAL"];

/// How many entries one of the lists `CreateDBCluster` STORES may carry
/// (`AvailabilityZones`, `EnableCloudwatchLogsExports`).
///
/// Emulator resource guard, not an AWS quota: neither member publishes a bound and both
/// are stored, so the growth site carries a cap — the same reason [`MAX_FILTER_TERMS`]
/// caps a describe's filter terms. Generous next to the three Availability Zones an Aurora
/// cluster spans and the handful of log types each engine publishes.
pub const MAX_CLUSTER_LIST_ENTRIES: usize = 32;

/// Where automated backups and manual snapshots are stored when the request names no
/// `BackupTarget`: "Valid Values: local (Dedicated Local Zone) outposts (Amazon Web
/// Services Outposts) region (Amazon Web Services Region) Default: region".
pub const DEFAULT_BACKUP_TARGET: &str = "region";

/// The lifecycle type an instance is created with when the request names none:
/// `EngineLifecycleSupport` "Default: `open-source-rds-extended-support`".
pub const DEFAULT_ENGINE_LIFECYCLE_SUPPORT: &str = "open-source-rds-extended-support";

/// The Enhanced Monitoring interval an instance is created with when the request names
/// none: `MonitoringInterval` "Valid Values: 0 | 1 | 5 | 10 | 15 | 30 | 60. Default:
/// 0" — zero disables collection.
pub const DEFAULT_MONITORING_INTERVAL: i32 = 0;

/// The baseline storage throughput, in MiBps, of the `gp3` storage type — the only
/// type `StorageThroughput` "applies only to" ("General Purpose SSD gp3 storage:
/// baseline storage throughput of 125 MiBps", Amazon RDS storage types, Amazon RDS
/// User Guide).
pub const GP3_BASELINE_STORAGE_THROUGHPUT: i32 = 125;

/// The activity-stream status of an instance no activity stream was started for: the
/// `stopped` member of `ActivityStreamStatus` (`stopped | starting | started |
/// stopping`). Nothing here starts one — `StartActivityStream` is still a stub — so
/// this is the status every instance this service holds reports.
pub const ACTIVITY_STREAM_STOPPED: &str = "stopped";

/// The upgrade rollout order of an instance nothing configured one for:
/// `UpgradeRolloutOrder` "[second] - Default order for resources not specifically
/// configured". No operation this service implements configures it.
pub const DEFAULT_UPGRADE_ROLLOUT_ORDER: &str = "second";

/// The status RDS reports for a group membership that is in effect — the
/// `ParameterApplyStatus` of an applied DB parameter group and the `Status` of an
/// applied option group.
const MEMBERSHIP_IN_SYNC: &str = "in-sync";

/// The status RDS reports for a security-group membership that is in effect.
const MEMBERSHIP_ACTIVE: &str = "active";

/// The lowest and highest port `CreateDBInstanceMessage$Port` accepts
/// ("Valid Values: 1150-65535").
pub const PORT_RANGE: std::ops::RangeInclusive<i32> = 1150..=65535;

/// The lowest and highest `BackupRetentionPeriod`
/// ("Constraints: Must be a value from 0 to 35").
pub const BACKUP_RETENTION_RANGE: std::ops::RangeInclusive<i32> = 0..=35;

/// The lowest and highest `MaxRecords` a describe operation accepts
/// ("Constraints: Minimum 20, maximum 100").
pub const MAX_RECORDS_RANGE: std::ops::RangeInclusive<i32> = 20..=100;

/// The page size a describe operation uses when the request names none
/// (`MaxRecords` "Default: 100").
pub const DEFAULT_MAX_RECORDS: usize = 100;

/// Whether `engine` names an Aurora engine, which several documented defaults
/// branch on (`aurora-mysql`, `aurora-postgresql`).
pub fn is_aurora(engine: &str) -> bool {
    engine.to_ascii_lowercase().starts_with("aurora")
}

/// The documented default for `PubliclyAccessible` when the request omits it.
///
/// From `CreateDBInstanceMessage$PubliclyAccessible`: "If `DBSubnetGroup` isn't
/// specified, `PubliclyAccessible` defaults to `false` for Aurora instances and
/// `true` for non-Aurora instances. If `DBSubnetGroup` is specified,
/// `PubliclyAccessible` defaults to `false` unless the value of `DBSubnetGroup` is
/// `default`, in which case `PubliclyAccessible` defaults to `true`."
pub fn default_publicly_accessible(engine: &str, db_subnet_group_name: Option<&str>) -> bool {
    match db_subnet_group_name {
        None => !is_aurora(engine),
        Some(name) => name.eq_ignore_ascii_case("default"),
    }
}

/// The documented default for `StorageType` when the request omits it.
///
/// From `CreateDBInstanceMessage$StorageType`: "Default: `io1`, if the `Iops`
/// parameter is specified. Otherwise, `gp3`."
pub fn default_storage_type(iops: Option<i32>) -> &'static str {
    if iops.is_some() {
        "io1"
    } else {
        "gp3"
    }
}

/// The storage throughput a DB instance is provisioned with when the request names
/// none.
///
/// `StorageThroughput` "applies only to the `gp3` storage type", whose baseline is
/// [`GP3_BASELINE_STORAGE_THROUGHPUT`]; an instance on any other storage type is
/// provisioned no separate throughput, which RDS reports as zero rather than as an
/// absent member.
pub fn default_storage_throughput(storage_type: &str) -> i32 {
    if storage_type.eq_ignore_ascii_case("gp3") {
        GP3_BASELINE_STORAGE_THROUGHPUT
    } else {
        0
    }
}

/// The license model of an instance the request named none for, or `None` for an engine
/// whose license model this emulator cannot cite one for.
///
/// From the value list `CreateDBInstanceMessage$LicenseModel` publishes: "RDS for Db2 -
/// bring-your-own-license | marketplace-license" ("The default for RDS for Db2 is
/// bring-your-own-license"), "RDS for MariaDB - general-public-license", "RDS for
/// Microsoft SQL Server - license-included | bring-your-own-media", "RDS for MySQL -
/// general-public-license", "RDS for Oracle - bring-your-own-license |
/// license-included", "RDS for PostgreSQL - postgresql-license". Where the list offers
/// one value it IS the default; where it offers two, the default is the one that engine
/// can be created with unprepared — a Db2 or Oracle Enterprise Edition instance runs on
/// a license you bring, while SQL Server and Oracle SE2 include one. The member
/// "doesn't apply to Amazon Aurora", whose instances RDS reports with the license model
/// of the engine family they run (`aurora-mysql` the MySQL one, `aurora-postgresql` the
/// PostgreSQL one), so the match is on the engine family rather than on the exact
/// engine name.
pub fn default_license_model(engine: &str) -> Option<&'static str> {
    let engine = engine.to_ascii_lowercase();
    if engine.contains("db2") {
        Some("bring-your-own-license")
    } else if engine.contains("sqlserver") || engine.contains("oracle-se") {
        Some("license-included")
    } else if engine.contains("oracle") {
        Some("bring-your-own-license")
    } else if engine.contains("postgres") {
        Some("postgresql-license")
    } else if engine.contains("mysql") || engine.contains("mariadb") {
        Some("general-public-license")
    } else {
        None
    }
}

/// The name of the default DB parameter group of one engine-version family.
///
/// `DBParameterGroupName`: "If you don't specify a value, then Amazon RDS uses the
/// default DB parameter group for the specified DB engine and version" — the group RDS
/// names `default.` followed by that version's `DBParameterGroupFamily`
/// (`default.mysql8.0`).
pub fn default_db_parameter_group_name(parameter_group_family: &str) -> String {
    format!("default.{parameter_group_family}")
}

/// The name of the default option group of one engine and major engine version.
///
/// RDS creates a default option group per engine and major version and names it
/// `default:<engine>-<major engine version>`, writing the version's dots as hyphens
/// (`default:mysql-8-0`, `default:sqlserver-ee-15-00`) — the naming the "Working with
/// option groups" section of the Amazon RDS User Guide publishes.
pub fn default_option_group_name(engine: &str, major_engine_version: &str) -> String {
    format!(
        "default:{}-{}",
        engine.to_ascii_lowercase(),
        major_engine_version.replace('.', "-")
    )
}

/// Refuse an identifier that breaks the constraints its member documents.
///
/// `DBInstanceIdentifier` and `DBSnapshotIdentifier` share one rule, differing only
/// in the length bound (63 and 255): "Must contain from 1 to N letters, numbers, or
/// hyphens. First character must be a letter. Can't end with a hyphen or contain two
/// consecutive hyphens." Returns the message to refuse the request with, naming the
/// member and the constraint it broke.
pub fn check_identifier(member: &str, value: &str, max_len: usize) -> Result<(), String> {
    // The alphabet comes first: a value outside it is reported as the wrong alphabet
    // rather than measured against the length bound, which for a multi-byte character
    // would name a count the caller never typed.
    if !value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(format!(
            "{member} must contain only letters, numbers, or hyphens; got '{value}'"
        ));
    }
    let length = value.chars().count();
    if length == 0 || length > max_len {
        return Err(format!(
            "{member} must contain from 1 to {max_len} letters, numbers, or hyphens; \
             got {length} characters"
        ));
    }
    if !value
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
    {
        return Err(format!(
            "the first character of {member} must be a letter; got '{value}'"
        ));
    }
    if value.ends_with('-') {
        return Err(format!("{member} can't end with a hyphen; got '{value}'"));
    }
    if value.contains("--") {
        return Err(format!(
            "{member} can't contain two consecutive hyphens; got '{value}'"
        ));
    }
    Ok(())
}

/// Refuse a `MaxRecords` outside the range every describe operation documents.
pub fn check_max_records(max_records: Option<i32>) -> Result<usize, String> {
    check_max_records_in(max_records, &MAX_RECORDS_RANGE)
}

/// Refuse a `MaxRecords` outside `range`, for the operations whose own member documents
/// a different bound than [`MAX_RECORDS_RANGE`] — `DescribeOrderableDBInstanceOptions`
/// documents "Minimum 20, maximum 1000". The default page size and the wording of the
/// refusal stay here, shared by every describe operation.
pub fn check_max_records_in(
    max_records: Option<i32>,
    range: &std::ops::RangeInclusive<i32>,
) -> Result<usize, String> {
    match max_records {
        None => Ok(DEFAULT_MAX_RECORDS),
        Some(value) if range.contains(&value) => Ok(value as usize),
        Some(value) => Err(format!(
            "invalid value {value} for MaxRecords: must be a value from {} to {}",
            range.start(),
            range.end()
        )),
    }
}

/// Refuse a describe request carrying more filter values than [`MAX_FILTER_TERMS`].
///
/// The scan is the product of the stored record count and the filter terms, and it
/// runs with this service's one state lock held; the record count is capped by the
/// per-region quotas, and this caps the other factor.
pub fn check_filter_terms(filters: &[crate::ops::types::Filter]) -> Result<(), String> {
    let terms: usize = filters.iter().map(|filter| filter.values.len()).sum();
    if terms > MAX_FILTER_TERMS {
        return Err(format!(
            "Filters carries {terms} values: this emulator serves at most {MAX_FILTER_TERMS}"
        ));
    }
    Ok(())
}

/// How many security group memberships one option of an option group may carry.
///
/// `OptionConfiguration$DBSecurityGroupMemberships` and
/// `OptionConfiguration$VpcSecurityGroupMemberships` publish no bound, and their values
/// are STORED, so they are capped for the same reason as [`MAX_FILTER_TERMS`]: request
/// data that enters the store has to be bounded. Generous next to the five security
/// groups AWS lets one DB instance carry.
pub const MAX_OPTION_SECURITY_GROUPS: usize = 32;

/// How long a text value a `ModifyOptionGroup` request may store: the value of an
/// option setting, or an `OptionVersion`. Neither member publishes a bound and both are
/// stored, so this emulator caps them.
pub const MAX_OPTION_TEXT_LEN: usize = 1024;

/// Refuse stored request text longer than [`MAX_OPTION_TEXT_LEN`], naming the member
/// and the bound it broke.
pub fn check_option_text(member: &str, value: &str) -> Result<(), String> {
    let length = value.chars().count();
    if length > MAX_OPTION_TEXT_LEN {
        return Err(format!(
            "{member} carries {length} characters: this emulator stores at most \
             {MAX_OPTION_TEXT_LEN}"
        ));
    }
    Ok(())
}

/// Refuse a security group membership list longer than [`MAX_OPTION_SECURITY_GROUPS`],
/// and any entry that is not of the form a security group name or id takes.
pub fn check_security_group_memberships(member: &str, values: &[String]) -> Result<(), String> {
    if values.len() > MAX_OPTION_SECURITY_GROUPS {
        return Err(format!(
            "{member} carries {} entries: this emulator stores at most \
             {MAX_OPTION_SECURITY_GROUPS}",
            values.len()
        ));
    }
    for value in values {
        // A DB security group name and a VPC security group id are both drawn from the
        // letters/numbers/hyphens alphabet the identifier members share.
        check_identifier(member, value, 255)?;
    }
    Ok(())
}

/// How many entries one `ModifyOptionGroup` request may name in `OptionsToInclude`,
/// `OptionsToRemove`, or in one option configuration's `OptionSettings`.
///
/// None of those members publishes a bound, and the work a request costs is the PRODUCT
/// of its option count and its settings count, so each factor is capped BEFORE any
/// per-entry work runs — the same reason [`MAX_FILTER_TERMS`] caps a describe's filter
/// terms. The bound is generous: the whole catalogue publishes fewer options than this
/// for every engine together, and no option publishes more settings than this, so a
/// request past it is asking for options and settings that do not exist.
pub const MAX_OPTIONS_PER_REQUEST: usize = 64;

/// Refuse an option or option-settings list longer than [`MAX_OPTIONS_PER_REQUEST`],
/// naming the member and the bound it broke.
pub fn check_option_list_len(member: &str, len: usize) -> Result<(), String> {
    if len > MAX_OPTIONS_PER_REQUEST {
        return Err(format!(
            "{member} carries {len} entries: this emulator serves at most \
             {MAX_OPTIONS_PER_REQUEST} per request"
        ));
    }
    Ok(())
}

/// One page of an ordered result set, plus the `Marker` to resume from.
///
/// The marker is the key the next page starts AT, so resuming is a single ordered
/// lookup and a page boundary cannot drift when a record is added or removed between
/// calls. A marker is returned only while records remain, which is what `@paginated`
/// callers loop on.
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_marker: Option<String>,
}

/// Take the page of `records` that starts at `marker` and holds at most
/// `page_size` items. `records` must be ordered by its key.
///
/// A `marker` is only ever a key of the result set — that is what this function
/// returns as `next_marker` — so a marker naming no key of `records` is one no prior
/// page can have issued. It is REFUSED (the returned message is what the caller
/// refuses the request with) rather than resynchronised to the nearest key: a
/// paginator that silently restarts on a cursor the service never handed out reports a
/// page the caller cannot place, and the real service answers an invalid-parameter
/// error instead.
pub fn paginate<T>(
    records: Vec<(String, T)>,
    marker: Option<&str>,
    page_size: usize,
) -> Result<Page<T>, String> {
    let start = match marker {
        None => 0,
        Some(marker) => records
            .iter()
            .position(|(key, _)| key.as_str() == marker)
            .ok_or_else(|| {
                format!(
                    "invalid value '{marker}' for Marker: it was not returned by a prior \
                     request; omit Marker to start at the first page"
                )
            })?,
    };
    let end = start.saturating_add(page_size).min(records.len());
    let next_marker = records.get(end).map(|(key, _)| key.clone());
    let items = records
        .into_iter()
        .skip(start)
        .take(end.saturating_sub(start))
        .map(|(_, record)| record)
        .collect();
    Ok(Page { items, next_marker })
}

// ---------------------------------------------------------------------------
// The reference catalogs this front door reports
//
// `DescribeCertificates` and `DescribeEventCategories` report what the service
// PUBLISHES rather than what a caller created, so their source is a catalog rather than
// the store above. Each catalog below names where its values come from, and every
// handler projects them through one of the `to_wire` functions here, so no two handlers
// can disagree about a CA certificate or an event category. (The DB engine version
// catalogue is the same kind of reference data and lives in
// `crate::ops::describe_db_engine_versions`, which owns it and lends it to
// `DescribeOrderableDBInstanceOptions`.)
//
// This front door serves the RDS, Amazon DocumentDB and Amazon Neptune control planes —
// the three models that share its endpoint prefix, signing name, protocol and API
// version (see `src/service.rs`) — so the catalogs cover all three, as AWS's own `rds`
// endpoint does.
// ---------------------------------------------------------------------------

/// One CA certificate the service publishes for a DB instance's TLS endpoint.
///
/// Source: the certificate authorities Amazon RDS publishes in "Using SSL/TLS to
/// encrypt a connection to a DB instance" (Amazon RDS User Guide) and reports through
/// `DescribeCertificates`. AWS publishes a validity DATE for each, not a time of day, so
/// the instants below are that date at midnight UTC. `Thumbprint` is deliberately absent
/// rather than invented: it is the fingerprint of a certificate this emulator does not
/// hold.
#[derive(Debug, Clone, Copy)]
pub struct CaCertificate {
    /// `Certificate$CertificateIdentifier` — "The unique key that identifies a
    /// certificate."
    pub identifier: &'static str,
    /// `Certificate$CertificateType` — the type AWS reports for every published CA.
    pub certificate_type: &'static str,
    /// `Certificate$ValidFrom`, as an ISO-8601 instant.
    pub valid_from: &'static str,
    /// `Certificate$ValidTill`, as an ISO-8601 instant.
    pub valid_till: &'static str,
}

impl CaCertificate {
    /// This catalog entry as the modelled `Certificate`, with the ARN scoped to the
    /// region the request was signed for and the override members answered from
    /// `override_identifier` — the `CertificateIdentifier` the account's
    /// `ModifyCertificates` override names in that region, or `None` where it set none.
    pub fn to_wire(&self, region: &str, override_identifier: Option<&str>) -> Certificate {
        // "Indicates whether there is an override for the default certificate
        // identifier": true for the ONE certificate the override pins, false for every
        // other published certificate. The comparison is case-insensitive because
        // `ModifyCertificates`/`DescribeCertificates` accept the identifier in any case.
        let overridden =
            override_identifier.is_some_and(|pinned| pinned.eq_ignore_ascii_case(self.identifier));
        Certificate {
            certificate_identifier: Some(self.identifier.to_string()),
            certificate_type: Some(self.certificate_type.to_string()),
            thumbprint: None,
            valid_from: catalog_instant(self.valid_from),
            valid_till: catalog_instant(self.valid_till),
            certificate_arn: Some(ca_certificate_arn(region, self.identifier)),
            customer_override: Some(overridden),
            // `CustomerOverrideValidTill`: "If there is an override for the default
            // certificate identifier, when the override expires." The override pins THIS
            // certificate, so it cannot outlive it: the override expires when the
            // certificate it names does, and a certificate under no override reports no
            // expiry at all.
            customer_override_valid_till: overridden
                .then(|| catalog_instant(self.valid_till))
                .flatten(),
        }
    }
}

/// The ARN of a published CA certificate: `arn:aws:rds:<region>::cert:<identifier>`.
///
/// The account field is EMPTY, unlike every other ARN this service builds (through
/// `emulator_server::emulator_arn`, which stamps the emulator's account id): a CA
/// certificate is published BY Amazon RDS rather than created by an account, so there is
/// no account that owns it to name — the same shape AWS gives the certificate ARNs
/// `DescribeCertificates` reports. It is still region-scoped, because the certificate
/// authority a DB instance is launched under is reported per Region.
pub fn ca_certificate_arn(region: &str, identifier: &str) -> String {
    format!("arn:aws:{ARN_NAMESPACE}:{region}::cert:{identifier}")
}

/// The published CA certificate `identifier` names, or `None` when no published
/// certificate carries it.
///
/// `DescribeCertificates$CertificateIdentifier`: "This parameter isn't case-sensitive",
/// so the match ignores case — and `ModifyCertificates` resolves the identifier it is
/// given through this same lookup, so the two operations cannot disagree about which
/// identifiers exist.
pub fn ca_certificate(identifier: &str) -> Option<&'static CaCertificate> {
    CA_CERTIFICATES
        .iter()
        .find(|certificate| certificate.identifier.eq_ignore_ascii_case(identifier))
}

/// Every CA certificate `DescribeCertificates` reports. See [`CaCertificate`] for the
/// source; the expired `rds-ca-2019` is included because AWS still reports it.
pub const CA_CERTIFICATES: &[CaCertificate] = &[
    CaCertificate {
        identifier: "rds-ca-2019",
        certificate_type: "CA",
        valid_from: "2019-08-22T00:00:00Z",
        valid_till: "2024-08-22T00:00:00Z",
    },
    CaCertificate {
        identifier: "rds-ca-ecc384-g1",
        certificate_type: "CA",
        valid_from: "2021-05-25T00:00:00Z",
        valid_till: "2121-05-25T00:00:00Z",
    },
    CaCertificate {
        identifier: "rds-ca-rsa2048-g1",
        certificate_type: "CA",
        valid_from: "2021-05-25T00:00:00Z",
        valid_till: "2061-05-25T00:00:00Z",
    },
    CaCertificate {
        identifier: "rds-ca-rsa4096-g1",
        certificate_type: "CA",
        valid_from: "2021-05-25T00:00:00Z",
        valid_till: "2121-05-25T00:00:00Z",
    },
];

/// The CA certificate identifiers an engine version can be launched with:
/// `DBEngineVersion$SupportedCACertificateIdentifiers`, "a list of the supported CA
/// certificate identifiers".
///
/// Read from the ONE certificate catalogue this service reports through
/// `DescribeCertificates` ([`CA_CERTIFICATES`]), so the two reports cannot name different
/// certificates — minus any entry whose `ValidTill` has passed: a caller picks the
/// `CACertificateIdentifier` of a new instance out of this list, and an expired
/// certificate is not something it can be launched with. The catalogue is ordered, so the
/// list is too.
pub fn supported_ca_certificate_identifiers() -> Vec<String> {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_secs() as i64);
    CA_CERTIFICATES
        .iter()
        .filter(|certificate| {
            catalog_instant(certificate.valid_till).is_some_and(|till| till.secs() > now)
        })
        .map(|certificate| certificate.identifier.to_string())
        .collect()
}

/// One of the catalogs' fixed ISO-8601 instants as the modelled timestamp shapes carry
/// it.
///
/// The input is a literal in this file, never caller data, so a value that is not an
/// instant is a typo in the catalog above: it panics at the first read rather than
/// silently dropping `ValidFrom`/`ValidTill` from the response.
fn catalog_instant(iso: &str) -> Option<::aws_smithy_types::DateTime> {
    Some(
        ::aws_smithy_types::DateTime::from_str(iso, ::aws_smithy_types::date_time::Format::DateTime)
            .expect("every catalog instant in this file is an ISO-8601 literal"),
    )
}

/// The event source types and their categories, as `DescribeEventCategories` reports
/// them.
///
/// The source types are the valid values documented on
/// `DescribeEventCategoriesMessage$SourceType` ("`db-instance` | `db-cluster` |
/// `db-parameter-group` | `db-security-group` | `db-snapshot` | `db-cluster-snapshot` |
/// `db-proxy`"), in that order; the categories of each are the ones published in "Amazon
/// RDS event categories and event messages" (Amazon RDS User Guide), in the order that
/// table lists them (alphabetical): the wire order of `EventCategories` is not documented
/// anywhere in the model or the API reference, so the ONE order this catalog can state a
/// source for is the published table's.
///
/// `db-instance` carries the two categories that table lists beyond the ones a DB
/// instance shares with the other source types — `backtrack` (an Aurora MySQL backtrack
/// event) and `security` — because a client builds a `CreateEventSubscription`
/// `EventCategories` argument out of this map and AWS accepts both for that source type;
/// `db-cluster-snapshot` carries `notification` beside `backup` for the same reason.
///
/// The model's `SourceType` enum (`smithy_query(query="shape",shape="SourceType")`)
/// carries four further values — `custom-engine-version`, `blue-green-deployment`,
/// `db-shard-group` and `zero-etl` — which this member's own documentation does not list
/// and whose published categories this emulator does not have. It publishes no map for
/// them rather than an invented one, and a request naming one is refused with the code a
/// value this parameter does not accept gets, naming the set it is not in.
pub const EVENT_CATEGORIES: &[(&str, &[&str])] = &[
    (
        "db-instance",
        &[
            "availability",
            "backtrack",
            "backup",
            "configuration change",
            "creation",
            "deletion",
            "failover",
            "failure",
            "low storage",
            "maintenance",
            "notification",
            "read replica",
            "recovery",
            "restoration",
            "security",
            "security patching",
        ],
    ),
    (
        "db-cluster",
        &[
            "configuration change",
            "creation",
            "deletion",
            "failover",
            "failure",
            "global-failover",
            "maintenance",
            "notification",
        ],
    ),
    ("db-parameter-group", &["configuration change"]),
    ("db-security-group", &["configuration change", "failure"]),
    (
        "db-snapshot",
        &["creation", "deletion", "notification", "restoration"],
    ),
    ("db-cluster-snapshot", &["backup", "notification"]),
    (
        "db-proxy",
        &["configuration change", "creation", "deletion", "failure"],
    ),
];

/// The event categories of `source_type`, or `None` when it names no published source
/// type. Matching is exact: the documented values are all lowercase.
pub fn event_categories_of(source_type: &str) -> Option<&'static [&'static str]> {
    EVENT_CATEGORIES
        .iter()
        .find(|(published, _)| *published == source_type)
        .map(|(_, categories)| *categories)
}

/// The published event source types, comma-separated — what a refusal names as the set a
/// rejected `SourceType` is not in.
pub fn event_source_types() -> String {
    EVENT_CATEGORIES
        .iter()
        .map(|(source_type, _)| *source_type)
        .collect::<Vec<&str>>()
        .join(", ")
}

/// One source type's categories as the modelled `EventCategoriesMap`.
pub fn event_categories_to_wire(source_type: &str, categories: &[&str]) -> EventCategoriesMap {
    EventCategoriesMap {
        source_type: Some(source_type.to_string()),
        event_categories: Some(
            categories
                .iter()
                .map(|category| (*category).to_string())
                .collect(),
        ),
    }
}

/// The source types an event notification SUBSCRIPTION may name, as
/// `CreateEventSubscriptionMessage$SourceType` publishes them ("Valid Values: db-instance
/// | db-cluster | db-parameter-group | db-security-group | db-snapshot |
/// db-cluster-snapshot | db-proxy | zero-etl | custom-engine-version |
/// blue-green-deployment").
///
/// Wider than the source types [`EVENT_CATEGORIES`] carries a category map for, which is
/// the narrower list `DescribeEventCategories`'s own member documents: a subscription may
/// be created for a source type whose published categories this emulator does not have,
/// and [`check_event_categories`] says what that means for the categories such a
/// subscription may name.
pub const EVENT_SUBSCRIPTION_SOURCE_TYPES: [&str; 10] = [
    "db-instance",
    "db-cluster",
    "db-parameter-group",
    "db-security-group",
    "db-snapshot",
    "db-cluster-snapshot",
    "db-proxy",
    "zero-etl",
    "custom-engine-version",
    "blue-green-deployment",
];

/// Refuse a `SourceType` a subscription cannot be created for, naming the member, the
/// value rejected and the set it is not in.
pub fn check_event_subscription_source_type(source_type: &str) -> Result<(), String> {
    if EVENT_SUBSCRIPTION_SOURCE_TYPES.contains(&source_type) {
        return Ok(());
    }
    Err(format!(
        "invalid value {source_type} for SourceType: must be one of {}",
        EVENT_SUBSCRIPTION_SOURCE_TYPES.join(", ")
    ))
}

/// The categories of `source_type` a subscription may subscribe to, with the one that
/// is not among them, if any: `EventCategories` is "a list of event categories for a
/// particular source type (SourceType) that you want to subscribe to", so a category
/// that source type does not publish names no events at all.
///
/// A source type [`EVENT_CATEGORIES`] carries no published map for is not checked —
/// there is nothing to check it against, and refusing every category for it would refuse
/// the ones AWS accepts. Matching is exact: the published categories are all lowercase.
pub fn check_event_categories(source_type: &str, categories: &[String]) -> Result<(), String> {
    let Some(published) = event_categories_of(source_type) else {
        return Ok(());
    };
    for category in categories {
        if !published.contains(&category.as_str()) {
            return Err(format!(
                "{category} is not a published event category of {source_type} \
                 (its categories are {})",
                published.join(", ")
            ));
        }
    }
    Ok(())
}

/// The longest `SubscriptionName` this service accepts: "Constraints: The name must be
/// less than 255 characters" (`CreateEventSubscriptionMessage$SubscriptionName`), which
/// the model itself does not express.
pub const MAX_SUBSCRIPTION_NAME_LEN: usize = 254;

/// How many `EventCategories` entries one request may carry.
///
/// The member publishes no bound and every entry is STORED, so it is capped for the same
/// reason as [`MAX_FILTER_TERMS`]: request data that enters the store has to be bounded.
/// Generous next to the fourteen categories the largest published source type has.
pub const MAX_EVENT_CATEGORIES_PER_REQUEST: usize = 32;

/// The longest `EventCategories` entry this service stores. The published categories are
/// short phrases — the longest, `security patching`, is 17 characters — and a source type
/// this emulator publishes no category map for cannot have its entries resolved, so their
/// length is what bounds them.
pub const MAX_EVENT_CATEGORY_LEN: usize = 64;

/// How many `SourceIds` entries one request may carry.
///
/// Each entry is resolved against the stored resources of its source type, so the scan is
/// the PRODUCT of this count and the per-region record quota, under this service's one
/// state lock: this caps the factor the caller chooses, exactly as [`check_filter_terms`]
/// does for a describe. Generous next to the forty DB instances a region may hold.
pub const MAX_SOURCE_IDS_PER_REQUEST: usize = 64;

/// How many tags one resource may carry: Amazon RDS's published tag quota ("Tags per
/// resource": 50). Tags are stored request data, so the bound is enforced rather than
/// documented.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The longest tag key AWS accepts: "Tag keys ... maximum length: 128 Unicode characters"
/// (Tagging AWS resources).
pub const MAX_TAG_KEY_LEN: usize = 128;

/// The longest tag value AWS accepts: "maximum length: 256 Unicode characters".
pub const MAX_TAG_VALUE_LEN: usize = 256;

/// Refuse a request list longer than `max`, naming the member, the count and the bound it
/// broke — for the members that publish no bound of their own and whose entries are
/// stored or scanned.
pub fn check_request_list_len(member: &str, len: usize, max: usize) -> Result<(), String> {
    if len > max {
        return Err(format!(
            "{member} carries {len} entries: this emulator serves at most {max}"
        ));
    }
    Ok(())
}

/// Refuse stored request text longer than `max`, naming the member and the bound it broke.
pub fn check_stored_text(member: &str, value: &str, max: usize) -> Result<(), String> {
    let length = value.chars().count();
    if length > max {
        return Err(format!(
            "{member} carries {length} characters: this emulator stores at most {max}"
        ));
    }
    Ok(())
}

/// Refuse a `Tags` list this service will not store: more tags than one resource may
/// carry, or a key or value past AWS's published tag lengths. Every tag a create stores is
/// request data, so all three bounds are checked before the store is touched.
pub fn check_tags(tags: &[Tag]) -> Result<(), String> {
    check_request_list_len("Tags", tags.len(), MAX_TAGS_PER_RESOURCE)?;
    for tag in tags {
        if let Some(key) = &tag.key {
            check_stored_text("Tags.Key", key, MAX_TAG_KEY_LEN)?;
        }
        if let Some(value) = &tag.value {
            check_stored_text("Tags.Value", value, MAX_TAG_VALUE_LEN)?;
        }
    }
    Ok(())
}

/// Refuse a `SubscriptionName` outside the constraint its member documents, naming the
/// member and the bound it broke.
pub fn check_subscription_name(name: &str) -> Result<(), String> {
    let length = name.chars().count();
    if length == 0 || length > MAX_SUBSCRIPTION_NAME_LEN {
        return Err(format!(
            "SubscriptionName must be less than {} characters; got {length}",
            MAX_SUBSCRIPTION_NAME_LEN + 1
        ));
    }
    Ok(())
}

/// Refuse an `SnsTopicArn` that is not an Amazon SNS topic ARN:
/// `arn:<partition>:sns:<region>:<account>:<topic name>`, the form
/// "`SnsTopicArn` — The Amazon Resource Name (ARN) of the SNS topic created for event
/// notification" takes.
///
/// Only the FORM is checked. Whether a topic of that ARN exists is Amazon SNS's state,
/// which this service does not hold, so a well-formed ARN is accepted rather than
/// answered with the modelled `SNSTopicArnNotFoundFault` — this service cannot tell an
/// existing topic from a missing one, and claiming either would be a fabrication. A value
/// that is not an SNS topic ARN names no topic whatever SNS holds, which is what the
/// modelled `SNSInvalidTopicFault` says.
pub fn check_sns_topic_arn(arn: &str) -> Result<(), String> {
    let malformed = || {
        format!(
            "invalid value {arn} for SnsTopicArn: expected an Amazon SNS topic ARN of the \
             form arn:aws:sns:<region>:<account>:<topic name>"
        )
    };
    let fields: Vec<&str> = arn.split(':').collect();
    let [prefix, partition, service, region, account, topic] = fields[..] else {
        return Err(malformed());
    };
    if prefix != "arn"
        || partition.is_empty()
        || service != "sns"
        || account.is_empty()
        || !account.bytes().all(|b| b.is_ascii_digit())
        || topic.is_empty()
    {
        return Err(malformed());
    }
    check_region(region).map_err(|_| malformed())?;
    Ok(())
}

