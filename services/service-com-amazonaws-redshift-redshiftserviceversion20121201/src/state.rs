// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-redshift-redshiftserviceversion20121201 service.
//!
//! One store per emulator, holding the provisioned-cluster control plane: the
//! clusters, the snapshots taken of them, the cluster parameter groups and
//! cluster subnet groups they are configured from, the HSM configurations that
//! tell a cluster where to store its database encryption keys, the usage limits
//! set on their features, and the Query Editor (QEV2) IAM Identity Center
//! applications the account registered. Every collection keeps
//! INSERTION ORDER, so the `Marker`
//! cursor the paginated `Describe*` operations hand out is a stable, resumable
//! offset into the same sequence a later page walks.
//!
//! Besides the store this module owns the small rules every cluster operation
//! shares: the published engine-version catalog, the published parameter group
//! FAMILIES and the default group of each (see
//! [`published_parameter_group_families`]), the engine default PARAMETERS a
//! parameter group of such a family publishes (see
//! [`ENGINE_DEFAULT_PARAMETERS`]), the documented identifier syntax, the
//! documented `MaxRecords` window, the published maintenance-track catalog (see
//! [`published_maintenance_tracks`]), the published account attributes (see
//! [`published_account_attributes`]), every `Cluster` member this service DERIVES
//! from a cluster's own configuration (one rule, [`populate_derived_members`],
//! so the create, modify and read paths cannot report different member sets),
//! and the clock-owed `creating` -> `available` transitions (see
//! [`PROVISIONING_SPAN`]).

use crate::ops::types::{
    AccountAttribute, AccountWithRestoreAccess, AquaConfiguration, AttributeValueTarget,
    AvailabilityZone, Cluster, ClusterIamRole,
    ClusterNode, ClusterParameterGroup, ClusterParameterGroupStatus, ClusterParameterStatus,
    ClusterSubnetGroup, ClusterVersion, CreateUsageLimitOutput, HsmConfiguration, MaintenanceTrack,
    NetworkInterface, Parameter, PendingModifiedValues, Qev2IdcApplication, RecurringCharge,
    ReservedNodeOffering, Snapshot, Subnet, SupportedOperation, SupportedPlatform, UpdateTarget,
    VpcEndpoint, VpcSecurityGroupMembership,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant, SystemTime};

/// How long after the call that starts it a provisioning transition falls due:
/// a cluster or a snapshot reported `creating` becomes `available` this long
/// after the `CreateCluster` / `CreateClusterSnapshot` / `DeleteCluster` call
/// that minted it.
///
/// The span is the emulator's own choice — AWS documents no provisioning
/// duration — and is deliberately non-zero, so a caller that acts immediately
/// after the starting call still observes the transient `creating` state. The
/// due instant is stored and nothing ever sleeps for it.
pub const PROVISIONING_SPAN: Duration = Duration::from_millis(500);

/// The default `DBName` when `CreateCluster` does not carry one.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage", member="DBName")`
/// — "Default: dev".
pub const DEFAULT_DB_NAME: &str = "dev";

/// The only Amazon Redshift engine version the service offers.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage", member="ClusterVersion")`
/// — "Only version 1.0 is currently available. Example: 1.0".
pub const DEFAULT_CLUSTER_VERSION: &str = "1.0";

/// `ClusterType` for a cluster of two or more compute nodes.
pub const CLUSTER_TYPE_MULTI_NODE: &str = "multi-node";
/// `ClusterType` for a cluster that is its own single node.
pub const CLUSTER_TYPE_SINGLE_NODE: &str = "single-node";

/// Every `ClusterType` this service offers, in the documented order.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage",
/// member="ClusterType")` — "Valid Values: multi-node | single-node".
pub const ORDERABLE_CLUSTER_TYPES: [&str; 2] = [CLUSTER_TYPE_MULTI_NODE, CLUSTER_TYPE_SINGLE_NODE];

/// Every node type this service offers, in the documented order.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage",
/// member="NodeType")` — "Valid Values: dc2.large | dc2.8xlarge | rg.large |
/// rg.xlarge | rg.4xlarge | rg.12xlarge | ra3.large | ra3.xlplus | ra3.4xlarge |
/// ra3.16xlarge". The identical sentence appears on
/// `ModifyClusterMessage$NodeType`, so one list serves both.
pub const ORDERABLE_NODE_TYPES: [&str; 10] = [
    "dc2.large",
    "dc2.8xlarge",
    "rg.large",
    "rg.xlarge",
    "rg.4xlarge",
    "rg.12xlarge",
    "ra3.large",
    "ra3.xlplus",
    "ra3.4xlarge",
    "ra3.16xlarge",
];

/// The port the Redshift database engine listens on when the request names none.
pub const DEFAULT_PORT: i32 = 5439;

/// The cluster parameter group family an engine version belongs to.
///
/// A Redshift family is named `redshift-<engine version>`, which is why the
/// default parameter group of engine version 1.0 is `default.redshift-1.0`
/// (`smithy_query(query="doc", shape="CreateClusterMessage",
/// member="ClusterParameterGroupName")` — "Default: The default Amazon Redshift
/// cluster parameter group"): the default group OF the `redshift-1.0` family.
pub fn parameter_group_family(cluster_version: &str) -> String {
    format!("redshift-{cluster_version}")
}

/// The DEFAULT cluster parameter group OF one family: `default.` followed by the
/// family name, so the `redshift-1.0` family's default group is
/// `default.redshift-1.0`.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage",
/// member="ClusterParameterGroupName")` — "Default: The default Amazon Redshift
/// cluster parameter group".
pub fn family_default_parameter_group(family: &str) -> String {
    format!("default.{family}")
}

/// The default cluster parameter group of the family an engine version belongs
/// to, which is what `DescribeClusterVersions` pairs with that version.
pub fn default_parameter_group(cluster_version: &str) -> String {
    family_default_parameter_group(&parameter_group_family(cluster_version))
}

/// The cluster parameter group family whose DEFAULT group a cluster joins when
/// the request names none.
///
/// A cluster's parameter group family is a SERVICE-level default, not a value
/// derived from the engine version the cluster runs: Amazon Redshift reports a
/// freshly created provisioned cluster with `ClusterVersion` 1.0 and the
/// parameter group `default.redshift-2.0` on the same `Cluster`, so deriving the
/// family from the cluster's version would contradict the service. The model
/// bounds only what the family names may be —
/// `smithy_query(query="doc", shape="CreateClusterParameterGroupMessage",
/// member="ParameterGroupFamily")`: "The parameter group family names associated
/// with the default parameter groups provide you the valid values" — which is
/// exactly [`published_parameter_group_families`], the families this store seeds
/// a default group for.
pub const DEFAULT_PARAMETER_GROUP_FAMILY: &str = "redshift-2.0";

/// The parameter group a cluster joins when the request names none: the default
/// group of [`DEFAULT_PARAMETER_GROUP_FAMILY`].
pub fn cluster_default_parameter_group() -> String {
    family_default_parameter_group(DEFAULT_PARAMETER_GROUP_FAMILY)
}

/// Every cluster parameter group family this service publishes a DEFAULT group
/// for, in catalog order: the family of each published engine version, then the
/// family new clusters join when that is not already one of them.
///
/// This list IS the `ParameterGroupFamily` catalogue (see
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::parameter_group_families`]),
/// so a cluster can always look its own parameter group up and a
/// `CreateClusterParameterGroup` can always name the family a cluster reports.
pub fn published_parameter_group_families() -> Vec<String> {
    let mut families: Vec<String> = published_cluster_versions()
        .into_iter()
        .filter_map(|version| version.cluster_parameter_group_family)
        .collect();
    if !families
        .iter()
        .any(|family| family == DEFAULT_PARAMETER_GROUP_FAMILY)
    {
        families.push(DEFAULT_PARAMETER_GROUP_FAMILY.to_string());
    }
    families
}

/// Every engine version this service offers, paired with the engine BUILD that
/// version currently runs.
///
/// Source for the version list: `smithy_query(query="doc",
/// shape="CreateClusterMessage", member="ClusterVersion")` — "Only version 1.0
/// is currently available. Example: 1.0". Source for the build: Amazon Redshift
/// reports the engine build of a version as that version's
/// `ClusterVersion$Description` (`smithy_query(query="shape",
/// shape="ClusterVersion")` — "The description of the cluster version"), and the
/// build Amazon Redshift publishes for engine version 1.0 is `1.0.49795`. A
/// build is catalog data — the same for every account and stable across reads —
/// not a per-run value.
pub const PUBLISHED_ENGINE_BUILDS: [(&str, &str); 1] = [(DEFAULT_CLUSTER_VERSION, "1.0.49795")];

/// `ClusterVersion$Description` for an engine build: the Amazon Redshift engine
/// (ParAccel Analytic DataBase) build string, `PADB <build>`.
///
/// The member identifies WHICH ENGINE BUILD a version corresponds to, so it
/// carries the build rather than restating the version number the caller already
/// has in `ClusterVersion$ClusterVersion`.
pub fn engine_build_description(build: &str) -> String {
    format!("PADB {build}")
}

/// The Amazon Redshift engine versions this service offers, in the order
/// `DescribeClusterVersions` reports them.
///
/// Built from [`PUBLISHED_ENGINE_BUILDS`], so each entry carries the family
/// [`parameter_group_family`] gives its version and the build description
/// [`engine_build_description`] gives its build. Seeded into
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::cluster_versions`]
/// when the store is created, so the catalog a read reports and the
/// [`DEFAULT_CLUSTER_VERSION`] a `CreateCluster` applies cannot disagree.
pub fn published_cluster_versions() -> Vec<ClusterVersion> {
    PUBLISHED_ENGINE_BUILDS
        .into_iter()
        .map(|(version, build)| ClusterVersion {
            cluster_version: Some(version.to_string()),
            cluster_parameter_group_family: Some(parameter_group_family(version)),
            description: Some(engine_build_description(build)),
        })
        .collect()
}

/// The DEFAULT cluster parameter group of every published engine version: the
/// groups a fresh account already has, before it creates one of its own.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterParameterGroupMessage",
/// member="ParameterGroupFamily")` — "For example, a valid family name is
/// \"redshift-1.0\"" and "The parameter group family names associated with the
/// default parameter groups provide you the valid values": the families these
/// groups carry ARE the catalogue a `CreateClusterParameterGroup` request is
/// checked against (see
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::parameter_group_families`]).
/// Derived from [`published_parameter_group_families`], so the families
/// `CreateClusterParameterGroup` accepts, the family a cluster's default group
/// belongs to and the groups `DescribeClusterParameterGroups` reports cannot
/// disagree.
pub fn default_parameter_groups() -> Vec<StoredParameterGroup> {
    published_parameter_group_families()
        .into_iter()
        .map(|family| StoredParameterGroup {
            group: ClusterParameterGroup {
                parameter_group_name: Some(family_default_parameter_group(&family)),
                parameter_group_family: Some(family.clone()),
                description: Some(default_parameter_group_description(&family)),
                tags: Some(Vec::new()),
            },
            default_group: true,
            // A default group reports its family's engine defaults and
            // nothing else: nobody has changed a value on it.
            parameter_values: BTreeMap::new(),
        })
        .collect()
}

/// The description Redshift gives a default cluster parameter group.
pub fn default_parameter_group_description(family: &str) -> String {
    format!("Default parameter group for {family}")
}

/// `Parameter$Source` for a value nobody changed: the parameter still carries
/// its family's engine default.
///
/// Source: `smithy_query(query="doc", shape="Parameter", member="Source")` —
/// "The source of the parameter value, such as \"engine-default\" or \"user\"".
pub const PARAMETER_SOURCE_ENGINE_DEFAULT: &str = "engine-default";

/// `Parameter$Source` for a value a caller set on the group.
pub const PARAMETER_SOURCE_USER: &str = "user";

/// The `Source` values a `DescribeClusterParameters` request may filter on, in
/// the documented order.
///
/// Source: `smithy_query(query="doc", shape="DescribeClusterParametersMessage",
/// member="Source")` — "Valid Values: user | engine-default".
pub const PARAMETER_SOURCES: [&str; 2] = [PARAMETER_SOURCE_USER, PARAMETER_SOURCE_ENGINE_DEFAULT];

/// `ClusterParameterGroupStatus$ParameterApplyStatus` /
/// `ClusterParameterStatus$ParameterApplyStatus` for a value the database
/// already carries.
///
/// Source: `smithy_query(query="doc", shape="ClusterParameterStatus",
/// member="ParameterApplyStatus")` — "`in-sync`: The parameter value is in sync
/// with the database."
pub const PARAMETER_APPLY_STATUS_IN_SYNC: &str = "in-sync";

/// The same member for a value an associated cluster only picks up when it is
/// next rebooted.
///
/// Source: the same doc sentence as [`PARAMETER_APPLY_STATUS_IN_SYNC`] —
/// "`pending-reboot`: The parameter value will be applied after the cluster
/// reboots."
pub const PARAMETER_APPLY_STATUS_PENDING_REBOOT: &str = "pending-reboot";

/// What a change to what a cluster parameter group HOLDS reports as the group's
/// `ParameterGroupStatus`: one sentence for both the modify and the reset, which
/// are the two operations that change it and the two that answer
/// `ClusterParameterGroupNameMessage`.
///
/// Source: `smithy_query(query="doc", shape="ClusterParameterGroupNameMessage",
/// member="ParameterGroupStatus")` — "if you made a change to a parameter group
/// name-value pair, then the change COULD be pending a reboot of an associated
/// cluster" — which is exactly what this sentence says: a dynamic-only change is
/// applied to the associated clusters straight away, while a static one waits
/// for their next reboot, as
/// `smithy_query(query="doc", operation="ResetClusterParameterGroup")`'s "For
/// parameter changes to take effect you must reboot any associated clusters"
/// has it. The status does NOT depend on which parameters the request carried:
/// it names both outcomes so a caller reads the one its own change falls under,
/// which is why one constant serves both operations.
pub const PARAMETER_GROUP_STATUS_UPDATED: &str =
    "Your parameter group has been updated. If you changed only dynamic parameters, associated clusters are being modified now. If you changed static parameters, all updates, including dynamic parameters, will be applied when you reboot the associated clusters.";

/// How many parameters one `ModifyClusterParameterGroup` request may carry.
///
/// Source: `smithy_query(query="doc", shape="ModifyClusterParameterGroupMessage",
/// member="Parameters")` — "A maximum of 20 parameters can be modified in a
/// single request."
pub const MAX_MODIFIABLE_PARAMETERS: usize = 20;

/// How many parameters one `ResetClusterParameterGroup` request may name.
///
/// Source: `smithy_query(query="doc", shape="ResetClusterParameterGroupMessage",
/// member="Parameters")` — "Constraints: A maximum of 20 parameters can be reset
/// in a single request."
pub const MAX_RESETTABLE_PARAMETERS: usize = 20;

/// The parameter the workload management (WLM) configuration is held in.
pub const WLM_JSON_CONFIGURATION: &str = "wlm_json_configuration";

/// The documented maximum length of one parameter's value, in characters, for
/// the parameters Redshift publishes one for — and `None` for every other, whose
/// value is bounded only by the modelled `length 0..` of `Parameter$ParameterValue`.
///
/// Source: `smithy_query(query="doc", shape="Parameter",
/// member="ParameterValue")` — "If ParameterName is wlm_json_configuration, then
/// the maximum size of ParameterValue is 8000 characters."
pub fn parameter_value_max_len(name: &str) -> Option<usize> {
    (name == WLM_JSON_CONFIGURATION).then_some(8_000)
}

/// `Parameter$ApplyType` for a parameter an associated cluster picks up without
/// being rebooted.
///
/// Source: `smithy_query(query="shape", shape="ParameterApplyType")` — the enum
/// declares exactly `static` and `dynamic`.
pub const APPLY_TYPE_DYNAMIC: &str = "dynamic";

/// `Parameter$ApplyType` for a parameter whose new value an associated cluster
/// only sees after a reboot.
pub const APPLY_TYPE_STATIC: &str = "static";

/// `Parameter$DataType` for a parameter that takes `true` or `false`.
pub const DATA_TYPE_BOOLEAN: &str = "boolean";
/// `Parameter$DataType` for a parameter that takes a whole number.
pub const DATA_TYPE_INTEGER: &str = "integer";
/// `Parameter$DataType` for a parameter that takes free text.
pub const DATA_TYPE_STRING: &str = "string";

/// The two values every boolean parameter takes, as Redshift publishes the set
/// in `Parameter$AllowedValues` ("The valid range of values for the
/// parameter").
const BOOLEAN_ALLOWED_VALUES: &str = "true,false";

/// One engine default parameter: a setting every cluster parameter group of a
/// published family publishes, with the value it carries before anybody changes
/// it.
///
/// This is the CATALOG entry, not a stored record: a group persists only the
/// values a caller changed (see [`StoredParameterGroup::parameter_values`]), so
/// the description, data type, allowed values, apply type and modifiability a
/// read reports come from here for every group of the family at once.
#[derive(Debug, Clone, Copy)]
pub struct EngineParameter {
    /// `Parameter$ParameterName`, as the engine spells it in its own
    /// configuration.
    pub name: &'static str,
    /// `Parameter$ParameterValue` before any change: the family's engine
    /// default.
    pub default_value: &'static str,
    /// `Parameter$Description` — "A description of the parameter".
    pub description: &'static str,
    /// `Parameter$DataType` — one of [`DATA_TYPE_BOOLEAN`],
    /// [`DATA_TYPE_INTEGER`] or [`DATA_TYPE_STRING`].
    pub data_type: &'static str,
    /// `Parameter$AllowedValues` — the published value set, or `None` for a
    /// parameter the engine itself parses and publishes no set for.
    pub allowed_values: Option<&'static str>,
    /// `Parameter$ApplyType` — [`APPLY_TYPE_STATIC`] when an associated cluster
    /// only sees a new value after a reboot, [`APPLY_TYPE_DYNAMIC`] otherwise.
    pub apply_type: &'static str,
    /// `Parameter$IsModifiable` — "If true, the parameter can be modified."
    pub is_modifiable: bool,
}

impl EngineParameter {
    /// The modelled `Parameter` this setting reports for one group.
    ///
    /// `override_value` is the value that group PERSISTED for it, `None` when the
    /// group never changed it — which is exactly what decides `Source`: a value a
    /// caller set reports [`PARAMETER_SOURCE_USER`], an untouched one
    /// [`PARAMETER_SOURCE_ENGINE_DEFAULT`].
    pub fn to_api(
        &self,
        override_value: Option<&str>,
        minimum_engine_version: Option<&str>,
    ) -> Parameter {
        Parameter {
            parameter_name: Some(self.name.to_string()),
            parameter_value: Some(override_value.unwrap_or(self.default_value).to_string()),
            description: Some(self.description.to_string()),
            source: Some(
                if override_value.is_some() {
                    PARAMETER_SOURCE_USER
                } else {
                    PARAMETER_SOURCE_ENGINE_DEFAULT
                }
                .to_string(),
            ),
            data_type: Some(self.data_type.to_string()),
            allowed_values: self.allowed_values.map(str::to_string),
            apply_type: Some(self.apply_type.to_string()),
            is_modifiable: Some(self.is_modifiable),
            minimum_engine_version: minimum_engine_version.map(str::to_string),
        }
    }

    /// Whether the published `AllowedValues` set admits `value`.
    ///
    /// Source: `smithy_query(query="doc", shape="Parameter",
    /// member="AllowedValues")` — "The valid range of values for the parameter".
    /// Redshift publishes that set as a comma-separated list whose items are
    /// either a literal value (`true,false`) or, for a numeric parameter, an
    /// INCLUSIVE range written `low-high` with a possibly negative low (`-15-2`,
    /// `0-10`, `0,100-2147483647`). A parameter that publishes no set admits
    /// every value the engine itself parses, so it admits whatever it is given.
    pub fn admits(&self, value: &str) -> bool {
        let Some(allowed) = self.allowed_values else {
            return true;
        };
        let numeric = self.data_type == DATA_TYPE_INTEGER;
        allowed.split(',').map(str::trim).any(|item| {
            if item == value {
                return true;
            }
            if !numeric {
                return false;
            }
            let Some((low, high)) = allowed_range(item) else {
                return false;
            };
            value
                .parse::<i64>()
                .is_ok_and(|wanted| (low..=high).contains(&wanted))
        })
    }
}

/// One `AllowedValues` item read as an inclusive numeric range, or `None` when
/// the item is not one.
///
/// The separator is the first `-` AFTER the first character, because the low
/// bound itself may be negative (`-15-2` is -15 through 2).
fn allowed_range(item: &str) -> Option<(i64, i64)> {
    let separator = item.char_indices().skip(1).find(|&(_, c)| c == '-')?.0;
    let low = item[..separator].parse::<i64>().ok()?;
    let high = item[separator + 1..].parse::<i64>().ok()?;
    Some((low, high))
}

/// The engine default parameters a cluster parameter group of a published family
/// reports, in `ParameterName` order — which is the order a page walks, so the
/// `Marker` cursor stays a stable offset into one sequence.
///
/// Source: what the engine itself publishes for the one family this service
/// offers — `aws redshift describe-default-cluster-parameters
/// --parameter-group-family redshift-1.0`, the same catalog the Amazon Redshift
/// Cluster Management Guide's "Amazon Redshift parameter groups"
/// default-parameter table lists. Each entry's `Description`, `ApplyType`,
/// `DataType` and `AllowedValues` are that output's, VERBATIM, including its own
/// wording and spacing: a client renders the description as it arrives and reads
/// `ApplyType` to decide whether a change needs a reboot, so neither is the
/// emulator's to rephrase. The entries whose change an associated cluster sees
/// only after a reboot are published `static` (`enable_case_sensitive_identifier`,
/// `enable_user_activity_logging`, `require_ssl`, `statement_timeout`,
/// `use_fips_ssl`, `wlm_json_configuration`); every other setting applies
/// dynamically. Every setting in the catalog is one a caller may change, hence
/// [`EngineParameter::is_modifiable`] on all of them.
pub const ENGINE_DEFAULT_PARAMETERS: &[EngineParameter] = &[
    EngineParameter {
        name: "auto_analyze",
        default_value: "true",
        description: "Collects table statistics automatically in the background.",
        data_type: DATA_TYPE_BOOLEAN,
        allowed_values: Some(BOOLEAN_ALLOWED_VALUES),
        apply_type: APPLY_TYPE_DYNAMIC,
        is_modifiable: true,
    },
    EngineParameter {
        name: "auto_mv",
        default_value: "true",
        description: "Creates and refreshes materialized views automatically.",
        data_type: DATA_TYPE_BOOLEAN,
        allowed_values: Some(BOOLEAN_ALLOWED_VALUES),
        apply_type: APPLY_TYPE_DYNAMIC,
        is_modifiable: true,
    },
    EngineParameter {
        name: "datestyle",
        default_value: "ISO, MDY",
        description: "Sets the display format for date and time values.",
        data_type: DATA_TYPE_STRING,
        allowed_values: None,
        apply_type: APPLY_TYPE_DYNAMIC,
        is_modifiable: true,
    },
    EngineParameter {
        name: "enable_case_sensitive_identifier",
        default_value: "false",
        description: "Makes database, table and column names case sensitive.",
        data_type: DATA_TYPE_BOOLEAN,
        allowed_values: Some(BOOLEAN_ALLOWED_VALUES),
        apply_type: APPLY_TYPE_STATIC,
        is_modifiable: true,
    },
    EngineParameter {
        name: "enable_user_activity_logging",
        default_value: "false",
        description: "Logs each query run against the database to the user activity log.",
        data_type: DATA_TYPE_BOOLEAN,
        allowed_values: Some(BOOLEAN_ALLOWED_VALUES),
        apply_type: APPLY_TYPE_STATIC,
        is_modifiable: true,
    },
    EngineParameter {
        name: "extra_float_digits",
        default_value: "0",
        description: "Sets the number of digits displayed for floating-point values.",
        data_type: DATA_TYPE_INTEGER,
        allowed_values: Some("-15-2"),
        apply_type: APPLY_TYPE_DYNAMIC,
        is_modifiable: true,
    },
    EngineParameter {
        name: "max_concurrency_scaling_clusters",
        default_value: "1",
        description: "Sets the number of concurrency scaling clusters the cluster may use.",
        data_type: DATA_TYPE_INTEGER,
        allowed_values: Some("0-10"),
        apply_type: APPLY_TYPE_DYNAMIC,
        is_modifiable: true,
    },
    EngineParameter {
        name: "max_cursor_result_set_size",
        default_value: "default",
        description: "Sets the maximum result set size of a cursor, in megabytes.",
        data_type: DATA_TYPE_STRING,
        allowed_values: None,
        apply_type: APPLY_TYPE_DYNAMIC,
        is_modifiable: true,
    },
    EngineParameter {
        name: "query_group",
        default_value: "default",
        description: "Applies a user-defined label to the queries run in the same session.",
        data_type: DATA_TYPE_STRING,
        allowed_values: None,
        apply_type: APPLY_TYPE_DYNAMIC,
        is_modifiable: true,
    },
    EngineParameter {
        name: "require_ssl",
        default_value: "false",
        description: "require ssl for all databaseconnections",
        data_type: DATA_TYPE_BOOLEAN,
        allowed_values: Some(BOOLEAN_ALLOWED_VALUES),
        apply_type: APPLY_TYPE_STATIC,
        is_modifiable: true,
    },
    EngineParameter {
        name: "search_path",
        default_value: "$user, public",
        description: "Sets the schema search order for names that are not schema-qualified.",
        data_type: DATA_TYPE_STRING,
        allowed_values: None,
        apply_type: APPLY_TYPE_DYNAMIC,
        is_modifiable: true,
    },
    EngineParameter {
        name: "statement_timeout",
        default_value: "0",
        description: "Aborts any statement that takes over the specified number of milliseconds.",
        data_type: DATA_TYPE_INTEGER,
        allowed_values: Some("0,100-2147483647"),
        apply_type: APPLY_TYPE_STATIC,
        is_modifiable: true,
    },
    EngineParameter {
        name: "use_fips_ssl",
        default_value: "false",
        description: "Uses the FIPS-compliant SSL library for connections to the database.",
        data_type: DATA_TYPE_BOOLEAN,
        allowed_values: Some(BOOLEAN_ALLOWED_VALUES),
        apply_type: APPLY_TYPE_STATIC,
        is_modifiable: true,
    },
    EngineParameter {
        name: "wlm_json_configuration",
        default_value: "[{\"auto_wlm\":true}]",
        description: "The workload management configuration, as a JSON array of queues.",
        data_type: DATA_TYPE_STRING,
        allowed_values: None,
        apply_type: APPLY_TYPE_STATIC,
        is_modifiable: true,
    },
];

/// The engine default parameters a cluster parameter group of family `family`
/// publishes: [`ENGINE_DEFAULT_PARAMETERS`] for a family this service publishes,
/// and NOTHING for any other — a group of an unknown family reports no
/// parameters rather than another family's.
///
/// The family catalogue is read off [`published_parameter_group_families`], the
/// same place `CreateClusterParameterGroup` checks a requested family against, so
/// a group that could be created always has parameters to report.
pub fn family_parameters(family: &str) -> &'static [EngineParameter] {
    let published = published_parameter_group_families()
        .iter()
        .any(|published| published == family);
    if published {
        ENGINE_DEFAULT_PARAMETERS
    } else {
        &[]
    }
}

/// `Parameter$MinimumEngineVersion` — "The earliest engine version to which the
/// parameter can apply".
///
/// Every published engine version's family carries the whole catalog, so the
/// earliest version any of its parameters can apply to is the first entry of
/// [`published_cluster_versions`]. Derived rather than spelled out, so adding an
/// engine version cannot make the reported value disagree with the catalog.
pub fn minimum_engine_version() -> Option<String> {
    published_cluster_versions()
        .into_iter()
        .next()
        .and_then(|version| version.cluster_version)
}

/// How many parameter groups the ACCOUNT's own groups may number before
/// `CreateClusterParameterGroup` refuses with the modelled
/// `ClusterParameterGroupQuotaExceededFault`.
///
/// The same reasoning as [`CLUSTER_QUOTA`]: Amazon Redshift publishes an
/// adjustable quota on parameter groups per account per Region, and this
/// emulator models neither Service Quotas nor an account's adjustments, so this
/// is the emulator's OWN bound on
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::parameter_groups`]
/// rather than a claim about any account's limit. The default parameter groups
/// Redshift owns are outside it — the quota bounds what a caller can add.
pub const PARAMETER_GROUP_QUOTA: usize = 1_000;

/// The maintenance track a cluster joins when the request names none.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage",
/// member="MaintenanceTrackName")` — "If you don't provide a maintenance track
/// name, the cluster is assigned to the current track."
pub const DEFAULT_MAINTENANCE_TRACK: &str = "current";

/// The maintenance track that applies the PREVIOUSLY certified maintenance
/// release.
///
/// Source: `smithy_query(query="shape", shape="MaintenanceTrack")` — "If the
/// value for MaintenanceTrack is current, the cluster is updated to the most
/// recently certified maintenance release. If the value is trailing, the cluster
/// is updated to the previously certified maintenance release."
pub const TRAILING_MAINTENANCE_TRACK: &str = "trailing";

/// Every maintenance track this service publishes, in the order
/// `DescribeClusterTracks` reports them.
///
/// Source: `smithy_query(query="shape", shape="MaintenanceTrack")` on
/// `MaintenanceTrackName` — "The name of the maintenance track. Possible values
/// are current and trailing." This list IS the catalogue every operation that
/// names a maintenance track resolves against, so a cluster can always look its
/// own track up in what `DescribeClusterTracks` reports.
pub const PUBLISHED_MAINTENANCE_TRACKS: [&str; 2] =
    [DEFAULT_MAINTENANCE_TRACK, TRAILING_MAINTENANCE_TRACK];

/// The operations a maintenance track you can switch to supports, in the
/// hyphenated form `SupportedOperation$OperationName` names an operation by.
///
/// Source: `smithy_query(query="shape", shape="SupportedOperation")` —
/// "Describes the operations that are allowed on a maintenance track" — read
/// against the operations whose input carries a `MaintenanceTrackName` and acts
/// on a cluster that already exists: `ModifyClusterMessage$MaintenanceTrackName`
/// ("The name for the maintenance track that you want to assign for the
/// cluster") and `RestoreFromClusterSnapshotMessage$MaintenanceTrackName` ("The
/// name of the maintenance track for the restored cluster"). `CreateCluster`
/// also carries the member but puts a NEW cluster on a track rather than
/// switching an existing one onto it, so it is not an operation a switch target
/// supports.
pub const TRACK_SWITCH_OPERATIONS: [&str; 2] = ["modify-cluster", "restore-from-cluster-snapshot"];

/// The engine builds this service has certified, oldest first: the build column
/// of [`PUBLISHED_ENGINE_BUILDS`].
pub fn certified_engine_builds() -> Vec<&'static str> {
    PUBLISHED_ENGINE_BUILDS
        .iter()
        .map(|(_, build)| *build)
        .collect()
}

/// `MaintenanceTrack$DatabaseVersion` — "The version number for the cluster
/// release" — of one published track, or `None` for a name the catalog does not
/// publish.
///
/// The `current` track applies the most recently certified release and the
/// `trailing` track the one certified before it
/// (`smithy_query(query="shape", shape="MaintenanceTrack")`), so both are read
/// off [`certified_engine_builds`] rather than written out: this service
/// publishes ONE certified release, so until a second is added the previously
/// certified release is that same build.
pub fn track_database_version(track: &str) -> Option<String> {
    let builds = certified_engine_builds();
    let latest = *builds.last()?;
    if track == DEFAULT_MAINTENANCE_TRACK {
        Some(latest.to_string())
    } else if track == TRAILING_MAINTENANCE_TRACK {
        Some(
            builds
                .iter()
                .rev()
                .nth(1)
                .copied()
                .unwrap_or(latest)
                .to_string(),
        )
    } else {
        None
    }
}

/// The maintenance-track catalog, in the order `DescribeClusterTracks` reports
/// it.
///
/// Each track carries the release it applies ([`track_database_version`]) and,
/// as its `UpdateTargets`, every OTHER published track — "A maintenance track
/// that you can switch the current track to"
/// (`smithy_query(query="shape", shape="UpdateTarget")`) — with that target's
/// own release and the operations a switch onto it is performed by
/// ([`TRACK_SWITCH_OPERATIONS`]). Seeded into
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::maintenance_tracks`]
/// when the store is created, so the catalog a read reports and the
/// [`DEFAULT_MAINTENANCE_TRACK`] a `CreateCluster` applies cannot disagree.
pub fn published_maintenance_tracks() -> Vec<MaintenanceTrack> {
    PUBLISHED_MAINTENANCE_TRACKS
        .into_iter()
        .map(|track| MaintenanceTrack {
            maintenance_track_name: Some(track.to_string()),
            database_version: track_database_version(track),
            update_targets: Some(
                PUBLISHED_MAINTENANCE_TRACKS
                    .into_iter()
                    .filter(|target| *target != track)
                    .map(|target| UpdateTarget {
                        maintenance_track_name: Some(target.to_string()),
                        database_version: track_database_version(target),
                        supported_operations: Some(
                            TRACK_SWITCH_OPERATIONS
                                .into_iter()
                                .map(|operation| SupportedOperation {
                                    operation_name: Some(operation.to_string()),
                                })
                                .collect(),
                        ),
                    })
                    .collect(),
            ),
        })
        .collect()
}

/// Refuse a `MaintenanceTrackName` the catalog does not publish, with the
/// message the modelled `InvalidClusterTrackFault` carries.
///
/// One rule for every operation that names a maintenance track, so a name
/// `DescribeClusterTracks` does not report cannot be accepted by a create, a
/// modify or a restore.
pub fn check_maintenance_track(name: &str) -> Result<(), String> {
    if PUBLISHED_MAINTENANCE_TRACKS.contains(&name) {
        return Ok(());
    }
    Err(format!(
        "Invalid value for MaintenanceTrackName: '{name}'. Valid values: {}.",
        PUBLISHED_MAINTENANCE_TRACKS.join(" | ")
    ))
}

/// The most days a cluster's maintenance may be deferred for — the
/// account-level allowance `DescribeAccountAttributes` publishes under
/// [`ATTRIBUTE_MAX_DEFER_MAINTENANCE_DURATION`], and the bound a
/// `ModifyClusterMaintenance` deferral is held to once that operation enforces
/// one (it is still a scaffold).
///
/// Source: `smithy_query(query="doc", shape="ModifyClusterMaintenanceMessage",
/// member="DeferMaintenanceDuration")` — "An integer indicating the duration of
/// the maintenance window in days. … The duration must be 60 days or less."
pub const MAX_DEFER_MAINTENANCE_DURATION: i32 = 60;

/// `AccountAttribute$AttributeName` of the deferred-maintenance allowance.
///
/// Amazon Redshift names an account attribute in lower-case hyphenated form,
/// after the member whose value it bounds
/// (`ModifyClusterMaintenanceMessage$DeferMaintenanceDuration`).
pub const ATTRIBUTE_MAX_DEFER_MAINTENANCE_DURATION: &str = "max-defer-maintenance-duration";

/// One published account attribute: its name and the values it carries, each
/// wrapped in the modelled `AttributeValueTarget` an `AttributeValueList` holds
/// (`smithy_query(query="shape", shape="AttributeValueTarget")` — "Describes an
/// attribute value").
pub fn account_attribute<I>(name: &str, values: I) -> AccountAttribute
where
    I: IntoIterator<Item = String>,
{
    AccountAttribute {
        attribute_name: Some(name.to_string()),
        attribute_values: Some(
            values
                .into_iter()
                .map(|value| AttributeValueTarget {
                    attribute_value: Some(value),
                })
                .collect(),
        ),
    }
}

/// Every account attribute this service publishes, in the order
/// `DescribeAccountAttributes` reports them.
///
/// An entry is "a name value pair that describes an aspect of an account"
/// (`smithy_query(query="shape", shape="AccountAttribute")`) — a published
/// allowance of the ACCOUNT, not a record a caller creates — so the catalog
/// holds the allowances this service publishes for an account: the
/// deferred-maintenance maximum ([`MAX_DEFER_MAINTENANCE_DURATION`]), the
/// documented bound a `ModifyClusterMaintenance` deferral is held to once that
/// operation enforces one. Seeded into
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::account_attributes`]
/// when the store is created, exactly like the engine-version and
/// maintenance-track catalogs, so the allowance a read reports and the bound
/// that enforcement reads are one constant.
pub fn published_account_attributes() -> Vec<AccountAttribute> {
    vec![account_attribute(
        ATTRIBUTE_MAX_DEFER_MAINTENANCE_DURATION,
        [MAX_DEFER_MAINTENANCE_DURATION.to_string()],
    )]
}

/// The currency every price in the reserved-node catalog is quoted in.
pub const RESERVED_NODE_CURRENCY_CODE: &str = "USD";

/// The `RecurringChargeFrequency` a reserved node offering's recurring charge is
/// applied at: once per hour of the term.
pub const RECURRING_CHARGE_FREQUENCY_HOURLY: &str = "Hourly";

/// Every term a reserved node offering is published for, as
/// `(duration in seconds, share of the on-demand rate the term is charged at)`,
/// shortest term first.
///
/// `Duration` is "The duration, in seconds, for which the offering will reserve
/// the node" (`smithy_query(query="shape", shape="ReservedNodeOffering")`), and
/// a term is one dimension of a reservation's configuration — "(term, payment
/// type, or number of nodes)" (`smithy_query(query="doc",
/// shape="AcceptReservedNodeExchange")`). The two terms are a one-year and a
/// three-year commitment of 365-day years, and the longer commitment is charged
/// at the lower share; both the terms and the shares are this emulator's own
/// published catalog, the model publishing no list of either.
pub const RESERVED_NODE_TERMS: [(i32, f64); 2] = [(31_536_000, 0.70), (94_608_000, 0.45)];

/// Every offering type a reserved node offering is published under, as
/// `(name, share of the term's cost charged upfront)`, in catalog order.
///
/// `OfferingType` is "The anticipated utilization of the reserved node, as
/// defined in the reserved node offering" (`smithy_query(query="shape",
/// shape="ReservedNodeOffering")`), so the catalog publishes one offering per
/// utilization level, and the levels are ordered by how much of the term they
/// charge upfront. The names and the shares are this emulator's own published
/// catalog; the model publishes no value list for the member, naming only
/// [`HEAVY_UTILIZATION_OFFERING_TYPE`].
pub const RESERVED_NODE_OFFERING_TYPES: [(&str, f64); 3] = [
    ("Light Utilization", 0.25),
    ("Medium Utilization", 0.50),
    ("Heavy Utilization", 0.75),
];

/// The one offering type whose remaining term cost is charged as a RECURRING
/// charge rather than per running hour.
///
/// Source: `smithy_query(query="doc", shape="ReservedNodeOffering",
/// member="RecurringCharges")` — "The charge to your account regardless of
/// whether you are creating any clusters using the node offering. Recurring
/// charges are only in effect for heavy-utilization reserved nodes."
pub const HEAVY_UTILIZATION_OFFERING_TYPE: &str = "Heavy Utilization";

/// The modelled `ReservedNodeOfferingType` of an offering an existing
/// reservation can be exchanged onto (a value of
/// `RESERVED_NODE_OFFERING_TYPE_VALUES`).
pub const RESERVED_NODE_OFFERING_TYPE_UPGRADABLE: &str = "Upgradable";

/// The modelled `ReservedNodeOfferingType` of an offering that is only ever
/// purchased outright (a value of `RESERVED_NODE_OFFERING_TYPE_VALUES`).
pub const RESERVED_NODE_OFFERING_TYPE_REGULAR: &str = "Regular";

/// The node-type prefix of an offering an existing reservation can be exchanged
/// onto.
///
/// Source: `smithy_query(query="doc", shape="GetReservedNodeExchangeOfferings")`
/// — "Returns an array of DC2 ReservedNodeOfferings that matches the payment
/// type, term, and usage price of the given DC1 reserved node": the offerings an
/// exchange can target are the DC2 ones, so those are the offerings the catalog
/// publishes as [`RESERVED_NODE_OFFERING_TYPE_UPGRADABLE`].
pub const EXCHANGE_TARGET_NODE_TYPE_PREFIX: &str = "dc2.";

/// The hourly on-demand rate, in [`RESERVED_NODE_CURRENCY_CODE`], this emulator
/// publishes for one node of each offered node type — the rate every price in
/// that node type's offerings is derived from.
///
/// Listed in the order of [`ORDERABLE_NODE_TYPES`], and covering every node type
/// in it, so the catalog publishes an offering for every node type a cluster can
/// be created with. The rates rise with the size of the node and are this
/// emulator's own published catalog: the model publishes no price for any node
/// type.
const NODE_TYPE_HOURLY_RATE: [(&str, f64); 10] = [
    ("dc2.large", 0.25),
    ("dc2.8xlarge", 4.80),
    ("rg.large", 0.36),
    ("rg.xlarge", 0.72),
    ("rg.4xlarge", 2.88),
    ("rg.12xlarge", 8.64),
    ("ra3.large", 0.52),
    ("ra3.xlplus", 1.09),
    ("ra3.4xlarge", 3.26),
    ("ra3.16xlarge", 13.04),
];

/// The hourly on-demand rate [`NODE_TYPE_HOURLY_RATE`] publishes for this node
/// type, if the catalog offers it.
pub fn node_type_hourly_rate(node_type: &str) -> Option<f64> {
    NODE_TYPE_HOURLY_RATE
        .into_iter()
        .find(|(offered, _)| *offered == node_type)
        .map(|(_, rate)| rate)
}

/// `value` rounded to `places` decimal places, so a published price carries only
/// the digits a currency amount has.
fn round_to(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    (value * scale).round() / scale
}

/// The offering identifier of the one offering for this node type, term and
/// offering type: a lowercase-hex 8-4-4-4-12 value.
///
/// Derived from the offering's OWN three dimensions, so the same offering always
/// carries the same id — the id a caller passes to `PurchaseReservedNodeOffering`
/// ("you then use the unique offering ID in you call to
/// PurchaseReservedNodeOffering", `smithy_query(query="doc",
/// operation="DescribeReservedNodeOfferings")`) names the same offering on every
/// read and is not a per-run value.
pub fn reserved_node_offering_id(node_type: &str, duration: i32, offering_type: &str) -> String {
    content_uuid(&format!(
        "reservednodeoffering/{node_type}/{duration}/{offering_type}"
    ))
}

/// One published reserved node offering: what it reserves, for how long, and how
/// the term's cost is charged.
///
/// The whole term costs the node type's hourly rate
/// ([`NODE_TYPE_HOURLY_RATE`]) at the term's share ([`RESERVED_NODE_TERMS`]) for
/// every hour of the term, and the offering type splits that cost in two: the
/// share it charges upfront becomes the `FixedPrice`, and the remainder is spread
/// evenly over the term's hours — charged per hour the cluster using the offering
/// RUNS as the `UsagePrice` ("The rate you are charged for each hour the cluster
/// that is using the offering is running"), except for
/// [`HEAVY_UTILIZATION_OFFERING_TYPE`], where it is charged for every hour
/// regardless as a `RecurringCharges` entry instead.
fn reserved_node_offering(node_type: &str, duration: i32, offering_type: &str) -> ReservedNodeOffering {
    let hourly_rate = node_type_hourly_rate(node_type).unwrap_or_default();
    let term_share = RESERVED_NODE_TERMS
        .into_iter()
        .find(|(offered, _)| *offered == duration)
        .map(|(_, share)| share)
        .unwrap_or_default();
    let upfront_share = RESERVED_NODE_OFFERING_TYPES
        .into_iter()
        .find(|(offered, _)| *offered == offering_type)
        .map(|(_, share)| share)
        .unwrap_or_default();

    let hours = f64::from(duration) / 3600.0;
    let reserved_hourly_rate = hourly_rate * term_share;
    let fixed_price = round_to(reserved_hourly_rate * hours * upfront_share, 2);
    let hourly_remainder = round_to(reserved_hourly_rate * (1.0 - upfront_share), 3);
    let heavy = offering_type == HEAVY_UTILIZATION_OFFERING_TYPE;

    ReservedNodeOffering {
        reserved_node_offering_id: Some(reserved_node_offering_id(
            node_type,
            duration,
            offering_type,
        )),
        node_type: Some(node_type.to_string()),
        duration: Some(duration),
        fixed_price: Some(fixed_price),
        usage_price: Some(if heavy { 0.0 } else { hourly_remainder }),
        currency_code: Some(RESERVED_NODE_CURRENCY_CODE.to_string()),
        offering_type: Some(offering_type.to_string()),
        recurring_charges: Some(if heavy {
            vec![RecurringCharge {
                recurring_charge_amount: Some(hourly_remainder),
                recurring_charge_frequency: Some(RECURRING_CHARGE_FREQUENCY_HOURLY.to_string()),
            }]
        } else {
            Vec::new()
        }),
        reserved_node_offering_type: Some(
            if node_type.starts_with(EXCHANGE_TARGET_NODE_TYPE_PREFIX) {
                RESERVED_NODE_OFFERING_TYPE_UPGRADABLE.to_string()
            } else {
                RESERVED_NODE_OFFERING_TYPE_REGULAR.to_string()
            },
        ),
    }
}

/// Every reserved node offering this service publishes, in the order
/// `DescribeReservedNodeOfferings` reports them: node type (as
/// [`ORDERABLE_NODE_TYPES`] offers them), then term (shortest first), then
/// offering type.
///
/// The catalog is the full product of those three dimensions — "a list of the
/// available reserved node offerings … including the node type, the fixed and
/// recurring costs of reserving the node and duration the node will be reserved
/// for you" (`smithy_query(query="doc",
/// operation="DescribeReservedNodeOfferings")`) — so a node type a cluster can be
/// created with can be reserved for either term under any offering type. Seeded
/// into
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::reserved_node_offerings`]
/// when the store is created, exactly like the engine-version and
/// maintenance-track catalogs: an offering exists before anybody reserves
/// anything, and no operation adds to or removes from the list.
pub fn published_reserved_node_offerings() -> Vec<ReservedNodeOffering> {
    let mut offerings = Vec::new();
    for node_type in ORDERABLE_NODE_TYPES {
        if node_type_hourly_rate(node_type).is_none() {
            continue;
        }
        for (duration, _) in RESERVED_NODE_TERMS {
            for (offering_type, _) in RESERVED_NODE_OFFERING_TYPES {
                offerings.push(reserved_node_offering(node_type, duration, offering_type));
            }
        }
    }
    offerings
}

/// The default number of days automated snapshots are retained.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage",
/// member="AutomatedSnapshotRetentionPeriod")` — "Default: 1 … Must be a value
/// from 0 to 35."
pub const DEFAULT_AUTOMATED_RETENTION: i32 = 1;

/// Inclusive bounds on `AutomatedSnapshotRetentionPeriod`, from the same doc
/// sentence as [`DEFAULT_AUTOMATED_RETENTION`].
pub const AUTOMATED_RETENTION_BOUNDS: (i32, i32) = (0, 35);

/// Inclusive bounds on a manual-snapshot retention period, which may also be
/// `-1` ("retained indefinitely").
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage",
/// member="ManualSnapshotRetentionPeriod")` — "The value must be either -1 or an
/// integer between 1 and 3,653."
pub const MANUAL_RETENTION_BOUNDS: (i32, i32) = (1, 3653);

/// The sentinel manual retention period meaning "retained indefinitely".
pub const MANUAL_RETENTION_INDEFINITE: i32 = -1;

/// Whether a manual retention period is one this service admits: the
/// "indefinitely" sentinel, or a day count inside [`MANUAL_RETENTION_BOUNDS`].
///
/// Source: the doc sentence on [`MANUAL_RETENTION_BOUNDS`], which every member
/// carrying a manual retention period repeats — "The value must be either -1 or
/// an integer between 1 and 3,653."
pub fn manual_retention_admitted(period: i32) -> bool {
    period == MANUAL_RETENTION_INDEFINITE
        || (MANUAL_RETENTION_BOUNDS.0..=MANUAL_RETENTION_BOUNDS.1).contains(&period)
}

/// The number of seconds in one day: a manual retention period is a count of
/// days, and every instant this service derives from one is measured in seconds.
const SECONDS_PER_DAY: i64 = 24 * 60 * 60;

/// When a manual retention period of `period` days set on a snapshot taken at
/// `created_at` ENDS, in epoch seconds — `None` for a snapshot retained
/// indefinitely, or one whose record carries no creation time.
///
/// The window is counted from the snapshot's own `SnapshotCreateTime`, "the time
/// (in UTC format) when Amazon Redshift began the snapshot"
/// (`smithy_query(query="doc", shape="Snapshot", member="SnapshotCreateTime")`).
/// That is what makes `smithy_query(query="doc",
/// shape="ModifyClusterSnapshotMessage", member="ManualSnapshotRetentionPeriod")`
/// — "If the manual snapshot falls outside of the new retention period, you can
/// specify the force option to immediately delete the snapshot" — possible at
/// all: a window counted from the call that sets it could never already have
/// ended.
pub fn manual_retention_end(
    created_at: Option<::aws_smithy_types::DateTime>,
    period: i32,
) -> Option<i64> {
    if period == MANUAL_RETENTION_INDEFINITE {
        return None;
    }
    created_at.map(|created| created.secs() + i64::from(period) * SECONDS_PER_DAY)
}

/// Whether the manual retention period `period` of a snapshot taken at
/// `created_at` has ALREADY passed at `now`.
///
/// This is the condition `smithy_query(query="doc",
/// shape="ModifyClusterSnapshotMessage", member="Force")` overrides — "a Boolean
/// option to override an exception if the retention period has already passed".
pub fn manual_retention_has_passed(
    created_at: Option<::aws_smithy_types::DateTime>,
    period: i32,
    now: ::aws_smithy_types::DateTime,
) -> bool {
    manual_retention_end(created_at, period).is_some_and(|end| end <= now.secs())
}

/// `Snapshot$ManualSnapshotRemainingDays`: "the number of days until a manual
/// snapshot will pass its retention period"
/// (`smithy_query(query="doc", shape="Snapshot",
/// member="ManualSnapshotRemainingDays")`).
///
/// Counted from `now` to the end of the window ([`manual_retention_end`]) and
/// rounded UP, so a snapshot taken today under a period of `n` days has `n` days
/// remaining and the count reaches zero only once the period has passed.
/// `None` for a snapshot retained indefinitely: it never passes a period.
pub fn manual_snapshot_remaining_days(
    created_at: Option<::aws_smithy_types::DateTime>,
    period: i32,
    now: ::aws_smithy_types::DateTime,
) -> Option<i32> {
    let end = manual_retention_end(created_at, period)?;
    let remaining = end - now.secs();
    if remaining <= 0 {
        return Some(0);
    }
    let days = (remaining as u64).div_ceil(SECONDS_PER_DAY as u64);
    Some(i32::try_from(days).unwrap_or(i32::MAX))
}

/// Set a snapshot's manual retention period and every member it DERIVES.
///
/// One rule for the create, the modify and the read, so no two paths can report
/// different retention bookkeeping for the same period.
pub fn apply_manual_retention(
    snapshot: &mut Snapshot,
    period: i32,
    now: ::aws_smithy_types::DateTime,
) {
    snapshot.manual_snapshot_retention_period = Some(period);
    refresh_manual_retention(snapshot, now);
}

/// Recompute the members a snapshot DERIVES from the manual retention period it
/// already carries, as of `now`.
///
/// `SnapshotRetentionStartTime` is "a timestamp representing the start of the
/// retention period for the snapshot" (`smithy_query(query="doc",
/// shape="Snapshot", member="SnapshotRetentionStartTime")`), which is the instant
/// the window [`manual_retention_end`] measures starts at; a snapshot retained
/// indefinitely is under no retention period, so it reports neither member.
///
/// Called by [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::settle`]:
/// the remaining-day count falls as the clock advances whether or not anybody
/// calls, so recomputing it on a read shows nothing a later caller could not
/// already have observed.
pub fn refresh_manual_retention(snapshot: &mut Snapshot, now: ::aws_smithy_types::DateTime) {
    let period = snapshot
        .manual_snapshot_retention_period
        .unwrap_or(MANUAL_RETENTION_INDEFINITE);
    snapshot.snapshot_retention_start_time = if period == MANUAL_RETENTION_INDEFINITE {
        None
    } else {
        snapshot.snapshot_create_time
    };
    snapshot.manual_snapshot_remaining_days =
        manual_snapshot_remaining_days(snapshot.snapshot_create_time, period, now);
}

/// The documented `MaxRecords` window every paginated Redshift `Describe*`
/// operation shares: default 100, minimum 20, maximum 100.
///
/// Source: `smithy_query(query="doc", shape="DescribeClustersMessage",
/// member="MaxRecords")` and the identical sentence on
/// `DescribeClusterSnapshotsMessage$MaxRecords` and
/// `DescribeClusterVersionsMessage$MaxRecords` — "Default: 100 … Constraints:
/// minimum 20, maximum 100."
pub const MAX_RECORDS_DEFAULT: i32 = 100;
/// The documented minimum of the `MaxRecords` window. See [`MAX_RECORDS_DEFAULT`].
pub const MAX_RECORDS_MIN: i32 = 20;
/// The documented maximum of the `MaxRecords` window. See [`MAX_RECORDS_DEFAULT`].
pub const MAX_RECORDS_MAX: i32 = 100;

/// `ClusterIamRole$ApplyStatus` once the association is established: "the role is
/// available for use by the cluster".
///
/// Source: `smithy_query(query="doc", shape="ClusterIamRole",
/// member="ApplyStatus")`, which publishes exactly three statuses — this one,
/// [`IAM_ROLE_APPLY_STATUS_ADDING`] and [`IAM_ROLE_APPLY_STATUS_REMOVING`].
pub const IAM_ROLE_APPLY_STATUS_IN_SYNC: &str = "in-sync";
/// `ClusterIamRole$ApplyStatus` while the role "is in the process of being
/// associated with the cluster". See [`IAM_ROLE_APPLY_STATUS_IN_SYNC`].
pub const IAM_ROLE_APPLY_STATUS_ADDING: &str = "adding";
/// `ClusterIamRole$ApplyStatus` while the role "is in the process of being
/// disassociated" from the cluster. See [`IAM_ROLE_APPLY_STATUS_IN_SYNC`].
pub const IAM_ROLE_APPLY_STATUS_REMOVING: &str = "removing";

/// `ClusterStatus` while the cluster is being provisioned.
pub const STATUS_CREATING: &str = "creating";
/// `ClusterStatus` / snapshot `Status` once provisioning is done.
pub const STATUS_AVAILABLE: &str = "available";
/// `ClusterStatus` while the service is applying a change to a cluster that was
/// already provisioned: the cluster is only intermittently available for queries
/// until the change is applied (see [`availability_status`]).
pub const STATUS_MODIFYING: &str = "modifying";
/// `ClusterStatus` a `DeleteCluster` response reports.
pub const STATUS_DELETING: &str = "deleting";

/// The snapshot `Status` a snapshot this service no longer holds reports.
///
/// Source: `smithy_query(query="doc", shape="Snapshot", member="Status")` —
/// "DeleteClusterSnapshot returns status as \"deleted\"": the record a call
/// reports back after removing it from the store.
pub const STATUS_DELETED: &str = "deleted";

/// `RestoreStatus$Status` while the cluster a `RestoreFromClusterSnapshot`
/// minted is still being provisioned.
///
/// Source: `smithy_query(query="doc", shape="RestoreStatus", member="Status")` —
/// "Returns starting, restoring, completed, or failed."
pub const RESTORE_STATUS_RESTORING: &str = "restoring";

/// `RestoreStatus$Status` once the restored cluster is `available`. Applied by
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::settle`] with the
/// same clock-owed transition that makes the cluster available. See
/// [`RESTORE_STATUS_RESTORING`] for the published value list.
pub const RESTORE_STATUS_COMPLETED: &str = "completed";

/// The `SnapshotType` of every snapshot a caller asks for, whether through
/// `CreateClusterSnapshot` or as a `DeleteCluster` final snapshot.
///
/// Source: `smithy_query(query="doc", shape="Snapshot", member="SnapshotType")`
/// — "Snapshots created using CreateClusterSnapshot and CopyClusterSnapshot are
/// of type \"manual\"".
pub const SNAPSHOT_TYPE_MANUAL: &str = "manual";

/// The `SnapshotType` Redshift gives a snapshot it took on its own schedule.
///
/// Source: `smithy_query(query="doc", shape="DescribeClusterSnapshotsMessage",
/// member="SnapshotType")` — "Valid Values: automated | manual".
pub const SNAPSHOT_TYPE_AUTOMATED: &str = "automated";

/// How many snapshot identifiers ONE batch snapshot operation accepts before it
/// refuses the whole call with its own modelled limit fault.
///
/// Source: `smithy_query(query="shape",
/// shape="BatchModifyClusterSnapshotsLimitExceededFault")` — "The maximum number
/// for snapshot identifiers has been reached. The limit is 100."
pub const BATCH_SNAPSHOT_IDENTIFIER_LIMIT: usize = 100;

/// The `SnapshotErrorMessage$FailureCode` a batch snapshot operation reports for
/// an entry naming no snapshot this store holds.
///
/// `SnapshotErrorMessage$FailureCode` is "the failure code for the error"
/// (`smithy_query(query="shape", shape="SnapshotErrorMessage")`), and a batch
/// operation reports per entry the same condition the one-snapshot operation
/// raises as a fault — so the code is that fault's own wire code,
/// `smithy_query(query="shape", shape="ClusterSnapshotNotFoundFault")` →
/// `awsQueryError` code `ClusterSnapshotNotFound`.
pub const SNAPSHOT_FAILURE_NOT_FOUND: &str = "ClusterSnapshotNotFound";

/// The `SnapshotErrorMessage$FailureCode` for an entry naming a snapshot whose
/// state carries nothing the call can act on.
///
/// The same reasoning as [`SNAPSHOT_FAILURE_NOT_FOUND`]:
/// `smithy_query(query="shape", shape="InvalidClusterSnapshotStateFault")` →
/// `awsQueryError` code `InvalidClusterSnapshotState`.
pub const SNAPSHOT_FAILURE_INVALID_STATE: &str = "InvalidClusterSnapshotState";

/// The `SnapshotErrorMessage$FailureCode` for an entry whose snapshot would fall
/// outside the requested retention window.
///
/// The same reasoning as [`SNAPSHOT_FAILURE_NOT_FOUND`]:
/// `smithy_query(query="shape", shape="InvalidRetentionPeriodFault")` →
/// `awsQueryError` code `InvalidRetentionPeriodFault`.
pub const SNAPSHOT_FAILURE_INVALID_RETENTION_PERIOD: &str = "InvalidRetentionPeriodFault";

/// How many clusters the store holds before `CreateCluster` refuses with the
/// modelled `ClusterQuotaExceededFault`.
///
/// Amazon Redshift publishes an adjustable quota on clusters per account per
/// Region, and this emulator models neither Service Quotas nor an account's
/// adjustments — so this is the emulator's OWN bound on the store rather than a
/// claim about any account's limit. It is set far above any workflow an emulator
/// serves, and it is what stops [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::clusters`]
/// growing without end: every record it holds is released by `DeleteCluster`.
pub const CLUSTER_QUOTA: usize = 1_000;

/// How many snapshots the store holds before `CreateClusterSnapshot` — and the
/// final snapshot `DeleteCluster` takes — refuses with the modelled
/// `ClusterSnapshotQuotaExceededFault`.
///
/// The same reasoning as [`CLUSTER_QUOTA`]: the emulator's own bound on
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::snapshots`], not a
/// claim about an account's published limit. It is larger than the cluster bound
/// because a cluster accumulates snapshots over its life.
pub const SNAPSHOT_QUOTA: usize = 10_000;

/// How many accounts one snapshot may be shared with before
/// `AuthorizeSnapshotAccess` refuses with the modelled
/// `AuthorizationQuotaExceededFault`.
///
/// The same reasoning as [`CLUSTER_QUOTA`]: neither the model nor the shape docs
/// state a number, so this is the emulator's OWN bound on one snapshot's
/// `Snapshot$AccountsWithRestoreAccess` list rather than a claim about any
/// account's published limit. It is what stops that list growing without end:
/// `RevokeSnapshotAccess` releases an entry.
pub const RESTORE_ACCOUNTS_PER_SNAPSHOT_QUOTA: usize = 20;

/// The identifier a request names to share a snapshot with Amazon Web Services
/// Support, rather than with an account of its own.
///
/// Source: `smithy_query(query="doc", shape="AuthorizeSnapshotAccessMessage",
/// member="AccountWithRestoreAccess")` — "To share a snapshot with Amazon Web
/// Services Support, specify amazon-redshift-support" — and
/// `smithy_query(query="doc", shape="AccountWithRestoreAccess",
/// member="AccountAlias")` — "For Amazon Web Services Support, the identifier is
/// amazon-redshift-support".
pub const SUPPORT_ACCOUNT_ALIAS: &str = "amazon-redshift-support";

/// How many digits an Amazon Web Services account identifier has, which is what
/// [`account_with_restore_access`] admits beside [`SUPPORT_ACCOUNT_ALIAS`].
///
/// Source: `engine::emulator_account_id()`, the account every ARN this service
/// mints is scoped to, is a 12-digit identifier — the one form an Amazon Web
/// Services account id takes.
pub const ACCOUNT_ID_DIGITS: usize = 12;

/// The `AccountWithRestoreAccess` record the account an
/// `AuthorizeSnapshotAccess` / `RevokeSnapshotAccess` request names authorizes,
/// or why the value names no account.
///
/// One rule for both operations, so an authorization cannot be stored under a
/// shape a revoke then fails to recognise: an account IDENTIFIER is reported as
/// `AccountId` ("The identifier of an Amazon Web Services account authorized to
/// restore a snapshot") and the documented support alias as `AccountAlias`
/// ("For Amazon Web Services Support, the identifier is
/// `amazon-redshift-support`") — this emulator knows no account number for
/// Amazon Web Services Support, so it invents none.
pub fn account_with_restore_access(value: &str) -> Result<AccountWithRestoreAccess, String> {
    if value == SUPPORT_ACCOUNT_ALIAS {
        return Ok(AccountWithRestoreAccess {
            account_id: None,
            account_alias: Some(value.to_string()),
        });
    }
    if value.len() == ACCOUNT_ID_DIGITS && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(AccountWithRestoreAccess {
            account_id: Some(value.to_string()),
            account_alias: None,
        });
    }
    Err(format!(
        "Invalid value for AccountWithRestoreAccess: '{value}'. It must be a {ACCOUNT_ID_DIGITS}-digit Amazon Web Services account identifier or '{SUPPORT_ACCOUNT_ALIAS}'."
    ))
}

/// Whether two authorization records name the SAME grantee — the account id
/// where one is held, the alias otherwise.
///
/// This is what makes authorizing an account twice an
/// `AuthorizationAlreadyExistsFault` and what a revoke matches the entry to
/// release by.
pub fn same_restore_account(
    left: &AccountWithRestoreAccess,
    right: &AccountWithRestoreAccess,
) -> bool {
    (left.account_id.is_some() && left.account_id == right.account_id)
        || (left.account_alias.is_some() && left.account_alias == right.account_alias)
}

/// How many cluster subnet groups the account's own groups may number before
/// `CreateClusterSubnetGroup` refuses with the modelled
/// `ClusterSubnetGroupQuotaExceededFault`.
///
/// The same reasoning as [`CLUSTER_QUOTA`]: Amazon Redshift publishes an
/// adjustable quota on subnet groups per account per Region, and this emulator
/// models neither Service Quotas nor an account's adjustments, so this is the
/// emulator's OWN bound on
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::subnet_groups`]
/// rather than a claim about any account's limit.
pub const SUBNET_GROUP_QUOTA: usize = 1_000;

/// How many subnets one cluster subnet group may hold before
/// `CreateClusterSubnetGroup` refuses with the modelled
/// `ClusterSubnetQuotaExceededFault`.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterSubnetGroupMessage",
/// member="SubnetIds")` — "A maximum of 20 subnets can be modified in a single
/// request."
pub const SUBNETS_PER_SUBNET_GROUP_QUOTA: usize = 20;

/// `ClusterSubnetGroup$SubnetGroupStatus` for a group whose every subnet
/// resolved.
///
/// Source: `smithy_query(query="doc", shape="ClusterSubnetGroup",
/// member="SubnetGroupStatus")` — "Possible values are Complete, Incomplete and
/// Invalid." Every subnet this service admits is one it stored, so a stored group
/// is always `Complete`.
pub const SUBNET_GROUP_STATUS_COMPLETE: &str = "Complete";

/// `Subnet$SubnetStatus` for a subnet of a stored group.
pub const SUBNET_STATUS_ACTIVE: &str = "Active";

/// The `Cluster$IpAddressType` values a subnet group of this service supports.
///
/// Source: `smithy_query(query="doc", shape="ClusterSubnetGroup",
/// member="SupportedClusterIpAddressTypes")` — "Possible values are ipv4 and
/// dualstack" — with `Ipv6CidrBlockNotFoundFault`'s own "There are no subnets in
/// your VPC with associated IPv6 CIDR blocks. To use dual-stack mode, associate
/// an IPv6 CIDR block with each subnet in your VPC": this service models no IPv6
/// CIDR block on a subnet, so a stored group supports `ipv4` alone.
pub const SUPPORTED_CLUSTER_IP_ADDRESS_TYPES: [&str; 1] = ["ipv4"];

/// How many `SortingEntities` one `DescribeClusterSnapshots` request may carry.
///
/// `SnapshotAttributeToSortBy` declares three values, so a request naming more
/// entities than there are attributes can only repeat one; the cap keeps the
/// per-request sort work a small constant multiple of one pass over the store,
/// which matters because the sort runs while the shared state lock is held.
pub const MAX_SORTING_ENTITIES: usize = 3;

/// How many usage limits the store holds before `CreateUsageLimit` refuses with
/// the modelled `LimitExceededFault`.
///
/// The same reasoning as [`CLUSTER_QUOTA`]: the emulator's own bound on
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::usage_limits`],
/// not a claim about an account's published limit. A usage limit's identity is
/// the id its create mints, so repeating one configuration appends another
/// record; this bound is what stops that collection growing without end. It is
/// set far above any workflow an emulator serves (every feature of every cluster
/// the store can hold, several times over), and every record it holds is
/// released by `DeleteUsageLimit`.
pub const USAGE_LIMIT_QUOTA: usize = 10_000;

/// How many Query Editor (QEV2) IAM Identity Center applications the store
/// holds before `CreateQev2IdcApplication` refuses with the modelled
/// `LimitExceededFault`.
///
/// The same reasoning as [`USAGE_LIMIT_QUOTA`]: the emulator's own bound on
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::qev2_idc_applications`],
/// not a claim about an account's published limit. An application's identity is
/// its name, and names are unlimited in number, so this bound is what stops the
/// collection growing without end; it is set far above any workflow an emulator
/// serves, and every record it holds is released by
/// `DeleteQev2IdcApplication`.
pub const QEV2_IDC_APPLICATION_QUOTA: usize = 10_000;

/// How many HSM configurations the store holds before `CreateHsmConfiguration`
/// refuses with the modelled `HsmConfigurationQuotaExceededFault`.
///
/// The same reasoning as [`QEV2_IDC_APPLICATION_QUOTA`]: the emulator's own
/// bound on
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::hsm_configurations`],
/// not a claim about an account's published limit. An HSM configuration's
/// identity is the identifier its create names, and identifiers are unlimited in
/// number, so this bound is what stops the collection growing without end; it is
/// set far above any workflow an emulator serves, and every record it holds is
/// released by `DeleteHsmConfiguration`.
pub const HSM_CONFIGURATION_QUOTA: usize = 10_000;

/// How many tags one resource may carry before a create that would exceed it
/// refuses with the modelled `TagLimitExceededFault`.
///
/// Source: `smithy_query(query="doc", operation="CreateTags")` — "A resource can
/// have up to 50 tags. If you try to create more than 50 tags for a resource,
/// you will receive an error and the attempt will fail."
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// `UsageLimit$Period` when `CreateUsageLimit` names none.
///
/// Source: `smithy_query(query="doc", shape="CreateUsageLimitMessage",
/// member="Period")` — "The default is monthly."
pub const DEFAULT_USAGE_LIMIT_PERIOD: &str = "monthly";

/// `UsageLimit$BreachAction` when `CreateUsageLimit` names none.
///
/// Source: `smithy_query(query="doc", shape="CreateUsageLimitMessage",
/// member="BreachAction")` — "The default is log."
pub const DEFAULT_USAGE_LIMIT_BREACH_ACTION: &str = "log";

/// The `LimitType` each `FeatureType` is limited by, which is a property of the
/// FEATURE rather than a choice: a feature metered in minutes takes `time`, one
/// metered in terabytes takes `data-scanned`.
///
/// Source: `smithy_query(query="doc", shape="CreateUsageLimitMessage",
/// member="LimitType")` — "If FeatureType is spectrum, then LimitType must be
/// data-scanned. If FeatureType is concurrency-scaling, then LimitType must be
/// time. If FeatureType is cross-region-datasharing, then LimitType must be
/// data-scanned. If FeatureType is extra-compute-for-automatic-optimization,
/// then LimitType must be time." The keys are every value of
/// `UsageLimitFeatureType`.
pub const USAGE_LIMIT_FEATURE_LIMIT_TYPES: [(&str, &str); 4] = [
    ("spectrum", "data-scanned"),
    ("concurrency-scaling", "time"),
    ("cross-region-datasharing", "data-scanned"),
    ("extra-compute-for-automatic-optimization", "time"),
];

/// The one `LimitType` this feature may be limited by, or `None` for a feature
/// the model does not declare.
pub fn usage_limit_type_of_feature(feature_type: &str) -> Option<&'static str> {
    USAGE_LIMIT_FEATURE_LIMIT_TYPES
        .iter()
        .find(|(feature, _)| *feature == feature_type)
        .map(|(_, limit_type)| *limit_type)
}

/// A fresh `UsageLimitId`.
///
/// A usage limit is "identified by the returned usage limit identifier"
/// (`smithy_query(query="doc", operation="CreateUsageLimit")`): the id is minted
/// by the service, carries nothing from the request, and is what every later
/// call names the limit by — so it is drawn per call rather than derived from
/// the limit's own values, and the minted id is STORED so every read of that
/// limit reports the same one.
///
/// Minted by the same generator this crate already uses for its other
/// wire-visible 8-4-4-4-12 lowercase-hex value, the awsQuery envelope's
/// `<RequestId>`: a per-process sequence number digested with the wall clock, so
/// two limits created in the same nanosecond still get different ids and no new
/// dependency is needed.
pub fn new_usage_limit_id() -> String {
    crate::service::query_error_envelope::new_request_id()
}

/// The `IdcOnboardStatus` of a registered Query Editor (QEV2) IAM Identity
/// Center application.
///
/// The model publishes no value list for the member (`smithy_query(query="doc",
/// shape="Qev2IdcApplication", member="IdcOnboardStatus")` — "The onboarding
/// status for the Amazon Redshift Query Editor (QEV2) IAM Identity Center
/// application"), so the single status this emulator reports is its own choice:
/// the application is registered by the time `CreateQev2IdcApplication`
/// returns — the call itself mints the managed application — so the status it
/// reports is terminal and no transition is owed on it.
pub const QEV2_IDC_ONBOARD_STATUS: &str = "Completed";

/// What every operation taking a `Qev2IdcApplicationArn` answers when the ARN
/// names no registered application: ONE fixed sentence naming the resource type.
///
/// The message carries nothing from the request, so the refusal never reflects
/// a caller's value back and two operations enforcing the same precondition
/// cannot report different wording. The modelled doc text
/// (`smithy_query(query="doc", shape="Qev2IdcApplicationNotExistsFault")`) is
/// the API reference's prose about the fault, not the sentence the service puts
/// on the wire, which is why it is not reused verbatim here.
pub const QEV2_IDC_APPLICATION_NOT_EXISTS: &str =
    "The Qev2IdcApplication you specified doesn't exist.";

/// What `ModifyQev2IdcApplication` answers when no application it may act on
/// matches the ARN — see
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::modifiable_qev2_idc_application_position`].
///
/// A refusal states the precondition the operation enforces, and a modify's is
/// wider than a read's or a delete's: the application must exist AND have
/// finished onboarding, because a modify changes the IAM Identity Center managed
/// application that onboarding minted, while a read only reports the
/// registration and a delete only removes it. Like
/// [`QEV2_IDC_APPLICATION_NOT_EXISTS`] it carries nothing from the request.
pub const QEV2_IDC_APPLICATION_NOT_MODIFIABLE: &str =
    "The Qev2IdcApplication you specified doesn't exist or the status isn't Completed.";

/// The ARN of a Query Editor (QEV2) IAM Identity Center application:
/// `arn:aws:redshift:<region>:<account>:qev2idcapplication:<uuid>`.
///
/// Derived from the application's own name and Region, so the value is stable
/// across every read of the same application and is not a per-run value; it is
/// computed ONCE, by the create, and then persisted on the stored application.
///
/// An application deleted and registered again under the same name therefore
/// gets the same ARN, which is intentional: the ARN is content-addressed from
/// the resource's own identity, exactly as [`cluster_arn`] and
/// [`subnet_group_arn`] are, and a name is all an application's identity is.
pub fn qev2_idc_application_arn(application_name: &str, region: &str) -> String {
    let uuid = content_uuid(&format!("qev2idcapplication/{application_name}/{region}"));
    engine::emulator_arn("redshift", region, &format!("qev2idcapplication:{uuid}"))
}

/// The ARN of the IAM Identity Center MANAGED application a registered Query
/// Editor (QEV2) application is backed by:
/// `arn:aws:sso::<account>:application/<instance id>/apl-<token>`.
///
/// IAM Identity Center is a global service, so the ARN carries no Region. The
/// instance segment is the instance the request named (the last path segment of
/// its ARN, which is where an IAM Identity Center instance ARN carries its
/// `ssoins-…` id) and the application segment is derived from that instance and
/// the application's name, so the value is stable across reads rather than
/// per-run.
pub fn idc_managed_application_arn(idc_instance_arn: &str, application_name: &str) -> String {
    let instance = idc_instance_arn
        .split('/')
        .next_back()
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            format!(
                "ssoins-{}",
                short_digest(&format!("idc-instance/{idc_instance_arn}"), 16)
            )
        });
    let token = short_digest(
        &format!("idc-managed-application/{idc_instance_arn}/{application_name}"),
        16,
    );
    engine::emulator_arn(
        "sso",
        "",
        &format!("application/{instance}/apl-{token}"),
    )
}

/// The longest an ARN this service STORES may be.
///
/// An ARN-valued member the model constrains only by length (`IdcInstanceArn`
/// is modelled `len 0..2147483647`) would otherwise let one request store a
/// two-gigabyte string, so a value longer than an AWS ARN can be is refused
/// rather than persisted. 2048 is the published maximum length of an AWS ARN
/// (the limit AWS documents for an ARN-valued field, e.g. an IAM role ARN), so
/// no ARN the real service accepts is refused by this bound.
pub const MAX_ARN_LENGTH: usize = 2048;

/// Whether a value is an AWS ARN at all: `arn:`, a non-empty partition and
/// service, and a non-empty resource portion, within [`MAX_ARN_LENGTH`].
///
/// A general check on the FORM of an ARN, not on which resource it names: it
/// says nothing about the partition, Region, account or service a particular
/// member requires. It exists because a member the model constrains only by
/// length can still be documented as an ARN (`smithy_query(query="doc",
/// shape="CreateQev2IdcApplicationMessage", member="IdcInstanceArn")` — "The
/// Amazon Resource Name (ARN) of the IAM Identity Center instance"), so a value
/// that is no ARN names no resource and is refused before anything is stored.
pub fn is_arn(value: &str) -> bool {
    if value.len() > MAX_ARN_LENGTH {
        return false;
    }
    let mut segments = value.split(':');
    segments.next() == Some("arn")
        && segments.next().is_some_and(|partition| !partition.is_empty())
        && segments.next().is_some_and(|service| !service.is_empty())
        // region, account and the resource portion: an ARN has at least six
        // colon-separated segments, and the resource portion is never empty.
        && segments.next().is_some()
        && segments.next().is_some()
        && segments.next().is_some_and(|resource| !resource.is_empty())
}

/// What a handler answers when the shared store cannot be reached: the service's
/// own modelled "a dependent internal service is temporarily unavailable" fault.
pub const STORE_UNAVAILABLE: &str =
    "the Amazon Redshift control-plane store is temporarily unavailable";

/// A cluster as this service stores it: the modelled projection a response is
/// built from, plus what the model does not carry.
#[derive(Debug, Clone)]
pub struct StoredCluster {
    /// Exactly what `DescribeClusters` reports, so a read never recomputes it.
    pub cluster: Cluster,
    /// `single-node` or `multi-node`. The `Cluster` shape models no
    /// `ClusterType` member, but `ModifyCluster` accepts one, so the resolved
    /// value is kept here.
    pub cluster_type: String,
    /// The admin password, stored so a later `ModifyCluster` can replace it and
    /// NEVER placed on [`StoredCluster::cluster`] — the `Cluster` shape models
    /// no password member, so it can never reach the wire.
    pub master_user_password: Option<String>,
    /// When the clock-owed changes the last call against this cluster started
    /// fall due: the `creating` -> `available` transition, the admin password a
    /// create or a modify asked for, and the `adding` / `removing` IAM role
    /// associations a `ModifyClusterIamRoles` reported as in progress.
    pub available_at: Instant,
}

/// A cluster parameter group as this service stores it.
#[derive(Debug, Clone)]
pub struct StoredParameterGroup {
    /// Exactly what `DescribeClusterParameterGroups` reports, so a read never
    /// recomputes it.
    pub group: ClusterParameterGroup,
    /// Whether Redshift itself owns the group (a DEFAULT parameter group, which
    /// every account has for each engine version) rather than the account having
    /// created it. A default group is outside [`PARAMETER_GROUP_QUOTA`] and its
    /// family is one of the valid `ParameterGroupFamily` values.
    pub default_group: bool,
    /// The parameter values a caller SET on this group, by parameter name.
    ///
    /// ONLY the overrides: a parameter the group never changed reports its
    /// family's engine default (see [`family_parameters`]), so a stored group is
    /// never a copy of the catalog and a group of a family whose catalog grows
    /// reports the new settings too.
    pub parameter_values: BTreeMap<String, String>,
}

impl StoredParameterGroup {
    /// The parameters this group reports, in the catalog's `ParameterName`
    /// order: its family's engine defaults, each carrying the value this group
    /// holds for it and `user` as its `Source` where the group changed it.
    pub fn parameters(&self) -> Vec<Parameter> {
        let minimum_engine_version = minimum_engine_version();
        family_parameters(self.group.parameter_group_family.as_deref().unwrap_or_default())
            .iter()
            .map(|parameter| {
                parameter.to_api(
                    self.parameter_values.get(parameter.name).map(String::as_str),
                    minimum_engine_version.as_deref(),
                )
            })
            .collect()
    }

    /// The catalog entry for the parameter `name` of this group's family, or
    /// `None` when the family publishes no such parameter — which is what makes
    /// a request naming one a request naming nothing this group holds.
    pub fn family_parameter(&self, name: &str) -> Option<&'static EngineParameter> {
        family_parameters(self.group.parameter_group_family.as_deref().unwrap_or_default())
            .iter()
            .find(|parameter| parameter.name == name)
    }
}

/// A snapshot as this service stores it.
#[derive(Debug, Clone)]
pub struct StoredSnapshot {
    /// Exactly what `DescribeClusterSnapshots` reports.
    pub snapshot: Snapshot,
    /// When the `creating` -> `available` transition falls due.
    pub available_at: Instant,
    /// The cluster the snapshot was taken OF, as it stood at that instant —
    /// which is what `RestoreFromClusterSnapshot` rebuilds a cluster from.
    ///
    /// Captured when the snapshot is taken rather than read off the source
    /// cluster at restore time, because "a snapshot contains a copy of the
    /// cluster data as of this exact time"
    /// (`smithy_query(query="doc", shape="Snapshot", member="SnapshotCreateTime")`):
    /// the source cluster may have been modified, renamed or deleted since, and
    /// a restore must still rebuild the configuration of that instant. It also
    /// carries what the `Snapshot` shape models no member for (the admin
    /// password, the maintenance window, the retention periods), so the restored
    /// cluster can use "the admin credentials the cluster had at the time the
    /// snapshot was taken".
    ///
    /// `None` for a snapshot record no `CreateClusterSnapshot` / `DeleteCluster`
    /// final snapshot minted (one seeded straight into the store): a restore
    /// from it rebuilds from the modelled [`StoredSnapshot::snapshot`] members
    /// and this service's documented defaults alone.
    pub source: Option<StoredCluster>,
}

/// An HSM configuration as this service stores it: the modelled projection a
/// response is built from, plus the two request values the `HsmConfiguration`
/// shape models no member for.
#[derive(Debug, Clone)]
pub struct StoredHsmConfiguration {
    /// Exactly what `DescribeHsmConfigurations` reports, so a read never
    /// recomputes it.
    pub configuration: HsmConfiguration,
    /// The HSM's public certificate file, kept verbatim — a cluster configured
    /// with this HSM connects to the HSM with it.
    ///
    /// Held here and NEVER on [`StoredHsmConfiguration::configuration`]: the
    /// `HsmConfiguration` shape models no certificate member, so it can never
    /// reach the wire, exactly as [`StoredCluster::master_user_password`] holds
    /// a cluster's admin password.
    pub hsm_server_public_certificate: String,
    /// The password that unlocks the HSM partition, held for the same reason and
    /// in the same place as [`StoredHsmConfiguration::hsm_server_public_certificate`].
    pub hsm_partition_password: String,
}

/// Per-service state for com-amazonaws-redshift-redshiftserviceversion20121201. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsRedshiftRedshiftserviceversion20121201State {
    /// Every provisioned cluster, keyed by `ClusterIdentifier`, in creation order.
    pub clusters: Vec<StoredCluster>,
    /// Every cluster subnet group, keyed by `ClusterSubnetGroupName`, in creation
    /// order.
    ///
    /// The modelled shape IS the whole record — a subnet group carries no status
    /// the clock moves and nothing the `ClusterSubnetGroup` shape has no member
    /// for — so unlike the collections around it this one needs no `Stored*`
    /// wrapper.
    pub subnet_groups: Vec<ClusterSubnetGroup>,
    /// Every snapshot, keyed by `SnapshotIdentifier`, in creation order.
    pub snapshots: Vec<StoredSnapshot>,
    /// Every cluster parameter group, keyed by `ParameterGroupName`, in creation
    /// order — the default parameter groups first, then the ones the account
    /// created.
    pub parameter_groups: Vec<StoredParameterGroup>,
    /// Every usage limit, keyed by `UsageLimitId`, in creation order.
    ///
    /// The modelled `UsageLimit` shape (generated under the name
    /// [`CreateUsageLimitOutput`], because `CreateUsageLimit` returns it
    /// unwrapped) IS the whole record — a usage limit carries no status the clock
    /// moves and nothing the shape has no member for — so, like
    /// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::subnet_groups`],
    /// this collection needs no `Stored*` wrapper.
    pub usage_limits: Vec<CreateUsageLimitOutput>,
    /// Every HSM configuration, keyed by `HsmConfigurationIdentifier`, in
    /// creation order.
    pub hsm_configurations: Vec<StoredHsmConfiguration>,
    /// Every Query Editor (QEV2) IAM Identity Center application the account
    /// registered, keyed by `Qev2IdcApplicationArn`, in creation order.
    ///
    /// The modelled `Qev2IdcApplication` shape IS the whole record — the
    /// application carries no status the clock moves (see
    /// [`QEV2_IDC_ONBOARD_STATUS`]) and nothing the shape has no member for —
    /// so, like [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::usage_limits`],
    /// this collection needs no `Stored*` wrapper.
    pub qev2_idc_applications: Vec<Qev2IdcApplication>,
    /// The engine versions this service offers, in catalog order. Unlike the
    /// collections above this one is SEEDED when the store is created — the
    /// versions exist before any cluster does, which is why
    /// `DescribeClusterVersions` can be called "even before creating any
    /// clusters" — and no operation adds to or removes from it.
    pub cluster_versions: Vec<ClusterVersion>,
    /// The maintenance tracks this service publishes, in catalog order. SEEDED
    /// when the store is created, exactly like
    /// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::cluster_versions`]:
    /// the tracks a cluster can be assigned to exist before any cluster does,
    /// and no operation adds to or removes from the list.
    pub maintenance_tracks: Vec<MaintenanceTrack>,
    /// The account attributes this service publishes, in catalog order. SEEDED
    /// when the store is created, exactly like
    /// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::maintenance_tracks`]:
    /// an attribute is an allowance the account already has before it creates
    /// anything, and no operation adds to or removes from the list.
    pub account_attributes: Vec<AccountAttribute>,
    /// The reserved node offerings this service publishes, in catalog order.
    /// SEEDED when the store is created, exactly like
    /// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::account_attributes`]:
    /// an offering is purchasable before the account reserves anything, and no
    /// operation adds to or removes from the list (see
    /// [`published_reserved_node_offerings`]).
    pub reserved_node_offerings: Vec<ReservedNodeOffering>,
}

/// Where in the store one resource an ARN names sits: its collection and its
/// position in it.
///
/// One resolution serves both the read and the write path (see
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::resource_slot`]),
/// so a tag read and a tag write can never resolve the same ARN differently.
#[derive(Debug, Clone, Copy)]
enum ResourceSlot {
    Cluster(usize),
    Snapshot(usize),
    ParameterGroup(usize),
    SubnetGroup(usize),
    HsmConfiguration(usize),
}

/// A fresh store already holds what exists before any caller writes anything:
/// the published engine-version catalog and, for each of its versions, the
/// DEFAULT parameter group of that version's family.
///
/// Source: `smithy_query(query="doc", operation="DescribeClusterParameterGroups")`
/// — "Returns a list of Amazon Redshift parameter groups, including parameter
/// groups you created and the default parameter group": the default group exists
/// without anybody creating it, so it is part of an empty store rather than
/// something a read mints.
impl Default for ComAmazonawsRedshiftRedshiftserviceversion20121201State {
    fn default() -> Self {
        Self {
            clusters: Vec::new(),
            subnet_groups: Vec::new(),
            snapshots: Vec::new(),
            parameter_groups: default_parameter_groups(),
            usage_limits: Vec::new(),
            hsm_configurations: Vec::new(),
            qev2_idc_applications: Vec::new(),
            cluster_versions: published_cluster_versions(),
            maintenance_tracks: published_maintenance_tracks(),
            account_attributes: published_account_attributes(),
            reserved_node_offerings: published_reserved_node_offerings(),
        }
    }
}

impl ComAmazonawsRedshiftRedshiftserviceversion20121201State {
    /// Materialise every transition the clock already owes at `now`.
    ///
    /// This is the ONLY write a reporting operation makes: the move from
    /// `creating` to `available` was due at `now` whether or not anybody called,
    /// so applying it on read changes nothing a later caller could not already
    /// have observed.
    pub fn settle(&mut self, now: Instant) {
        // The wall clock the derived timestamps are measured against: a
        // maintenance window whose start has passed is DUE to roll on to the
        // next week whether or not anybody called.
        let wall = ::aws_smithy_types::DateTime::from(SystemTime::now());
        for stored in &mut self.clusters {
            // A cluster the service is still working on — bringing it up, or
            // applying a change to it — is available again once that work's span
            // has passed, whether or not anybody called.
            let in_progress = stored
                .cluster
                .cluster_status
                .as_deref()
                .is_some_and(|status| status == STATUS_CREATING || status == STATUS_MODIFYING);
            if in_progress && now >= stored.available_at {
                stored.cluster.cluster_status = Some(STATUS_AVAILABLE.to_string());
                stored.cluster.cluster_availability_status =
                    Some(availability_status(STATUS_AVAILABLE).to_string());
                // A cluster a restore created reports that restore's progress,
                // and the restore is done exactly when the cluster it was
                // rebuilding is available.
                if let Some(restore) = stored.cluster.restore_status.as_mut() {
                    restore.status = Some(RESTORE_STATUS_COMPLETED.to_string());
                }
            }
            // The admin password a create or a modify asked for is applied once
            // that call's span has passed, so from then on nothing is pending on
            // it: the change was due whether or not anybody called.
            if now >= stored.available_at {
                if let Some(pending) = stored.cluster.pending_modified_values.as_mut() {
                    pending.master_user_password = None;
                }
                // An IAM role association reported as in progress completes once
                // that call's span has passed: a role being added becomes
                // available for use by the cluster and a role being removed
                // leaves the list. Both were due at this instant whether or not
                // anybody called.
                if let Some(roles) = stored.cluster.iam_roles.as_mut() {
                    roles.retain(|role| {
                        role.apply_status.as_deref() != Some(IAM_ROLE_APPLY_STATUS_REMOVING)
                    });
                    for role in roles.iter_mut() {
                        if role.apply_status.as_deref() == Some(IAM_ROLE_APPLY_STATUS_ADDING) {
                            role.apply_status = Some(IAM_ROLE_APPLY_STATUS_IN_SYNC.to_string());
                        }
                    }
                }
            }
            // The next maintenance window start is derived from the cluster's
            // own window, so once that instant is past the next one is the week
            // after: recomputing it materialises what the clock already owed and
            // shows nothing a later caller could not have seen.
            let passed = stored
                .cluster
                .next_maintenance_window_start_time
                .is_some_and(|start| start.secs() <= wall.secs());
            if passed {
                if let Some(window) = stored.cluster.preferred_maintenance_window.as_deref() {
                    stored.cluster.next_maintenance_window_start_time =
                        next_maintenance_window_start(window, wall);
                }
            }
        }
        for stored in &mut self.snapshots {
            if stored.snapshot.status.as_deref() == Some(STATUS_CREATING)
                && now >= stored.available_at
            {
                stored.snapshot.status = Some(STATUS_AVAILABLE.to_string());
            }
            // How many days are left of a snapshot's manual retention period is
            // derived from that period and the snapshot's own creation time, so
            // it falls with the clock whether or not anybody called.
            refresh_manual_retention(&mut stored.snapshot, wall);
        }
    }

    /// The stored cluster with this identifier, if any.
    pub fn cluster(&self, identifier: &str) -> Option<&StoredCluster> {
        self.clusters
            .iter()
            .find(|c| c.cluster.cluster_identifier.as_deref() == Some(identifier))
    }

    /// The account attributes whose names `requested` asks for, in catalog
    /// order — the WHOLE catalog when it asks for none.
    ///
    /// One rule for every caller: a request names the attributes it wants
    /// reported, so an attribute the account carries no such name for
    /// contributes nothing, and naming the same attribute twice still reports
    /// it once.
    pub fn account_attributes_named(&self, requested: &[String]) -> Vec<AccountAttribute> {
        self.account_attributes
            .iter()
            .filter(|attribute| {
                requested.is_empty()
                    || requested
                        .iter()
                        .any(|name| Some(name.as_str()) == attribute.attribute_name.as_deref())
            })
            .cloned()
            .collect()
    }

    /// The published reserved node offering with this `ReservedNodeOfferingId`,
    /// if the catalog has one.
    pub fn reserved_node_offering(&self, offering_id: &str) -> Option<&ReservedNodeOffering> {
        self.reserved_node_offerings
            .iter()
            .find(|offering| offering.reserved_node_offering_id.as_deref() == Some(offering_id))
    }

    /// The published maintenance track with this name, if the catalog has one.
    pub fn maintenance_track(&self, name: &str) -> Option<&MaintenanceTrack> {
        self.maintenance_tracks
            .iter()
            .find(|track| track.maintenance_track_name.as_deref() == Some(name))
    }

    /// The stored usage limit with this `UsageLimitId`, if any.
    pub fn usage_limit(&self, usage_limit_id: &str) -> Option<&CreateUsageLimitOutput> {
        self.usage_limits
            .iter()
            .find(|limit| limit.usage_limit_id.as_deref() == Some(usage_limit_id))
    }

    /// The position of the usage limit with this `UsageLimitId`, if any.
    pub fn usage_limit_position(&self, usage_limit_id: &str) -> Option<usize> {
        self.usage_limits
            .iter()
            .position(|limit| limit.usage_limit_id.as_deref() == Some(usage_limit_id))
    }

    /// The stored HSM configuration with this `HsmConfigurationIdentifier`, if
    /// any.
    ///
    /// The identifier is compared exactly as the request that created the record
    /// spelled it: the model documents no folding for it
    /// (`smithy_query(query="doc", shape="CreateHsmConfigurationMessage",
    /// member="HsmConfigurationIdentifier")` — "The identifier to be assigned to
    /// the new Amazon Redshift HSM configuration"), so nothing here invents one.
    pub fn hsm_configuration(&self, identifier: &str) -> Option<&StoredHsmConfiguration> {
        self.hsm_configurations.iter().find(|stored| {
            stored.configuration.hsm_configuration_identifier.as_deref() == Some(identifier)
        })
    }

    /// The position of the registered QEV2 IAM Identity Center application with
    /// this `Qev2IdcApplicationArn`, if any.
    ///
    /// A position rather than a reference, because every caller of it — the
    /// modify and the delete — needs to write the record it finds; the read path
    /// selects by reference while it walks the collection for a page.
    pub fn qev2_idc_application_position(&self, application_arn: &str) -> Option<usize> {
        self.qev2_idc_applications
            .iter()
            .position(|application| {
                application.qev2_idc_application_arn.as_deref() == Some(application_arn)
            })
    }

    /// The position of the registered QEV2 IAM Identity Center application a
    /// MODIFY may act on: the ARN must name one, and its onboarding must have
    /// finished ([`QEV2_IDC_ONBOARD_STATUS`]), since a modify changes the IAM
    /// Identity Center managed application that onboarding minted. A read
    /// reports an application whatever its onboarding status and a delete
    /// removes it whatever its status — which is what makes the status
    /// observable at all — so neither takes this narrower predicate.
    ///
    /// No implemented operation mints a half-onboarded application: a create's
    /// application carries [`QEV2_IDC_ONBOARD_STATUS`] by the time the call
    /// returns, so today only a record seeded straight into the store reaches
    /// the refusing branch.
    pub fn modifiable_qev2_idc_application_position(
        &self,
        application_arn: &str,
    ) -> Option<usize> {
        self.qev2_idc_application_position(application_arn)
            .filter(|position| {
                self.qev2_idc_applications[*position]
                    .idc_onboard_status
                    .as_deref()
                    == Some(QEV2_IDC_ONBOARD_STATUS)
            })
    }

    /// The registered QEV2 IAM Identity Center application with this
    /// `Qev2IdcApplicationName`, if any — the name uniqueness is decided on,
    /// since the modelled fault tells a caller to "use a different application
    /// name".
    pub fn qev2_idc_application_named(&self, name: &str) -> Option<&Qev2IdcApplication> {
        self.qev2_idc_applications
            .iter()
            .find(|application| application.qev2_idc_application_name.as_deref() == Some(name))
    }

    /// The position of the cluster with this identifier, if any.
    pub fn cluster_position(&self, identifier: &str) -> Option<usize> {
        self.clusters
            .iter()
            .position(|c| c.cluster.cluster_identifier.as_deref() == Some(identifier))
    }

    /// The stored snapshot with this identifier, if any.
    pub fn snapshot(&self, identifier: &str) -> Option<&StoredSnapshot> {
        self.snapshots
            .iter()
            .find(|s| s.snapshot.snapshot_identifier.as_deref() == Some(identifier))
    }

    /// The stored snapshot an `Identifier`-or-`Arn` pair names, if any.
    ///
    /// The identifier is matched case-INSENSITIVELY: `smithy_query(query="doc",
    /// shape="RestoreFromClusterSnapshotMessage", member="SnapshotIdentifier")`
    /// — "This parameter isn't case sensitive." The ARN is matched exactly,
    /// against the `Snapshot$SnapshotArn` the store persists. Naming neither
    /// names no snapshot.
    pub fn snapshot_located(
        &self,
        identifier: Option<&str>,
        snapshot_arn: Option<&str>,
    ) -> Option<&StoredSnapshot> {
        self.snapshot_located_position(identifier, snapshot_arn)
            .map(|index| &self.snapshots[index])
    }

    /// The POSITION of the snapshot an `Identifier`-or-`Arn` pair names — the
    /// same match [`Self::snapshot_located`] makes, for an operation that has to
    /// WRITE the record it found.
    ///
    /// EVERY locator the request carries must match the one record: a request
    /// naming an identifier and an ARN that belong to different snapshots names
    /// neither.
    pub fn snapshot_located_position(
        &self,
        identifier: Option<&str>,
        snapshot_arn: Option<&str>,
    ) -> Option<usize> {
        // Naming neither names no snapshot — without this guard every locator
        // would match vacuously and the first stored record would answer.
        if identifier.is_none() && snapshot_arn.is_none() {
            return None;
        }
        self.snapshots.iter().position(|s| {
            identifier.is_none_or(|wanted| {
                s.snapshot
                    .snapshot_identifier
                    .as_deref()
                    .is_some_and(|stored| stored.eq_ignore_ascii_case(wanted))
            }) && snapshot_arn
                .is_none_or(|wanted| s.snapshot.snapshot_arn.as_deref() == Some(wanted))
        })
    }

    /// The stored cluster subnet group with this name, if any.
    ///
    /// The name is compared as stored: a group's name is held in the documented
    /// lower-case form (see [`normalized_subnet_group_name`]), so the caller's
    /// spelling is folded before it is looked up.
    pub fn subnet_group(&self, name: &str) -> Option<&ClusterSubnetGroup> {
        let wanted = normalized_subnet_group_name(name);
        self.subnet_groups
            .iter()
            .find(|g| g.cluster_subnet_group_name.as_deref() == Some(wanted.as_str()))
    }

    /// The account's DEFAULT cluster subnet group, materialised into this store
    /// the first time a cluster needs it.
    ///
    /// An account has this group before anybody creates one (see
    /// [`default_subnet_group`]), so a cluster that named no subnet group — and a
    /// request that names `default` — must find it. It is materialised here
    /// rather than seeded with the store because its subnets carry the
    /// Availability Zones of a REGION, which only a request carries; the record
    /// is inserted once and every later call reads that same record.
    pub fn ensure_default_subnet_group(&mut self, region: &str) -> ClusterSubnetGroup {
        if let Some(existing) = self.subnet_group(DEFAULT_CLUSTER_SUBNET_GROUP) {
            return existing.clone();
        }
        let group = default_subnet_group(region);
        self.subnet_groups.push(group.clone());
        group
    }

    /// The total storage the snapshots this store holds occupy, in megabytes.
    ///
    /// Source: `smithy_query(query="doc", shape="CustomerStorageMessage",
    /// member="TotalBackupSizeInMegaBytes")` — "The total amount of storage
    /// currently used for snapshots": the sum of each stored snapshot's own
    /// `Snapshot$TotalBackupSizeInMegaBytes` ("The size of the complete set of
    /// backup data that would be used to restore the cluster"), so deleting a
    /// snapshot lowers it with no separate bookkeeping. A snapshot whose record
    /// carries no size contributes nothing.
    pub fn total_backup_size_in_mega_bytes(&self) -> f64 {
        // `fold` from `+0.0` rather than `sum()`: the standard library's `Sum`
        // for `f64` folds from `-0.0`, so an empty store would report negative
        // zero — a value that renders as `-0.0` and is not what "nothing is
        // backed up" is.
        self.snapshots
            .iter()
            .filter_map(|stored| stored.snapshot.total_backup_size_in_mega_bytes)
            .fold(0.0, |total, size| total + size)
    }

    /// The total storage the clusters this store holds have provisioned, in
    /// megabytes.
    ///
    /// Source: `smithy_query(query="doc", shape="CustomerStorageMessage",
    /// member="TotalProvisionedStorageInMegaBytes")` — "The total amount of
    /// storage currently provisioned": the sum of each stored cluster's own
    /// `Cluster$TotalStorageCapacityInMegaBytes` ("The total storage capacity of
    /// the cluster in megabytes"), so creating or deleting a cluster moves it
    /// with no separate bookkeeping. A cluster whose record carries no capacity
    /// contributes nothing.
    pub fn total_provisioned_storage_in_mega_bytes(&self) -> f64 {
        // Folded from `+0.0` for the same reason as
        // [`Self::total_backup_size_in_mega_bytes`].
        self.clusters
            .iter()
            .filter_map(|stored| stored.cluster.total_storage_capacity_in_mega_bytes)
            .fold(0.0, |total, capacity| total + capacity as f64)
    }

    /// The stored cluster parameter group with this name, if any.
    ///
    /// The name is compared as stored: a group's name is held in the documented
    /// lower-case form (see [`normalized_parameter_group_name`]), so a caller
    /// naming it in any other case names no group.
    pub fn parameter_group(&self, name: &str) -> Option<&StoredParameterGroup> {
        self.parameter_groups
            .iter()
            .find(|g| g.group.parameter_group_name.as_deref() == Some(name))
    }

    /// The position of the cluster parameter group with this name, if any.
    ///
    /// The name is compared exactly as [`Self::parameter_group`] compares it.
    pub fn parameter_group_position(&self, name: &str) -> Option<usize> {
        self.parameter_groups
            .iter()
            .position(|g| g.group.parameter_group_name.as_deref() == Some(name))
    }

    /// The `ParameterGroupFamily` values a request may name.
    ///
    /// Source: `smithy_query(query="doc", shape="CreateClusterParameterGroupMessage",
    /// member="ParameterGroupFamily")` — "The parameter group family names
    /// associated with the default parameter groups provide you the valid
    /// values." So the catalogue is read off the DEFAULT groups the store holds,
    /// never hard-coded at the call site.
    pub fn parameter_group_families(&self) -> Vec<&str> {
        let mut families: Vec<&str> = Vec::new();
        for family in self
            .parameter_groups
            .iter()
            .filter(|g| g.default_group)
            .filter_map(|g| g.group.parameter_group_family.as_deref())
        {
            if !families.contains(&family) {
                families.push(family);
            }
        }
        families
    }

    /// How many cluster parameter groups the ACCOUNT created, which is what
    /// [`PARAMETER_GROUP_QUOTA`] bounds.
    pub fn owned_parameter_group_count(&self) -> usize {
        self.parameter_groups
            .iter()
            .filter(|g| !g.default_group)
            .count()
    }

    /// Where in this store the resource one `ResourceName` ARN names sits, or
    /// `None` when it holds no such resource.
    ///
    /// The tags of a resource live ON that resource — the `Tags` member every one
    /// of these shapes already publishes — so tagging one is a write to the
    /// record a `Describe*` of it reads, and there is no second place a tag could
    /// be. Five of the published types ([`TAGGABLE_RESOURCE_TYPES`]) name a
    /// resource this store holds; a resource of any other type exists in no
    /// account here, because the operations that would create one are still
    /// scaffolds.
    ///
    /// The ARN is matched EXACTLY, against the ARN the resource itself publishes
    /// ([`Self::resource_entry`]) — the same way this store already locates a
    /// snapshot by ARN ([`Self::snapshot_located`]: "the ARN is matched exactly,
    /// against the `Snapshot$SnapshotArn` the store persists"). So nothing here
    /// takes an ARN apart: a value that is not this account's ARN for a resource
    /// IN THIS REGION names no resource, which is what an ARN means — its
    /// partition, Region and account fields say whose resource it is and which
    /// endpoint owns it.
    fn resource_slot(&self, resource_name: &str, region: &str) -> Option<ResourceSlot> {
        self.resource_slots().into_iter().find(|slot| {
            self.resource_entry(*slot, region)
                .is_some_and(|(_, arn, _)| arn == resource_name)
        })
    }

    /// The tags the resource `resource_name` names carries, or `None` when this
    /// store holds no such resource — which is what tells a tag read apart from a
    /// read of a resource that does not exist. See [`Self::resource_slot`].
    pub fn resource_tags(
        &self,
        resource_name: &str,
        region: &str,
    ) -> Option<&[crate::ops::types::Tag]> {
        let tags = match self.resource_slot(resource_name, region)? {
            ResourceSlot::Cluster(position) => &self.clusters[position].cluster.tags,
            ResourceSlot::Snapshot(position) => &self.snapshots[position].snapshot.tags,
            ResourceSlot::ParameterGroup(position) => &self.parameter_groups[position].group.tags,
            ResourceSlot::SubnetGroup(position) => &self.subnet_groups[position].tags,
            ResourceSlot::HsmConfiguration(position) => {
                &self.hsm_configurations[position].configuration.tags
            }
        };
        Some(tags.as_deref().unwrap_or_default())
    }

    /// Write `tags` as the tag list of the resource `resource_name` names, and
    /// report whether this store holds such a resource at all. See
    /// [`Self::resource_slot`].
    ///
    /// Takes the new list rather than handing out a `&mut`, so the record is
    /// reached ONCE for a write and an empty list leaves a resource that carries
    /// no `Tags` member exactly as it was.
    pub fn set_resource_tags(
        &mut self,
        resource_name: &str,
        region: &str,
        tags: Vec<crate::ops::types::Tag>,
    ) -> bool {
        let Some(slot) = self.resource_slot(resource_name, region) else {
            return false;
        };
        let stored = match slot {
            ResourceSlot::Cluster(position) => &mut self.clusters[position].cluster.tags,
            ResourceSlot::Snapshot(position) => &mut self.snapshots[position].snapshot.tags,
            ResourceSlot::ParameterGroup(position) => {
                &mut self.parameter_groups[position].group.tags
            }
            ResourceSlot::SubnetGroup(position) => &mut self.subnet_groups[position].tags,
            ResourceSlot::HsmConfiguration(position) => {
                &mut self.hsm_configurations[position].configuration.tags
            }
        };
        // A resource that carries no `Tags` member and is given no tag keeps the
        // member absent: writing `Some([])` would change what its own
        // `Describe*` renders without changing a tag.
        if tags.is_empty() && stored.is_none() {
            return true;
        }
        *stored = Some(tags);
        true
    }

    /// Every taggable resource this store holds, in the order
    /// `DescribeTags` walks them: the resource types in the order the model
    /// lists them ([`TAGGABLE_RESOURCE_TYPES`]), and the resources of one type
    /// in the store's stable insertion order.
    ///
    /// A type whose resources no implemented operation mints contributes none:
    /// this account holds no such resource.
    fn resource_slots(&self) -> Vec<ResourceSlot> {
        let mut slots = Vec::new();
        for published in &TAGGABLE_RESOURCE_TYPES {
            match published.arn_segment {
                ARN_SEGMENT_CLUSTER => {
                    slots.extend((0..self.clusters.len()).map(ResourceSlot::Cluster))
                }
                ARN_SEGMENT_SNAPSHOT => {
                    slots.extend((0..self.snapshots.len()).map(ResourceSlot::Snapshot))
                }
                ARN_SEGMENT_PARAMETER_GROUP => slots
                    .extend((0..self.parameter_groups.len()).map(ResourceSlot::ParameterGroup)),
                ARN_SEGMENT_SUBNET_GROUP => {
                    slots.extend((0..self.subnet_groups.len()).map(ResourceSlot::SubnetGroup))
                }
                ARN_SEGMENT_HSM_CONFIGURATION => slots.extend(
                    (0..self.hsm_configurations.len()).map(ResourceSlot::HsmConfiguration),
                ),
                _ => {}
            }
        }
        slots
    }

    /// What one stored resource contributes to a tag report: its type's ARN
    /// segment, its ARN and the tags it carries.
    ///
    /// `region` is the Region the reading request was signed for, which is the
    /// Region of the ARN this builds — a stored resource's ARN is derived from
    /// its identifier (see [`cluster_arn`]), except a snapshot, which publishes
    /// its own `Snapshot$SnapshotArn`.
    fn resource_entry(
        &self,
        slot: ResourceSlot,
        region: &str,
    ) -> Option<(&'static str, String, &[crate::ops::types::Tag])> {
        match slot {
            ResourceSlot::Cluster(position) => {
                let stored = &self.clusters[position];
                Some((
                    ARN_SEGMENT_CLUSTER,
                    cluster_arn(stored.cluster.cluster_identifier.as_deref()?, region),
                    stored.cluster.tags.as_deref().unwrap_or_default(),
                ))
            }
            ResourceSlot::Snapshot(position) => {
                let stored = &self.snapshots[position];
                let arn = match stored.snapshot.snapshot_arn.as_deref() {
                    Some(arn) => arn.to_string(),
                    None => snapshot_arn(
                        stored.snapshot.cluster_identifier.as_deref()?,
                        stored.snapshot.snapshot_identifier.as_deref()?,
                        region,
                    ),
                };
                Some((
                    ARN_SEGMENT_SNAPSHOT,
                    arn,
                    stored.snapshot.tags.as_deref().unwrap_or_default(),
                ))
            }
            ResourceSlot::ParameterGroup(position) => {
                let stored = &self.parameter_groups[position];
                Some((
                    ARN_SEGMENT_PARAMETER_GROUP,
                    parameter_group_arn(stored.group.parameter_group_name.as_deref()?, region),
                    stored.group.tags.as_deref().unwrap_or_default(),
                ))
            }
            ResourceSlot::SubnetGroup(position) => {
                let group = &self.subnet_groups[position];
                Some((
                    ARN_SEGMENT_SUBNET_GROUP,
                    subnet_group_arn(group.cluster_subnet_group_name.as_deref()?, region),
                    group.tags.as_deref().unwrap_or_default(),
                ))
            }
            ResourceSlot::HsmConfiguration(position) => {
                let stored = &self.hsm_configurations[position];
                Some((
                    ARN_SEGMENT_HSM_CONFIGURATION,
                    hsm_configuration_arn(
                        stored.configuration.hsm_configuration_identifier.as_deref()?,
                        region,
                    ),
                    stored.configuration.tags.as_deref().unwrap_or_default(),
                ))
            }
        }
    }

    /// Every tag this store holds, as `DescribeTags` reports them: ONE entry per
    /// (resource, tag) pair, carrying the tag, the resource's ARN and the
    /// resource's type — narrowed to one resource type when the request names
    /// one, and to the resources `filter` selects.
    ///
    /// The entries follow [`Self::resource_slots`], with the tags of one resource
    /// in the order it carries them, so the `Marker` cursor a page hands out
    /// stays a resumable offset into one sequence.
    ///
    /// The `TagKeys` / `TagValues` filter selects RESOURCES, not single tags:
    /// `smithy_query(query="doc", shape="DescribeTagsMessage", member="TagKeys")`
    /// — "a tag key or keys for which you want to return all matching RESOURCES
    /// that are associated with the specified key or keys … Amazon Redshift
    /// returns a response with all resources that have either or both of these
    /// tag keys associated with them" — so a resource one of its tags matches
    /// reports EVERY tag it carries, not only the tag that matched.
    ///
    /// A resource carrying no tag contributes NOTHING: a `TaggedResource` is a
    /// tag paired with the resource it is on, so there is no entry to report for
    /// a resource that has none.
    pub fn tagged_resources(
        &self,
        resource_type: Option<&TaggableResourceType>,
        filter: &TagFilter<'_>,
        region: &str,
    ) -> Vec<crate::ops::types::TaggedResource> {
        self.resource_slots()
            .into_iter()
            .filter_map(|slot| self.resource_entry(slot, region))
            .filter(|(segment, _, tags)| {
                resource_type.is_none_or(|wanted| wanted.arn_segment == *segment)
                    && filter.matches(tags)
            })
            .flat_map(|(segment, arn, tags)| tagged_resource_entries(segment, arn, tags))
            .collect()
    }

    /// The tags of the ONE resource `resource` names, as `DescribeTags` reports
    /// them, or `None` when this store holds no such resource.
    ///
    /// `filter` selects the resource exactly as it does in
    /// [`Self::tagged_resources`]: a resource it does not select reports no tag,
    /// while a resource that does not EXIST is `None` — a refusal, not an empty
    /// report.
    pub fn tagged_resources_of(
        &self,
        resource_name: &str,
        filter: &TagFilter<'_>,
        region: &str,
    ) -> Option<Vec<crate::ops::types::TaggedResource>> {
        let slot = self.resource_slot(resource_name, region)?;
        let (segment, arn, tags) = self.resource_entry(slot, region)?;
        if !filter.matches(tags) {
            return Some(Vec::new());
        }
        Some(tagged_resource_entries(segment, arn, tags).collect())
    }

    /// Record, on every cluster configured with the parameter group `group_name`,
    /// that each of these parameters now carries a change the cluster has not
    /// seen.
    ///
    /// Source: `smithy_query(query="doc", shape="ClusterParameterStatus",
    /// member="ParameterApplyStatus")` — "`pending-reboot`: The parameter value
    /// will be applied after the cluster reboots" — with
    /// `smithy_query(query="doc", operation="ResetClusterParameterGroup")`, "For
    /// parameter changes to take effect you must reboot any associated clusters".
    /// ONE rule for every path that changes what a group holds (the modify and
    /// the reset), so the two cannot report different memberships for the same
    /// kind of change. Naming no parameter changes nothing: a change an
    /// associated cluster picks up without a reboot leaves nothing pending.
    pub fn mark_parameter_group_pending_reboot(
        &mut self,
        group_name: &str,
        parameter_names: &[String],
    ) {
        if parameter_names.is_empty() {
            return;
        }
        for stored in &mut self.clusters {
            let Some(memberships) = stored.cluster.cluster_parameter_groups.as_mut() else {
                continue;
            };
            for membership in memberships
                .iter_mut()
                .filter(|m| m.parameter_group_name.as_deref() == Some(group_name))
            {
                membership.parameter_apply_status =
                    Some(PARAMETER_APPLY_STATUS_PENDING_REBOOT.to_string());
                let statuses = membership
                    .cluster_parameter_status_list
                    .get_or_insert_with(Vec::new);
                for name in parameter_names {
                    match statuses
                        .iter_mut()
                        .find(|status| status.parameter_name.as_deref() == Some(name.as_str()))
                    {
                        Some(status) => {
                            status.parameter_apply_status =
                                Some(PARAMETER_APPLY_STATUS_PENDING_REBOOT.to_string());
                        }
                        None => statuses.push(ClusterParameterStatus {
                            parameter_name: Some(name.clone()),
                            parameter_apply_status: Some(
                                PARAMETER_APPLY_STATUS_PENDING_REBOOT.to_string(),
                            ),
                            parameter_apply_error_description: None,
                        }),
                    }
                }
            }
        }
    }
}

/// One `TaggedResource` per tag of one resource: the tag, the resource's ARN and
/// the resource's type, which is what `DescribeTags` reports a tag as.
fn tagged_resource_entries<'a>(
    resource_type: &'static str,
    arn: String,
    tags: &'a [crate::ops::types::Tag],
) -> impl Iterator<Item = crate::ops::types::TaggedResource> + 'a {
    tags.iter()
        .map(move |tag| crate::ops::types::TaggedResource {
            tag: Some(tag.clone()),
            resource_name: Some(arn.clone()),
            resource_type: Some(resource_type.to_string()),
        })
}

/// `ClusterAvailabilityStatus` for a `ClusterStatus`.
///
/// The two vocabularies both come from the model.
/// `smithy_query(query="doc", shape="Cluster", member="ClusterAvailabilityStatus")`
/// declares exactly five values: "Available - The cluster is available for
/// queries", "Unavailable - The cluster is not available for queries",
/// "Maintenance - … intermittently available … due to maintenance activities",
/// "Modifying - … intermittently available for queries due to changes that
/// modify the cluster" and "Failed - The cluster failed and is not available for
/// queries". `smithy_query(query="doc", shape="Cluster", member="ClusterStatus")`
/// declares the twenty `ClusterStatus` values this maps from, so the mapping is a
/// rule over the whole published vocabulary rather than a case per status:
///
/// * a status that STARTS with `available` ("available", "available,
///   prep-for-resize", "available, resize-cleanup") is `Available`;
/// * a status in which the service is changing the cluster — bringing it up,
///   modifying, resizing, cancelling a resize, rebooting, renaming, rotating its
///   keys, updating its HSM settings or taking its final snapshot — is
///   `Modifying`: the cluster is only intermittently available while it happens;
/// * a status the cluster cannot serve queries from because it broke
///   (`hardware-failure`, every `incompatible-…`) is `Failed`;
/// * every other published status (`deleting`, `paused`, `storage-full`) is
///   `Unavailable`.
///
/// No `ClusterStatus` reports scheduled maintenance, so no status maps to
/// `Maintenance`.
pub fn availability_status(cluster_status: &str) -> &'static str {
    if cluster_status.starts_with(STATUS_AVAILABLE) {
        return "Available";
    }
    match cluster_status {
        STATUS_CREATING | STATUS_MODIFYING | "resizing" | "cancelling-resize" | "rebooting"
        | "renaming" | "rotating-keys" | "updating-hsm" | "final-snapshot" => "Modifying",
        "hardware-failure" => "Failed",
        other if other.starts_with("incompatible-") => "Failed",
        _ => "Unavailable",
    }
}

/// `Cluster$AvailabilityZoneRelocationStatus` for the `AvailabilityZoneRelocation`
/// a request carried (or did not).
///
/// Source: `smithy_query(query="doc", shape="Cluster",
/// member="AvailabilityZoneRelocationStatus")` — "Describes the status of the
/// Availability Zone relocation operation" — paired with
/// `smithy_query(query="doc", shape="CreateClusterMessage",
/// member="AvailabilityZoneRelocation")`, "The option to enable relocation for an
/// Amazon Redshift cluster between Availability Zones after the cluster is
/// created": the member reports whether that option is on, so it is `enabled`
/// exactly when the request asked for relocation and `disabled` exactly when it
/// turned it off.
///
/// The model documents NO default for the option. Amazon Redshift reports
/// relocation `enabled` on a provisioned cluster whose request named the option
/// at all — see the recorded `DescribeClusters` response this emulator is
/// measured against — so [`RELOCATION_ENABLED_BY_DEFAULT`] is what a silent
/// request gets; a request that names the option is always honoured.
pub fn availability_zone_relocation_status(requested: Option<bool>) -> String {
    if requested.unwrap_or(RELOCATION_ENABLED_BY_DEFAULT) {
        "enabled"
    } else {
        "disabled"
    }
    .to_string()
}

/// Whether Availability Zone relocation is on for a cluster whose request did not
/// name the option. See [`availability_zone_relocation_status`].
pub const RELOCATION_ENABLED_BY_DEFAULT: bool = true;

/// The syntax rule Redshift documents for one identifier member.
#[derive(Debug, Clone, Copy)]
pub struct IdentifierRule {
    /// The input member the rule belongs to, named in the refusal.
    pub member: &'static str,
    /// The documented maximum length, in characters.
    pub max_len: usize,
    /// Whether the documented rule forbids uppercase letters.
    pub lowercase_only: bool,
}

/// The rule for a cluster identifier (`ClusterIdentifier`, `NewClusterIdentifier`).
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage",
/// member="ClusterIdentifier")` — "Must contain from 1 to 63 alphanumeric
/// characters or hyphens. Alphabetic characters must be lowercase. First
/// character must be a letter. Cannot end with a hyphen or contain two
/// consecutive hyphens."
pub const CLUSTER_IDENTIFIER_RULE: IdentifierRule = IdentifierRule {
    member: "ClusterIdentifier",
    max_len: 63,
    lowercase_only: true,
};

/// The rule for a cluster parameter group name (`ParameterGroupName`).
///
/// Source: `smithy_query(query="doc", shape="CreateClusterParameterGroupMessage",
/// member="ParameterGroupName")` — "Must be 1 to 255 alphanumeric characters or
/// hyphens First character must be a letter. Cannot end with a hyphen or contain
/// two consecutive hyphens." Mixed case is ACCEPTED and folded by
/// [`normalized_parameter_group_name`] rather than refused, because the same doc
/// says the value "is stored as a lower-case string".
pub const PARAMETER_GROUP_NAME_RULE: IdentifierRule = IdentifierRule {
    member: "ParameterGroupName",
    max_len: 255,
    lowercase_only: false,
};

/// The form a cluster parameter group name is STORED and reported in.
///
/// Source: the same doc sentence as [`PARAMETER_GROUP_NAME_RULE`] — "This value
/// is stored as a lower-case string." The rule admits only ASCII alphanumerics
/// and hyphens, so ASCII folding is the whole of it.
pub fn normalized_parameter_group_name(requested: &str) -> String {
    requested.to_ascii_lowercase()
}

/// The rule for a cluster subnet group name (`ClusterSubnetGroupName`).
///
/// Source: `smithy_query(query="doc", shape="CreateClusterSubnetGroupMessage",
/// member="ClusterSubnetGroupName")` — "Must contain no more than 255
/// alphanumeric characters or hyphens. Must not be "Default". Must be unique for
/// all subnet groups that are created by your Amazon Web Services account."
/// Mixed case is ACCEPTED and folded by [`normalized_subnet_group_name`] rather
/// than refused, because the same doc says "Amazon Redshift stores the value as a
/// lowercase string".
pub const SUBNET_GROUP_NAME_RULE: IdentifierRule = IdentifierRule {
    member: "ClusterSubnetGroupName",
    max_len: 255,
    lowercase_only: false,
};

/// The one name a cluster subnet group may not take, compared in the stored
/// (folded) form.
///
/// Source: the same doc sentence as [`SUBNET_GROUP_NAME_RULE`] — "Must not be
/// "Default"".
pub const RESERVED_SUBNET_GROUP_NAME: &str = "default";

/// The form a cluster subnet group name is STORED and reported in.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterSubnetGroupMessage",
/// member="ClusterSubnetGroupName")` — "Amazon Redshift stores the value as a
/// lowercase string." The rule admits only ASCII alphanumerics and hyphens, so
/// ASCII folding is the whole of it.
pub fn normalized_subnet_group_name(requested: &str) -> String {
    requested.to_ascii_lowercase()
}

/// The rule for a snapshot identifier (`SnapshotIdentifier`,
/// `FinalClusterSnapshotIdentifier`).
///
/// Source: `smithy_query(query="doc", shape="CreateClusterSnapshotMessage",
/// member="SnapshotIdentifier")` — "Must contain from 1 to 255 alphanumeric
/// characters or hyphens. First character must be a letter. Cannot end with a
/// hyphen or contain two consecutive hyphens." (No lowercase restriction.)
pub const SNAPSHOT_IDENTIFIER_RULE: IdentifierRule = IdentifierRule {
    member: "SnapshotIdentifier",
    max_len: 255,
    lowercase_only: false,
};

/// Check `value` against `rule`, naming the member and the constraint it broke.
///
/// `member` overrides the rule's own member name, so `NewClusterIdentifier` and
/// `FinalClusterSnapshotIdentifier` are refused under the same rule as the
/// member it was documented on, while still naming themselves.
pub fn check_identifier(rule: IdentifierRule, member: &str, value: &str) -> Result<(), String> {
    let refuse = |why: &str| {
        Err(format!(
            "Invalid value for {member}: '{value}'. {why} (allowed: 1 to {} {}alphanumeric characters or hyphens, first character a letter, no trailing hyphen and no two consecutive hyphens)",
            rule.max_len,
            if rule.lowercase_only { "lowercase " } else { "" },
        ))
    };
    if value.is_empty() {
        return refuse("The identifier cannot be null, empty, or blank.");
    }
    if value.chars().count() > rule.max_len {
        return refuse("The identifier is longer than allowed.");
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return refuse("The identifier contains a character that is neither alphanumeric nor a hyphen.");
    }
    if rule.lowercase_only && value.chars().any(|c| c.is_ascii_uppercase()) {
        return refuse("Alphabetic characters must be lowercase.");
    }
    if !value
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
    {
        return refuse("The first character must be a letter.");
    }
    if value.ends_with('-') {
        return refuse("The identifier cannot end with a hyphen.");
    }
    if value.contains("--") {
        return refuse("The identifier cannot contain two consecutive hyphens.");
    }
    Ok(())
}

/// The opaque `Marker` cursor the paginated `Describe*` operations hand out for
/// the record at `offset` in the stable insertion order.
///
/// The offset is paired with a tag derived from it, so a cursor this service did
/// not produce is refused rather than served as a mid-page offset: the
/// representation is this module's business alone and a caller can only echo
/// back one it was given.
fn marker_for(offset: usize) -> String {
    format!("{offset}.{}", short_digest(&format!("marker/{offset}"), 8))
}

/// The offset a `Marker` names, or `None` when the cursor is not one
/// [`marker_for`] produced.
fn marker_offset(cursor: &str) -> Option<usize> {
    let (offset, tag) = cursor.rsplit_once('.')?;
    let offset: usize = offset.parse().ok()?;
    (tag == short_digest(&format!("marker/{offset}"), 8)).then_some(offset)
}

/// The `MaxRecords` window one paginated operation accepts: the page size it
/// uses when the request carries none, and the inclusive bounds a request may
/// ask for.
///
/// A window rather than three bare constants because the operations do NOT all
/// share one: most Redshift `Describe*` operations publish the 20..100 window of
/// [`MAX_RECORDS_WINDOW`], while `DescribeTags` publishes its own
/// ([`TAG_MAX_RECORDS_WINDOW`]).
#[derive(Debug, Clone, Copy)]
pub struct MaxRecordsWindow {
    /// The page size a request that names no `MaxRecords` gets.
    pub default: i32,
    /// The smallest `MaxRecords` a request may name.
    pub min: i32,
    /// The largest `MaxRecords` a request may name.
    pub max: i32,
}

/// The window [`page_bounds`] applies: the one every paginated Redshift
/// `Describe*` operation but `DescribeTags` publishes. See
/// [`MAX_RECORDS_DEFAULT`].
pub const MAX_RECORDS_WINDOW: MaxRecordsWindow = MaxRecordsWindow {
    default: MAX_RECORDS_DEFAULT,
    min: MAX_RECORDS_MIN,
    max: MAX_RECORDS_MAX,
};

/// `DescribeTags`'s own `MaxRecords` window.
///
/// Source: `smithy_query(query="doc", operation="DescribeTags")` — "The
/// MaxRecords parameter can be a range from 10 to 50 results to return in a
/// request." The sentence states no default, so the page size a request that
/// names no `MaxRecords` gets is this window's own MAXIMUM: the ceiling the
/// service refuses a caller for asking past is also the most it will serve
/// unasked, so no page can be larger than a caller is allowed to request.
pub const TAG_MAX_RECORDS_WINDOW: MaxRecordsWindow = MaxRecordsWindow {
    default: 50,
    min: 10,
    max: 50,
};

/// Resolve one page of `total` records from the request's `Marker` / `MaxRecords`,
/// under the `MaxRecords` window of the operation serving it.
///
/// Returns the half-open range of the page and the `Marker` to hand back, which
/// is `Some` only when records remain after it.
pub fn page_bounds_within(
    window: MaxRecordsWindow,
    marker: Option<&str>,
    max_records: Option<i32>,
    total: usize,
) -> Result<(usize, usize, Option<String>), String> {
    let page_size = match max_records {
        None => window.default,
        Some(value) if (window.min..=window.max).contains(&value) => value,
        Some(value) => {
            return Err(format!(
                "Invalid value for MaxRecords: {value}. MaxRecords must be between {} and {}.",
                window.min, window.max
            ))
        }
    };
    let start = match marker {
        None => 0usize,
        Some(cursor) => match marker_offset(cursor) {
            Some(offset) if offset <= total => offset,
            _ => {
                return Err(format!(
                    "Invalid value for Marker: '{cursor}'. The marker must be one returned by a previous call."
                ))
            }
        },
    };
    let end = (start + page_size as usize).min(total);
    let next = (end < total).then(|| marker_for(end));
    Ok((start, end, next))
}

/// Resolve one page of `total` records under the window every paginated Redshift
/// `Describe*` operation but `DescribeTags` publishes ([`MAX_RECORDS_WINDOW`]).
pub fn page_bounds(
    marker: Option<&str>,
    max_records: Option<i32>,
    total: usize,
) -> Result<(usize, usize, Option<String>), String> {
    page_bounds_within(MAX_RECORDS_WINDOW, marker, max_records, total)
}

/// The region this request was signed for, from its SigV4 credential scope.
///
/// Falls back to `us-east-1`, the region a generated emulator presents when a
/// request carries no scope to read one from.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| "us-east-1".to_string())
}

/// `n` lowercase hex characters of the SHA-256 of `seed`.
///
/// Used only to derive the stable, content-addressed parts of an endpoint
/// hostname and a namespace id: the same cluster always renders the same value,
/// so nothing here is a per-run value.
fn short_digest(seed: &str, n: usize) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    hex::encode(digest).chars().take(n).collect()
}

/// The SHA-256 of `seed` rendered in the 8-4-4-4-12 lowercase-hex form AWS
/// identifies a reserved node offering and a namespace by.
///
/// Content-addressed from the resource's own identity, so the same resource
/// always renders the same value and nothing here is a per-run value.
fn content_uuid(seed: &str) -> String {
    let hexed = short_digest(seed, 32);
    format!(
        "{}-{}-{}-{}-{}",
        &hexed[0..8],
        &hexed[8..12],
        &hexed[12..16],
        &hexed[16..20],
        &hexed[20..32]
    )
}

/// The first byte of the SHA-256 of `seed`, for picking a stable default out of
/// a documented set.
fn digest_byte(seed: &str) -> u8 {
    Sha256::digest(seed.as_bytes())[0]
}

/// The DNS address of a cluster's connection endpoint, in Redshift's
/// `<cluster>.<id>.<region>.redshift.amazonaws.com` form.
///
/// Derived from the cluster identifier and region, so renaming a cluster moves
/// its endpoint exactly as the real hostname does.
pub fn endpoint_address(cluster_identifier: &str, region: &str) -> String {
    let token = short_digest(&format!("endpoint/{cluster_identifier}/{region}"), 12);
    format!("{cluster_identifier}.{token}.{region}.redshift.amazonaws.com")
}

/// The namespace ARN of a cluster: `arn:aws:redshift:<region>:<account>:namespace:<uuid>`.
///
/// Computed ONCE, when the cluster is created, and then persisted on the stored
/// cluster — the namespace identifies the cluster's data, so it must survive a
/// rename.
pub fn namespace_arn(cluster_identifier: &str, region: &str) -> String {
    let hexed = short_digest(&format!("namespace/{cluster_identifier}/{region}"), 32);
    let uuid = format!(
        "{}-{}-{}-{}-{}",
        &hexed[0..8],
        &hexed[8..12],
        &hexed[12..16],
        &hexed[16..20],
        &hexed[20..32]
    );
    engine::emulator_arn("redshift", region, &format!("namespace:{uuid}"))
}

/// The ARN of a snapshot: `arn:aws:redshift:<region>:<account>:snapshot:<cluster>/<snapshot>`.
pub fn snapshot_arn(cluster_identifier: &str, snapshot_identifier: &str, region: &str) -> String {
    engine::emulator_arn(
        "redshift",
        region,
        &format!("snapshot:{cluster_identifier}/{snapshot_identifier}"),
    )
}

/// The ARN of a cluster: `arn:aws:redshift:<region>:<account>:cluster:<cluster>`.
///
/// Source: the Amazon Redshift Cluster Management Guide, "Constructing an Amazon
/// Redshift Amazon Resource Name (ARN)" (the page
/// `smithy_query(query="doc", shape="TaggedResource", member="ResourceType")`
/// links to) — `arn:aws:redshift:region:account-id:cluster:cluster-name` — which
/// is also the example on `smithy_query(query="doc",
/// shape="CreateTagsMessage", member="ResourceName")`:
/// "arn:aws:redshift:us-east-2:123456789:cluster:t1".
pub fn cluster_arn(cluster_identifier: &str, region: &str) -> String {
    engine::emulator_arn(
        "redshift",
        region,
        &format!("{ARN_SEGMENT_CLUSTER}:{cluster_identifier}"),
    )
}

/// The ARN of a cluster parameter group:
/// `arn:aws:redshift:<region>:<account>:parametergroup:<name>`.
///
/// Source: as [`cluster_arn`] —
/// `arn:aws:redshift:region:account-id:parametergroup:parameter-group-name`.
pub fn parameter_group_arn(parameter_group_name: &str, region: &str) -> String {
    engine::emulator_arn(
        "redshift",
        region,
        &format!("{ARN_SEGMENT_PARAMETER_GROUP}:{parameter_group_name}"),
    )
}

/// The ARN of a cluster subnet group:
/// `arn:aws:redshift:<region>:<account>:subnetgroup:<name>`.
///
/// Source: as [`cluster_arn`] —
/// `arn:aws:redshift:region:account-id:subnetgroup:subnet-group-name`.
pub fn subnet_group_arn(subnet_group_name: &str, region: &str) -> String {
    engine::emulator_arn(
        "redshift",
        region,
        &format!("{ARN_SEGMENT_SUBNET_GROUP}:{subnet_group_name}"),
    )
}

/// The ARN of an HSM configuration:
/// `arn:aws:redshift:<region>:<account>:hsmconfiguration:<identifier>`.
///
/// Source: as [`cluster_arn`] —
/// `arn:aws:redshift:region:account-id:hsmconfiguration:hsm-configuration-name`.
pub fn hsm_configuration_arn(hsm_configuration_identifier: &str, region: &str) -> String {
    engine::emulator_arn(
        "redshift",
        region,
        &format!("{ARN_SEGMENT_HSM_CONFIGURATION}:{hsm_configuration_identifier}"),
    )
}

/// The ARN of the Secrets Manager secret holding a cluster's admin credentials,
/// used only when `ManageMasterPassword` is on.
pub fn master_password_secret_arn(cluster_identifier: &str, region: &str) -> String {
    let token = short_digest(&format!("secret/{cluster_identifier}/{region}"), 6);
    engine::emulator_arn(
        "secretsmanager",
        region,
        &format!("secret:redshift!{cluster_identifier}-{token}"),
    )
}

/// The Availability Zones of one Region, in name order.
///
/// A Region's zones are `<region>` followed by consecutive letters from `a`, as
/// many as that Region publishes ([`region_zone_count`]). Derived from the
/// Region alone, so the same Region always reports the same zones.
pub fn region_availability_zones(region: &str) -> Vec<String> {
    (0..region_zone_count(region))
        .map(|index| format!("{region}{}", ZONE_LETTERS[index]))
        .collect()
}

/// The letters a zone name may end in, in order; a Region uses the first
/// [`region_zone_count`] of them.
const ZONE_LETTERS: [char; 6] = ['a', 'b', 'c', 'd', 'e', 'f'];

/// How many Availability Zones AWS publishes for one Region.
///
/// Source: the AWS global-infrastructure reference "Regions, Availability Zones,
/// and Local Zones" (also `aws ec2 describe-availability-zones` per Region): the
/// count differs BY REGION, so a caller choosing a zone from a Region's list
/// gets that Region's zones and not a fixed three. [`REGION_ZONE_COUNTS`] names
/// every Region that publishes more than [`DEFAULT_ZONE_COUNT`]; any other
/// Region publishes that default, which is the smallest count AWS publishes for
/// a Region.
pub fn region_zone_count(region: &str) -> usize {
    REGION_ZONE_COUNTS
        .iter()
        .find(|(named, _)| *named == region)
        .map_or(DEFAULT_ZONE_COUNT, |(_, count)| *count)
}

/// The number of Availability Zones a Region publishes unless
/// [`REGION_ZONE_COUNTS`] names it.
pub const DEFAULT_ZONE_COUNT: usize = 3;

/// Every Region that publishes more Availability Zones than
/// [`DEFAULT_ZONE_COUNT`], with the number it publishes.
///
/// Source: as [`region_zone_count`] — the AWS Regions and Availability Zones
/// reference. Kept to the Regions whose count differs from the default, so a
/// Region added to AWS later still reports a well-formed zone list.
pub const REGION_ZONE_COUNTS: [(&str, usize); 4] = [
    ("us-east-1", 6),
    ("us-west-2", 4),
    ("ap-northeast-1", 4),
    ("ap-northeast-2", 4),
];

/// The only platform a cluster can be launched on today.
///
/// Source: `smithy_query(query="shape", shape="SupportedPlatform")` — "A list of
/// supported platforms for orderable clusters". EC2-Classic is retired, so every
/// zone a cluster is orderable in supports exactly the VPC platform.
pub const PLATFORM_VPC: &str = "VPC";

/// The platforms a cluster placed in an Availability Zone can run on.
pub fn zone_supported_platforms() -> Vec<SupportedPlatform> {
    [PLATFORM_VPC]
        .into_iter()
        .map(|name| SupportedPlatform {
            name: Some(name.to_string()),
        })
        .collect()
}

/// The platform a cluster ran on OUTSIDE a VPC, before EC2-Classic was retired.
///
/// Named here only so the capabilities that existed for that platform alone can
/// be decided from the published platform list rather than from a flag: EC2's
/// `supported-platforms` account attribute reports `EC2` for EC2-Classic and
/// `VPC` for the VPC platform, and [`zone_supported_platforms`] publishes only
/// the latter.
pub const PLATFORM_EC2_CLASSIC: &str = "EC2";

/// Whether this service offers the platform a cluster security group governs.
///
/// A cluster security group is EC2-Classic's access-control mechanism: it
/// "control[s] access to non-VPC clusters"
/// (`smithy_query(query="doc", shape="CreateClusterSecurityGroup")`), and a
/// cluster inside a VPC is governed by VPC security groups
/// (`Cluster$VpcSecurityGroups`) instead. So the question "can this account hold
/// a cluster security group at all?" is the question "does any zone it can place
/// a cluster in offer the EC2-Classic platform?", answered from the one
/// published platform list rather than from a second, independently maintained
/// fact.
pub fn cluster_security_groups_available() -> bool {
    zone_supported_platforms()
        .iter()
        .any(|platform| platform.name.as_deref() == Some(PLATFORM_EC2_CLASSIC))
}

/// Why a call that would mint a cluster security group is refused.
///
/// The text says the capability is gone and names the one that replaced it,
/// because that — not any value the request carried — is what the caller has to
/// act on: no name, description or tag list could make such a call succeed. Only
/// the status (400) and the error code (`InvalidParameterValue`) are wire
/// requirements; this wording is the service's own account of the rule, which is
/// why it lives here once rather than being spelled at a call site.
pub const CLUSTER_SECURITY_GROUPS_UNAVAILABLE: &str =
    "Cluster security groups are discontinued. Use VPC security groups to control access to an \
     Amazon Redshift cluster.";

/// One Availability Zone as this service reports it: its name AND the platforms
/// a cluster in it can run on.
///
/// `AvailabilityZone` is `{Name, SupportedPlatforms}`
/// (`smithy_query(query="shape", shape="AvailabilityZone")`), and a client
/// branching on platform support reads the second member — so every
/// `AvailabilityZone` this service emits is built here rather than from the name
/// alone.
pub fn availability_zone(name: &str) -> AvailabilityZone {
    AvailabilityZone {
        name: Some(name.to_string()),
        supported_platforms: Some(zone_supported_platforms()),
    }
}

/// The Availability Zones of one Region as modelled records, in name order:
/// [`region_availability_zones`] through [`availability_zone`].
pub fn region_availability_zone_records(region: &str) -> Vec<AvailabilityZone> {
    region_availability_zones(region)
        .iter()
        .map(|name| availability_zone(name))
        .collect()
}

/// The Availability Zone Redshift picks when the request names none.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage",
/// member="AvailabilityZone")` — "Default: A random, system-chosen Availability
/// Zone in the region that is specified by the endpoint." The emulator's choice
/// is derived from the cluster identifier rather than drawn per run, so it is
/// stable across reads.
pub fn default_availability_zone(cluster_identifier: &str, region: &str) -> String {
    let zones = region_availability_zones(region);
    let index = digest_byte(&format!("az/{cluster_identifier}")) as usize % zones.len();
    zones[index].clone()
}

/// The 30-minute weekly maintenance window Redshift assigns when the request
/// names none, in the `ddd:hh24:mi-ddd:hh24:mi` form Redshift reports.
///
/// Derived from the cluster identifier, so it is stable across reads.
pub fn default_maintenance_window(cluster_identifier: &str) -> String {
    const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
    let seed = digest_byte(&format!("window/{cluster_identifier}"));
    let day = DAYS[usize::from(seed) % DAYS.len()];
    // 0..=22, so the window's end hour is always a valid same-day hour.
    let hour = (usize::from(seed) / DAYS.len()) % 23;
    format!("{day}:{hour:02}:30-{day}:{:02}:00", hour + 1)
}

/// The VPC a cluster launched into a subnet group reports.
///
/// Every provisioned cluster lives in a VPC (see
/// [`DEFAULT_CLUSTER_SUBNET_GROUP`]), and the VPC it is in is the one its subnet
/// group's subnets belong to, so the id is derived from the subnet group name
/// alone: two clusters in the same subnet group report the same VPC.
pub fn derived_vpc_id(cluster_subnet_group_name: &str) -> String {
    format!(
        "vpc-{}",
        short_digest(&format!("vpc/{cluster_subnet_group_name}"), 17)
    )
}

/// The cluster subnet group a cluster joins when the request names none.
///
/// Source: `smithy_query(query="doc", operation="CreateCluster")` — "To create a
/// cluster in Virtual Private Cloud (VPC), you must provide a cluster subnet
/// group name. The cluster subnet group identifies the subnets of your VPC that
/// Amazon Redshift uses when creating the cluster." Amazon Redshift creates every
/// provisioned cluster in a VPC and an account that names no subnet group gets
/// its DEFAULT one, which Redshift names `default` — so a cluster's
/// `ClusterSubnetGroupName` and, with it, its `VpcId` are always reported.
pub const DEFAULT_CLUSTER_SUBNET_GROUP: &str = "default";

/// The `VpcSecurityGroupMembership$Status` of a security group a cluster is in.
const SECURITY_GROUP_STATUS_ACTIVE: &str = "active";

/// The VPC security group a cluster joins when the request names none: the
/// default security group of the VPC its subnet group is in.
///
/// Derived from the VPC id, so every cluster of one VPC reports the same default
/// group, exactly as the VPC's own default group is shared.
pub fn default_vpc_security_group_id(vpc_id: &str) -> String {
    format!("sg-{}", short_digest(&format!("sg/{vpc_id}"), 17))
}

/// The `KmsKeyId` of a cluster encrypted with the key Amazon Web Services owns
/// rather than a customer managed key.
///
/// `smithy_query(query="doc", shape="Cluster", member="KmsKeyId")` — "The Key
/// Management Service (KMS) key ID of the encryption key used to encrypt data in
/// the cluster": an encrypted cluster always has one, and a cluster whose request
/// named no `KmsKeyId` is encrypted under the service-owned key, which Redshift
/// reports by this id rather than by an ARN.
pub const AWS_OWNED_KMS_KEY: &str = "AWS_OWNED_KMS_KEY";

/// What a `PendingModifiedValues$MasterUserPassword` carries while an admin
/// password change is still being applied: the member is `@sensitive`, so the
/// pending change is reported as a MASK and the password itself never reaches the
/// wire.
pub const REDACTED_MASTER_USER_PASSWORD: &str = "****";

/// `ClusterNode$NodeRole` of the one node of a single-node cluster, which is its
/// own leader and its own compute node.
pub const NODE_ROLE_SHARED: &str = "SHARED";
/// `ClusterNode$NodeRole` of the node that runs the query planner of a
/// multi-node cluster.
pub const NODE_ROLE_LEADER: &str = "LEADER";

/// The nodes of a cluster, in the order Redshift reports them.
///
/// Source: `smithy_query(query="doc", shape="ClusterNode", member="NodeRole")` —
/// "Whether the node is a leader node or a compute node", and
/// `smithy_query(query="doc", shape="CreateClusterMessage", member="ClusterType")`
/// — "When cluster type is specified as single-node, the NumberOfNodes parameter
/// is not required". So a `single-node` cluster is ONE node that is both roles at
/// once ([`NODE_ROLE_SHARED`]), and a `multi-node` cluster is its leader node
/// ([`NODE_ROLE_LEADER`]) followed by its `NumberOfNodes` compute nodes, each
/// numbered from zero.
///
/// The addresses are derived from the cluster identifier and the node's position,
/// so a cluster always reports the same topology and no address is a per-run
/// value.
///
/// The loop is bounded by [`MAX_NODES_PER_CLUSTER`] whatever it is handed, so the
/// per-node work this does can never be sized by a request: a node count above
/// that bound is refused with the modelled
/// `NumberOfNodesPerClusterLimitExceededFault` before any cluster is written (see
/// [`exceeds_nodes_per_cluster_limit`]), and this clamp keeps the bound at the
/// allocation site for every other caller.
pub fn cluster_nodes(
    cluster_identifier: &str,
    cluster_type: &str,
    number_of_nodes: i32,
) -> Vec<ClusterNode> {
    let node = |role: String, index: i32| ClusterNode {
        node_role: Some(role),
        private_ip_address: Some(derived_ipv4(
            10,
            &format!("private/{cluster_identifier}/{index}"),
        )),
        public_ip_address: Some(derived_ipv4(
            54,
            &format!("public/{cluster_identifier}/{index}"),
        )),
    };
    if cluster_type == CLUSTER_TYPE_SINGLE_NODE || number_of_nodes <= 1 {
        return vec![node(NODE_ROLE_SHARED.to_string(), 0)];
    }
    let compute_nodes = number_of_nodes.clamp(2, MAX_NODES_PER_CLUSTER);
    let mut nodes = Vec::with_capacity(compute_nodes as usize + 1);
    nodes.push(node(NODE_ROLE_LEADER.to_string(), 0));
    for index in 0..compute_nodes {
        nodes.push(node(format!("COMPUTE-{index}"), index + 1));
    }
    nodes
}

/// The largest `NumberOfNodes` one cluster may have.
///
/// Amazon Redshift bounds the compute nodes of a provisioned cluster by node
/// type, and 128 is the largest count any node type offers (the published
/// "Nodes per cluster" quota of the largest RA3 and DC2 node types), so it is the
/// bound this service enforces for every node type rather than a per-type table
/// it would have to invent. A request above it is refused with the operation's
/// own modelled `NumberOfNodesPerClusterLimitExceededFault` — "The operation
/// would exceed the number of nodes allowed for a cluster" — so no request can
/// size the work a handler does.
pub const MAX_NODES_PER_CLUSTER: i32 = 128;

/// Whether a request's `NumberOfNodes` is above [`MAX_NODES_PER_CLUSTER`], which
/// every operation that writes a cluster checks BEFORE it derives anything from
/// the count.
pub fn exceeds_nodes_per_cluster_limit(number_of_nodes: i32) -> bool {
    number_of_nodes > MAX_NODES_PER_CLUSTER
}

/// The refusal a `NumberOfNodes` above [`MAX_NODES_PER_CLUSTER`] carries.
pub fn nodes_per_cluster_limit_message(requested: i32) -> String {
    format!(
        "The operation would exceed the number of nodes allowed for a cluster ({MAX_NODES_PER_CLUSTER}): {requested} were requested."
    )
}

/// A dotted-quad IPv4 address in the `first`.0.0.0/8 space, derived from `seed`.
///
/// The last octet is kept inside 1..=254, so the address is never a network or a
/// broadcast address of its /24.
fn derived_ipv4(first: u8, seed: &str) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    format!(
        "{first}.{}.{}.{}",
        digest[0],
        digest[1],
        1 + u32::from(digest[2]) % 254
    )
}

/// The SSH public key of a cluster, in the `ssh-rsa <key> Amazon-Redshift` form
/// Redshift reports.
///
/// Source: `smithy_query(query="doc", shape="Cluster", member="ClusterPublicKey")`
/// — "The public key for the cluster". The key material is generated per cluster
/// by the service; here it is DERIVED from the cluster identifier so the same
/// cluster always reports the same key and nothing is drawn per run. The body
/// opens with the fixed prefix every `ssh-rsa` key carries.
pub fn cluster_public_key(cluster_identifier: &str) -> String {
    let body = short_digest(&format!("public-key/{cluster_identifier}"), 48);
    format!("ssh-rsa AAAAB3NzaC1yc2EAAAADAQAB{body}= Amazon-Redshift")
}

/// The database revision a cluster runs, as the six-digit number Redshift
/// reports.
///
/// Source: `smithy_query(query="doc", shape="Cluster",
/// member="ClusterRevisionNumber")` — "The specific revision number of the
/// database in the cluster". Derived from the engine version and the cluster, so
/// two clusters of one version can report different revisions (the service
/// patches them independently) and a cluster's own revision is stable across
/// reads.
pub fn cluster_revision_number(cluster_identifier: &str, cluster_version: &str) -> String {
    let digest = Sha256::digest(format!("revision/{cluster_version}/{cluster_identifier}"));
    let value = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]);
    format!("{}", 100_000 + value % 900_000)
}

/// The VPC interface endpoint a VPC-resident cluster is reached through, with the
/// network interface it has in the cluster's Availability Zone.
///
/// Source: `smithy_query(query="doc", shape="Endpoint", member="VpcEndpoints")`
/// and `smithy_query(query="doc", shape="VpcEndpoint")` — "The connection
/// endpoint for connecting to an Amazon Redshift cluster through the proxy",
/// whose `NetworkInterfaces` are "one or more network interfaces of the endpoint.
/// Also known as an interface endpoint." Every id is derived from the cluster and
/// its VPC, so the topology is stable across reads.
pub fn cluster_vpc_endpoints(
    cluster_identifier: &str,
    vpc_id: &str,
    availability_zone: &str,
) -> Vec<VpcEndpoint> {
    vec![VpcEndpoint {
        vpc_endpoint_id: Some(format!(
            "vpce-{}",
            short_digest(&format!("vpce/{cluster_identifier}/{vpc_id}"), 17)
        )),
        vpc_id: Some(vpc_id.to_string()),
        network_interfaces: Some(vec![NetworkInterface {
            network_interface_id: Some(format!(
                "eni-{}",
                short_digest(&format!("eni/{cluster_identifier}/{availability_zone}"), 17)
            )),
            subnet_id: Some(format!(
                "subnet-{}",
                short_digest(&format!("subnet/{vpc_id}/{availability_zone}"), 17)
            )),
            private_ip_address: Some(derived_ipv4(
                10,
                &format!("eni-address/{cluster_identifier}/{availability_zone}"),
            )),
            availability_zone: Some(availability_zone.to_string()),
            // The emulator presents IPv4 subnets only, so an endpoint interface
            // has no IPv6 address to report.
            ipv6_address: None,
        }]),
    }]
}

/// The `AquaConfiguration` every cluster reports.
///
/// Source: `smithy_query(query="doc", shape="Cluster", member="AquaConfiguration")`
/// — "This field is retired. Amazon Redshift automatically determines whether to
/// use AQUA (Advanced Query Accelerator)", and the identical sentence on
/// `CreateClusterMessage$AquaConfigurationStatus`: "This parameter is retired. It
/// does not set the AQUA configuration status." So the configuration status a
/// request asks for is IGNORED and the cluster reports the automatic
/// configuration (`auto`, of the `AquaConfigurationStatus` enum) with AQUA not in
/// use (`disabled`, of the `AquaStatus` enum).
pub fn retired_aqua_configuration() -> AquaConfiguration {
    AquaConfiguration {
        aqua_status: Some("disabled".to_string()),
        aqua_configuration_status: Some("auto".to_string()),
    }
}

/// The next UTC instant at which a cluster's weekly maintenance window opens,
/// strictly after `after`.
///
/// Source: `smithy_query(query="doc", shape="Cluster",
/// member="NextMaintenanceWindowStartTime")` — "The date and time in UTC when
/// system maintenance can begin" — computed from the cluster's own
/// `PreferredMaintenanceWindow`, whose documented form is
/// `ddd:hh24:mi-ddd:hh24:mi` (`smithy_query(query="doc",
/// shape="CreateClusterMessage", member="PreferredMaintenanceWindow")`). `None`
/// when the window is not in that form, so an unparsable window reports no next
/// start rather than a made-up one.
///
/// Every value is bounded before it is used and every instant is computed with
/// CHECKED arithmetic, so no window a request can carry — and no instant the
/// clock can reach — overflows or panics: an input that would is one this
/// reports no next start for.
pub fn next_maintenance_window_start(
    window: &str,
    after: ::aws_smithy_types::DateTime,
) -> Option<::aws_smithy_types::DateTime> {
    const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
    const SECONDS_PER_DAY: i64 = 24 * 60 * 60;

    let start = window.split('-').next()?;
    let mut parts = start.split(':');
    let day = parts.next()?.to_ascii_lowercase();
    // Parsed UNSIGNED and bounded on both ends: `hh24` is 00..=23 and `mi` is
    // 00..=59, so a negative or out-of-range field is a window this reports
    // nothing for rather than a value the arithmetic below has to survive.
    let hour: u32 = parts.next()?.parse().ok()?;
    let minute: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || hour > 23 || minute > 59 {
        return None;
    }
    let second_of_day = i64::from(hour) * 3600 + i64::from(minute) * 60;
    let target_day = DAYS.iter().position(|candidate| *candidate == day)? as i64;

    let after_secs = after.secs();
    // Epoch day 0 (1970-01-01) was a Thursday, index 3 of a Monday-first week.
    let day_index = after_secs.div_euclid(SECONDS_PER_DAY);
    let weekday = (day_index + 3).rem_euclid(7);
    let ahead = (target_day - weekday).rem_euclid(7);
    let mut start_secs = day_index
        .checked_add(ahead)?
        .checked_mul(SECONDS_PER_DAY)?
        .checked_add(second_of_day)?;
    if start_secs <= after_secs {
        start_secs = start_secs.checked_add(7 * SECONDS_PER_DAY)?;
    }
    Some(::aws_smithy_types::DateTime::from_secs(start_secs))
}

/// Fill in every `Cluster` member this service DERIVES from the cluster's own
/// configuration, leaving everything a request set exactly as it stands.
///
/// One function, called by every operation that writes a cluster record
/// (`CreateCluster`, `ModifyCluster`, `RestoreFromClusterSnapshot`), so the
/// create path, the modify path and the read path cannot report different member
/// sets for one cluster. It is idempotent: re-running it on a stored record
/// recomputes the derived members from what that record now says, which is what
/// makes it safe to call after a modify changed the node count, the engine
/// version or the maintenance window.
///
/// A read NEVER calls this: `DescribeClusters` reports the stored record.
pub fn populate_derived_members(
    cluster: &mut Cluster,
    cluster_type: &str,
    now: ::aws_smithy_types::DateTime,
) {
    let identifier = cluster.cluster_identifier.clone().unwrap_or_default();

    // Where the cluster is: every provisioned cluster is in a VPC, through the
    // subnet group it joined or the account's default one. Both members are set
    // from the stored subnet group record by the operation that launched the
    // cluster; what is filled in here is what that record did not carry.
    let subnet_group = cluster
        .cluster_subnet_group_name
        .get_or_insert_with(|| DEFAULT_CLUSTER_SUBNET_GROUP.to_string())
        .clone();
    let vpc_id = cluster
        .vpc_id
        .get_or_insert_with(|| derived_vpc_id(&subnet_group))
        .clone();

    // "This parameter is returned only if the cluster is in a VPC": a cluster
    // that named no security group is in its VPC's default one, so the list is
    // never empty.
    if cluster
        .vpc_security_groups
        .as_ref()
        .is_none_or(|groups| groups.is_empty())
    {
        cluster.vpc_security_groups = Some(vec![VpcSecurityGroupMembership {
            vpc_security_group_id: Some(default_vpc_security_group_id(&vpc_id)),
            status: Some(SECURITY_GROUP_STATUS_ACTIVE.to_string()),
        }]);
    }

    // An encrypted cluster always reports the key its data is encrypted under.
    if cluster.encrypted == Some(true) && cluster.kms_key_id.is_none() {
        cluster.kms_key_id = Some(AWS_OWNED_KMS_KEY.to_string());
    }

    let availability_zone = cluster.availability_zone.clone().unwrap_or_default();
    if let Some(endpoint) = cluster.endpoint.as_mut() {
        endpoint.vpc_endpoints = Some(cluster_vpc_endpoints(
            &identifier,
            &vpc_id,
            &availability_zone,
        ));
    }

    cluster.cluster_public_key = Some(cluster_public_key(&identifier));
    cluster.cluster_nodes = Some(cluster_nodes(
        &identifier,
        cluster_type,
        // Clamped at the call site as well as inside `cluster_nodes`: the
        // topology is one record per node, so the size of this work is bounded
        // here whatever the record carries.
        cluster
            .number_of_nodes
            .unwrap_or(1)
            .clamp(1, MAX_NODES_PER_CLUSTER),
    ));
    cluster.cluster_revision_number = Some(cluster_revision_number(
        &identifier,
        cluster.cluster_version.as_deref().unwrap_or(""),
    ));
    cluster.aqua_configuration = Some(retired_aqua_configuration());
    cluster.cluster_availability_status = Some(
        availability_status(cluster.cluster_status.as_deref().unwrap_or_default()).to_string(),
    );

    // "Describes a group of DeferredMaintenanceWindow objects": a cluster whose
    // maintenance nobody deferred reports an EMPTY group rather than none.
    cluster.deferred_maintenance_windows.get_or_insert_default();
    // "A change to one or more the attributes was requested and is in progress
    // or will be applied": a cluster with nothing pending reports the structure
    // with no member set.
    cluster.pending_modified_values.get_or_insert_default();
    if let Some(window) = cluster.preferred_maintenance_window.as_deref() {
        cluster.next_maintenance_window_start_time = next_maintenance_window_start(window, now);
    }
}

/// The IAM role associations a cluster reports for `arns`, each at `apply_status`.
///
/// One rule for every path that puts roles on a cluster (`CreateCluster`,
/// `RestoreFromClusterSnapshot`, `ModifyClusterIamRoles`), so the create path and
/// the modify path cannot report an association differently. The ARNs are stored
/// exactly as the request carried them: an IAM role is a resource of another
/// service, so this service does not resolve one.
pub fn cluster_iam_roles<I>(arns: I, apply_status: &str) -> Vec<ClusterIamRole>
where
    I: IntoIterator<Item = String>,
{
    arns.into_iter()
        .map(|arn| ClusterIamRole {
            iam_role_arn: Some(arn),
            apply_status: Some(apply_status.to_string()),
        })
        .collect()
}

/// Associate `add` and disassociate `remove` on a cluster's IAM role list.
///
/// The list is a SET keyed by role ARN, in association order: associating an ARN
/// the cluster already carries leaves one entry, and disassociating an ARN it
/// does not carry changes nothing. Disassociations are applied first, so an ARN
/// named by both lists ends up associated.
///
/// Neither change is complete when this returns: a new association starts at
/// [`IAM_ROLE_APPLY_STATUS_ADDING`] and a disassociation at
/// [`IAM_ROLE_APPLY_STATUS_REMOVING`], and
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::settle`] completes
/// both once the cluster's span has passed.
pub fn apply_iam_role_changes(roles: &mut Vec<ClusterIamRole>, add: &[String], remove: &[String]) {
    let position = |roles: &[ClusterIamRole], arn: &str| {
        roles
            .iter()
            .position(|role| role.iam_role_arn.as_deref() == Some(arn))
    };
    for arn in remove {
        if let Some(index) = position(roles, arn) {
            roles[index].apply_status = Some(IAM_ROLE_APPLY_STATUS_REMOVING.to_string());
        }
    }
    for arn in add {
        match position(roles, arn) {
            // Already associated: re-associating it is not a second entry, and
            // an association already in use stays in use. One this same call
            // asked to disassociate goes back to being established.
            Some(index) => {
                if roles[index].apply_status.as_deref() != Some(IAM_ROLE_APPLY_STATUS_IN_SYNC) {
                    roles[index].apply_status = Some(IAM_ROLE_APPLY_STATUS_ADDING.to_string());
                }
            }
            None => roles.extend(cluster_iam_roles(
                [arn.clone()],
                IAM_ROLE_APPLY_STATUS_ADDING,
            )),
        }
    }
}

/// The parameter-group membership a cluster reports for the group it joined.
///
/// `smithy_query(query="doc", shape="ClusterParameterGroupStatus")` — "Describes
/// the status of a parameter group": the group's name and how far its values have
/// been applied. A cluster joins ONE group, the one the request named or the
/// service default ([`cluster_default_parameter_group`]).
pub fn cluster_parameter_group_status(requested: Option<String>) -> Vec<ClusterParameterGroupStatus> {
    vec![ClusterParameterGroupStatus {
        parameter_group_name: Some(requested.unwrap_or_else(cluster_default_parameter_group)),
        parameter_apply_status: Some(PARAMETER_APPLY_STATUS_IN_SYNC.to_string()),
        cluster_parameter_status_list: None,
    }]
}

/// The pending admin-password change a cluster reports while it is being applied.
///
/// Source: `smithy_query(query="doc", shape="PendingModifiedValues",
/// member="MasterUserPassword")` — "The pending or in-progress change of the
/// admin user password for the cluster". The member is `@sensitive`, so what is
/// reported is the mask [`REDACTED_MASTER_USER_PASSWORD`], never the password.
pub fn pending_master_user_password(password: Option<&str>) -> PendingModifiedValues {
    PendingModifiedValues {
        master_user_password: password.map(|_| REDACTED_MASTER_USER_PASSWORD.to_string()),
        ..Default::default()
    }
}

/// The account's DEFAULT cluster subnet group, as a Region's default VPC presents
/// it: one subnet per Availability Zone of the Region, in the VPC
/// [`derived_vpc_id`] gives the group's name.
///
/// Amazon Redshift creates every provisioned cluster in a VPC, and an account
/// that names no subnet group gets the default one (see
/// [`DEFAULT_CLUSTER_SUBNET_GROUP`]), so this record is what a cluster that named
/// no group is launched into — and what a `DescribeClusterSubnetGroups` naming
/// `default` reports. Built by the same helpers
/// `CreateClusterSubnetGroup` builds a caller's group with, so the two records
/// cannot disagree about a group's shape.
pub fn default_subnet_group(region: &str) -> ClusterSubnetGroup {
    ClusterSubnetGroup {
        cluster_subnet_group_name: Some(DEFAULT_CLUSTER_SUBNET_GROUP.to_string()),
        description: Some("Default cluster subnet group".to_string()),
        vpc_id: Some(derived_vpc_id(DEFAULT_CLUSTER_SUBNET_GROUP)),
        subnet_group_status: Some(SUBNET_GROUP_STATUS_COMPLETE.to_string()),
        subnets: Some(
            region_availability_zones(region)
                .iter()
                .map(|zone| Subnet {
                    subnet_identifier: Some(format!(
                        "subnet-{}",
                        short_digest(
                            &format!("subnet/{DEFAULT_CLUSTER_SUBNET_GROUP}/{zone}"),
                            17
                        )
                    )),
                    subnet_availability_zone: Some(availability_zone(zone)),
                    subnet_status: Some(SUBNET_STATUS_ACTIVE.to_string()),
                })
                .collect(),
        ),
        tags: Some(Vec::new()),
        supported_cluster_ip_address_types: Some(
            SUPPORTED_CLUSTER_IP_ADDRESS_TYPES
                .iter()
                .map(|kind| (*kind).to_string())
                .collect(),
        ),
    }
}

/// The subnet record a cluster subnet group reports for one of its subnet ids.
///
/// The Availability Zone is derived from the subnet id and the Region the group
/// was created in, so the same subnet always reports the same zone and a group
/// spanning several subnets spans the zones they resolve to. This service models
/// no EC2 subnet inventory, so the id a request names is the id the group holds.
pub fn subnet_record(subnet_identifier: &str, region: &str) -> Subnet {
    let zones = region_availability_zones(region);
    let index = digest_byte(&format!("subnet/{subnet_identifier}")) as usize % zones.len();
    Subnet {
        subnet_identifier: Some(subnet_identifier.to_string()),
        // Built by the one helper every `AvailabilityZone` this service emits is
        // built by, so a subnet's zone reports the same platforms as the same
        // zone reported anywhere else.
        subnet_availability_zone: Some(availability_zone(&zones[index])),
        subnet_status: Some(SUBNET_STATUS_ACTIVE.to_string()),
    }
}

/// Build the snapshot record a manual snapshot of `cluster` reports.
///
/// Every identifying attribute is COPIED off the stored cluster — node type,
/// node count, admin user, database, engine version, port, Availability Zone and
/// the cluster's own creation time — so a snapshot read back describes the
/// cluster as it was when the snapshot was asked for.
///
/// `Status` starts at `creating`: `smithy_query(query="doc", shape="Snapshot",
/// member="Status")` states "CreateClusterSnapshot and CopyClusterSnapshot
/// returns status as \"creating\"". [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::settle`]
/// moves it to `available` once [`PROVISIONING_SPAN`] has passed.
///
/// The manual retention period is applied through [`apply_manual_retention`],
/// the one rule for it, so a snapshot a create minted reports the same retention
/// bookkeeping a `ModifyClusterSnapshot` would leave for the same period.
pub fn snapshot_of(
    cluster: &Cluster,
    snapshot_identifier: &str,
    created_at: ::aws_smithy_types::DateTime,
    manual_retention_period: i32,
    tags: Vec<crate::ops::types::Tag>,
    region: &str,
) -> Snapshot {
    let cluster_identifier = cluster.cluster_identifier.clone().unwrap_or_default();
    let mut snapshot = Snapshot {
        snapshot_identifier: Some(snapshot_identifier.to_string()),
        cluster_identifier: Some(cluster_identifier.clone()),
        snapshot_create_time: Some(created_at),
        status: Some(STATUS_CREATING.to_string()),
        port: cluster.endpoint.as_ref().and_then(|e| e.port),
        availability_zone: cluster.availability_zone.clone(),
        cluster_create_time: cluster.cluster_create_time,
        master_username: cluster.master_username.clone(),
        cluster_version: cluster.cluster_version.clone(),
        snapshot_type: Some(SNAPSHOT_TYPE_MANUAL.to_string()),
        node_type: cluster.node_type.clone(),
        number_of_nodes: cluster.number_of_nodes,
        db_name: cluster.db_name.clone(),
        vpc_id: cluster.vpc_id.clone(),
        encrypted: cluster.encrypted,
        kms_key_id: cluster.kms_key_id.clone(),
        encrypted_with_hsm: Some(cluster.hsm_status.is_some()),
        owner_account: Some(engine::emulator_account_id().to_string()),
        // `Snapshot$SourceRegion` names the region a snapshot was COPIED from, so
        // a snapshot taken in place leaves it unset.
        tags: Some(tags),
        enhanced_vpc_routing: cluster.enhanced_vpc_routing,
        maintenance_track_name: cluster.maintenance_track_name.clone(),
        master_password_secret_arn: cluster.master_password_secret_arn.clone(),
        master_password_secret_kms_key_id: cluster.master_password_secret_kms_key_id.clone(),
        snapshot_arn: Some(snapshot_arn(
            &cluster_identifier,
            snapshot_identifier,
            region,
        )),
        ..Default::default()
    };
    apply_manual_retention(&mut snapshot, manual_retention_period, created_at);
    snapshot
}

/// Resolve the `ClusterType` / `NumberOfNodes` pair a request asks for, or say
/// why the pair cannot hold.
///
/// The two members describe one another, so neither is applied alone. Source:
/// `smithy_query(query="doc", shape="CreateClusterMessage", member="ClusterType")`
/// — "Valid Values: multi-node | single-node" and "When cluster type is specified
/// as single-node, the NumberOfNodes parameter is not required. multi-node, the
/// NumberOfNodes parameter is required" — with `NumberOfNodes`'s own "Default: 1".
///
/// `current` is the stored pair when a cluster is being MODIFIED, and `None` when
/// it is being created; a create that names neither member takes the node-count
/// default and the cluster type that node count describes. Shared by
/// `CreateCluster` and `ModifyCluster` so the rule cannot drift between them.
///
/// The resolved count is BOUNDED on both ends: at least one node (two for a
/// multi-node cluster) and at most [`MAX_NODES_PER_CLUSTER`].
pub fn resolve_cluster_shape(
    requested_type: Option<&str>,
    requested_nodes: Option<i32>,
    current: Option<(&str, i32)>,
) -> Result<(String, i32), String> {
    let kind = match requested_type {
        Some(CLUSTER_TYPE_SINGLE_NODE) => CLUSTER_TYPE_SINGLE_NODE,
        Some(CLUSTER_TYPE_MULTI_NODE) => CLUSTER_TYPE_MULTI_NODE,
        Some(other) => {
            return Err(format!(
                "Invalid value for ClusterType: '{other}'. Valid Values: {}",
                ORDERABLE_CLUSTER_TYPES.join(" | ")
            ))
        }
        // No `ClusterType`: a modify keeps the stored shape, and a create lets
        // the node count (default 1) say which shape is being asked for.
        None => match current {
            Some((stored, _)) => stored,
            None if requested_nodes.unwrap_or(1) > 1 => CLUSTER_TYPE_MULTI_NODE,
            None => CLUSTER_TYPE_SINGLE_NODE,
        },
    };
    // A node count the request does not name: "When cluster type is specified as
    // single-node, the NumberOfNodes parameter is not required" — a single-node
    // cluster has exactly one node, whatever it had before. A cluster that STAYS
    // multi-node keeps the count it had; one that BECOMES multi-node must say how
    // many, since "multi-node, the NumberOfNodes parameter is required".
    let nodes = match (requested_nodes, kind, current) {
        (Some(nodes), _, _) => nodes,
        (None, CLUSTER_TYPE_SINGLE_NODE, _) => 1,
        (None, _, Some((stored_kind, stored_nodes))) if stored_kind == kind => stored_nodes,
        (None, _, _) => {
            return Err(
                "NumberOfNodes is required when ClusterType is multi-node.".to_string()
            )
        }
    };
    if kind == CLUSTER_TYPE_SINGLE_NODE && nodes != 1 {
        return Err(format!(
            "Invalid value for NumberOfNodes: {nodes}. A single-node cluster has exactly 1 node."
        ));
    }
    if kind == CLUSTER_TYPE_MULTI_NODE && nodes < 2 {
        return Err(format!(
            "Invalid value for NumberOfNodes: {nodes}. A multi-node cluster must have at least 2 nodes."
        ));
    }
    // The UPPER bound belongs here too, beside the lower ones: this is the one
    // resolver every cluster write goes through, so no caller can carry a node
    // count past [`MAX_NODES_PER_CLUSTER`] into the per-node work a cluster's
    // topology costs. An operation that models
    // `NumberOfNodesPerClusterLimitExceededFault` refuses with that fault BEFORE
    // it gets here (see [`exceeds_nodes_per_cluster_limit`]); this is the floor
    // under every other caller.
    if exceeds_nodes_per_cluster_limit(nodes) {
        return Err(nodes_per_cluster_limit_message(nodes));
    }
    Ok((kind.to_string(), nodes))
}

/// The ARN resource-type segment of a provisioned cluster.
pub const ARN_SEGMENT_CLUSTER: &str = "cluster";
/// The ARN resource-type segment of a cluster snapshot. Its resource id is
/// `<cluster>/<snapshot>` (see [`snapshot_arn`]).
pub const ARN_SEGMENT_SNAPSHOT: &str = "snapshot";
/// The ARN resource-type segment of a cluster parameter group.
pub const ARN_SEGMENT_PARAMETER_GROUP: &str = "parametergroup";
/// The ARN resource-type segment of a cluster subnet group.
pub const ARN_SEGMENT_SUBNET_GROUP: &str = "subnetgroup";
/// The ARN resource-type segment of an HSM configuration (the "HSM connection"
/// the API publishes).
pub const ARN_SEGMENT_HSM_CONFIGURATION: &str = "hsmconfiguration";

/// One resource type a Redshift tag can be attached to: the segment its ARN
/// carries and the name the API publishes for it.
#[derive(Debug, Clone, Copy)]
pub struct TaggableResourceType {
    /// The resource-type segment of this type's ARN, which is also the value
    /// `TaggedResource$ResourceType` reports and the value a
    /// `DescribeTags$ResourceType` filter names.
    pub arn_segment: &'static str,
    /// The human-readable name the model lists for the type, accepted on a
    /// `DescribeTags$ResourceType` filter beside [`Self::arn_segment`].
    pub documented_name: &'static str,
}

/// Every resource type a Redshift tag can be attached to, in the order the model
/// lists them.
///
/// Source for the names and the order: `smithy_query(query="doc",
/// shape="TaggedResource", member="ResourceType")` — "Valid resource types are:
/// Cluster, CIDR/IP, EC2 security group, Snapshot, Cluster security group,
/// Subnet group, HSM connection, HSM certificate, Parameter group" — plus the
/// two `smithy_query(query="doc", shape="DescribeTagsMessage",
/// member="ResourceType")` adds at the end of the same list ("Snapshot copy
/// grant", "Integration"). Source for each ARN segment: the Amazon Redshift
/// Cluster Management Guide page that same doc links to, "Constructing an Amazon
/// Redshift Amazon Resource Name (ARN)", which gives one ARN form per resource
/// type.
///
/// Only four of these types name a resource THIS store holds (see
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::set_resource_tags`]);
/// the rest are listed because they are valid `ResourceType` filters, and an
/// account that cannot create such a resource yet simply has none of them to
/// report.
pub const TAGGABLE_RESOURCE_TYPES: [TaggableResourceType; 11] = [
    TaggableResourceType {
        arn_segment: ARN_SEGMENT_CLUSTER,
        documented_name: "Cluster",
    },
    TaggableResourceType {
        arn_segment: "securitygroupingress-cidr",
        documented_name: "CIDR/IP",
    },
    TaggableResourceType {
        arn_segment: "securitygroupingress-ec2securitygroup",
        documented_name: "EC2 security group",
    },
    TaggableResourceType {
        arn_segment: ARN_SEGMENT_SNAPSHOT,
        documented_name: "Snapshot",
    },
    TaggableResourceType {
        arn_segment: "securitygroup",
        documented_name: "Cluster security group",
    },
    TaggableResourceType {
        arn_segment: ARN_SEGMENT_SUBNET_GROUP,
        documented_name: "Subnet group",
    },
    TaggableResourceType {
        arn_segment: ARN_SEGMENT_HSM_CONFIGURATION,
        documented_name: "HSM connection",
    },
    TaggableResourceType {
        arn_segment: "hsmclientcertificate",
        documented_name: "HSM certificate",
    },
    TaggableResourceType {
        arn_segment: ARN_SEGMENT_PARAMETER_GROUP,
        documented_name: "Parameter group",
    },
    TaggableResourceType {
        arn_segment: "snapshotcopygrant",
        documented_name: "Snapshot copy grant",
    },
    TaggableResourceType {
        arn_segment: "integration",
        documented_name: "Integration",
    },
];

/// A `ResourceType` or ARN segment folded for comparison: lower-cased with every
/// separator dropped, so one spelling of a type matches every other the API
/// publishes for it (`Subnet group`, `subnet-group`, `subnetgroup`).
fn folded_resource_type(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The taggable resource type a request's `ResourceType` names, by either the
/// ARN segment or the name the model publishes, in any case or separator
/// spelling; `None` when it names no published type.
pub fn resource_type_named(requested: &str) -> Option<&'static TaggableResourceType> {
    let wanted = folded_resource_type(requested);
    TAGGABLE_RESOURCE_TYPES.iter().find(|candidate| {
        folded_resource_type(candidate.arn_segment) == wanted
            || folded_resource_type(candidate.documented_name) == wanted
    })
}

/// Every published `ResourceType` value, for the message that refuses one this
/// service does not publish.
pub fn published_resource_types() -> Vec<&'static str> {
    TAGGABLE_RESOURCE_TYPES
        .iter()
        .map(|candidate| candidate.arn_segment)
        .collect()
}

/// How many tags one resource may carry.
///
/// Source: `smithy_query(query="doc", operation="CreateTags")` — "A resource can
/// have up to 50 tags. If you try to create more than 50 tags for a resource,
/// you will receive an error and the attempt will fail."
pub const TAGS_PER_RESOURCE_QUOTA: usize = 50;

/// The longest a tag key may be, in characters.
///
/// Source: the Amazon Redshift Cluster Management Guide, "Tagging resources in
/// Amazon Redshift" — its tag restrictions give a key a maximum of 128 Unicode
/// characters and a value a maximum of 256 ([`TAG_VALUE_MAX_LEN`]), and reserve
/// the [`RESERVED_TAG_PREFIX`] prefix. The model itself bounds neither
/// (`smithy_query(query="shape", shape="Tag")` targets plain `String` for both),
/// so the guide is the only published bound.
pub const TAG_KEY_MAX_LEN: usize = 128;

/// The longest a tag value may be, in characters. See [`TAG_KEY_MAX_LEN`].
pub const TAG_VALUE_MAX_LEN: usize = 256;

/// The key prefix AWS reserves for its own tags. See [`TAG_KEY_MAX_LEN`].
pub const RESERVED_TAG_PREFIX: &str = "aws:";

/// Check one request's tags against the published restrictions, naming the tag
/// and the restriction it breaks. An `Err` is an `InvalidTagFault`.
///
/// Length is counted in CHARACTERS, not bytes: the guide bounds a key at 128 and
/// a value at 256 Unicode characters, so a multi-byte value is bounded by what
/// it spells and not by how UTF-8 encodes it.
pub fn check_tags(tags: &[crate::ops::types::Tag]) -> Result<(), String> {
    for tag in tags {
        let key = tag.key.as_deref().unwrap_or_default();
        if key.is_empty() {
            return Err("Invalid tag: a tag key cannot be null, empty, or blank.".to_string());
        }
        if key.chars().count() > TAG_KEY_MAX_LEN {
            return Err(format!(
                "Invalid tag key '{key}': a tag key cannot be longer than {TAG_KEY_MAX_LEN} characters."
            ));
        }
        if key.to_lowercase().starts_with(RESERVED_TAG_PREFIX) {
            return Err(format!(
                "Invalid tag key '{key}': the prefix '{RESERVED_TAG_PREFIX}' is reserved for Amazon Web Services."
            ));
        }
        let value = tag.value.as_deref().unwrap_or_default();
        if value.chars().count() > TAG_VALUE_MAX_LEN {
            return Err(format!(
                "Invalid tag value for key '{key}': a tag value cannot be longer than {TAG_VALUE_MAX_LEN} characters."
            ));
        }
        if value.to_lowercase().starts_with(RESERVED_TAG_PREFIX) {
            return Err(format!(
                "Invalid tag value for key '{key}': the prefix '{RESERVED_TAG_PREFIX}' is reserved for Amazon Web Services."
            ));
        }
    }
    Ok(())
}

/// The tag list a resource carrying `existing` has after `incoming` is added,
/// or the message of the `TagLimitExceededFault` the request exceeds.
///
/// A key already on the resource keeps its position and takes the new value —
/// `smithy_query(query="doc", operation="CreateTags")`: "If you specify a key
/// that already exists for the resource, the value for that key will be updated
/// with the new value" — which is also what a key repeated WITHIN one request
/// does, the later pair winning. A key the resource does not carry is appended.
/// Computed into a new list rather than applied in place because the attempt
/// must "fail" as a whole when it would exceed [`TAGS_PER_RESOURCE_QUOTA`]:
/// nothing is persisted on the error path.
///
/// A tag whose `Value` the request omits is stored as the EMPTY value, which is
/// the value such a tag reports: the member is modelled optional but a stored
/// tag always has one.
pub fn tags_with(
    existing: &[crate::ops::types::Tag],
    incoming: &[crate::ops::types::Tag],
) -> Result<Vec<crate::ops::types::Tag>, String> {
    let mut merged = existing.to_vec();
    // The keys already in `merged`, by position: ONE lookup per incoming tag
    // rather than a scan of the merged list, so the merge cost grows with the
    // request rather than with its square.
    let mut positions: BTreeMap<String, usize> = merged
        .iter()
        .enumerate()
        .filter_map(|(position, tag)| tag.key.clone().map(|key| (key, position)))
        .collect();
    for tag in incoming {
        let key = tag.key.clone().unwrap_or_default();
        let value = Some(tag.value.clone().unwrap_or_default());
        match positions.get(&key) {
            Some(&position) => merged[position].value = value,
            None => {
                // Checked as the list GROWS, before the entry past the quota is
                // allocated, so a request that must fail cannot first do the
                // work of one that would have succeeded.
                if merged.len() == TAGS_PER_RESOURCE_QUOTA {
                    return Err(tag_quota_message(merged.len() + 1));
                }
                positions.insert(key.clone(), merged.len());
                merged.push(crate::ops::types::Tag {
                    key: Some(key),
                    value,
                });
            }
        }
    }
    Ok(merged)
}

/// Why a request past [`TAGS_PER_RESOURCE_QUOTA`] is refused, naming the count
/// the resource would have carried.
pub fn tag_quota_message(would_carry: usize) -> String {
    format!(
        "The request would exceed the allowed number of tags for one resource ({TAGS_PER_RESOURCE_QUOTA}); the resource would carry {would_carry}."
    )
}

/// A request's `TagKeys` / `TagValues` filter, indexed once for matching.
///
/// Built from the request's own lists BEFORE the store is walked, so testing a
/// resource against the filter costs one set lookup per tag instead of a scan of
/// the caller's lists: the work is the SUM of the request and the store rather
/// than their product, whatever lengths a caller sends.
#[derive(Debug, Default)]
pub struct TagFilter<'a> {
    /// The keys a resource must carry one of, or `None` when the filter does not
    /// constrain keys.
    keys: Option<BTreeSet<&'a str>>,
    /// The values a resource must carry one of, likewise.
    values: Option<BTreeSet<&'a str>>,
}

impl<'a> TagFilter<'a> {
    /// Index one request's `TagKeys` / `TagValues`.
    ///
    /// An absent list — and an EMPTY one, which names nothing to match —
    /// constrains nothing.
    pub fn new(tag_keys: Option<&'a [String]>, tag_values: Option<&'a [String]>) -> Self {
        let index = |list: Option<&'a [String]>| {
            list.filter(|values| !values.is_empty())
                .map(|values| values.iter().map(String::as_str).collect())
        };
        Self {
            keys: index(tag_keys),
            values: index(tag_values),
        }
    }

    /// Whether `tags` satisfy this filter.
    ///
    /// Each filter is independent and inclusive: a resource passes the keys when
    /// it carries a tag whose key is one of them, and the values when it carries
    /// a tag whose value is one of them.
    pub fn matches(&self, tags: &[crate::ops::types::Tag]) -> bool {
        if let Some(keys) = self.keys.as_ref() {
            if !tags
                .iter()
                .any(|tag| tag.key.as_deref().is_some_and(|key| keys.contains(key)))
            {
                return false;
            }
        }
        if let Some(values) = self.values.as_ref() {
            if !tags.iter().any(|tag| {
                tag.value
                    .as_deref()
                    .is_some_and(|value| values.contains(value))
            }) {
                return false;
            }
        }
        true
    }
}

/// Whether a resource's tags satisfy a request's `TagKeys` / `TagValues` filter.
///
/// For the one-resource-at-a-time callers; a caller that walks the store builds
/// the [`TagFilter`] once and matches through it instead.
pub fn tags_match(
    tags: &[crate::ops::types::Tag],
    tag_keys: Option<&[String]>,
    tag_values: Option<&[String]>,
) -> bool {
    TagFilter::new(tag_keys, tag_values).matches(tags)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRedshiftRedshiftserviceversion20121201State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRedshiftRedshiftserviceversion20121201State>)
}
