// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-redshift-redshiftserviceversion20121201 service.
//!
//! One store per emulator, holding the provisioned-cluster control plane: the
//! clusters, the snapshots taken of them, and the cluster parameter groups they
//! are configured from. Every collection keeps INSERTION ORDER, so the `Marker`
//! cursor the paginated `Describe*` operations hand out is a stable, resumable
//! offset into the same sequence a later page walks.
//!
//! Besides the store this module owns the small rules every cluster operation
//! shares: the published engine-version catalog and the default parameter group
//! of each of its families, the engine default PARAMETERS a parameter group of
//! such a family publishes (see [`ENGINE_DEFAULT_PARAMETERS`]), the documented
//! identifier syntax, the documented `MaxRecords` window,
//! the derived endpoint address / namespace ARN, and the clock-owed
//! `creating` -> `available` transitions (see [`PROVISIONING_SPAN`]).

use crate::ops::types::{Cluster, ClusterParameterGroup, ClusterVersion, Parameter, Snapshot};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

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

/// The cluster parameter group a cluster joins when the request names none:
/// `default.` followed by the version's [`parameter_group_family`], so engine
/// version 1.0 joins `default.redshift-1.0`.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage",
/// member="ClusterParameterGroupName")` — "Default: The default Amazon Redshift
/// cluster parameter group".
pub fn default_parameter_group(cluster_version: &str) -> String {
    format!("default.{}", parameter_group_family(cluster_version))
}

/// The Amazon Redshift engine versions this service offers, in the order
/// `DescribeClusterVersions` reports them.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage",
/// member="ClusterVersion")` — "Only version 1.0 is currently available. Example:
/// 1.0" — so the published catalog holds exactly that one engine version, with the
/// family [`parameter_group_family`] gives it. Seeded into
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::cluster_versions`]
/// when the store is created, so the catalog a read reports and the
/// [`DEFAULT_CLUSTER_VERSION`] a `CreateCluster` applies cannot disagree.
pub fn published_cluster_versions() -> Vec<ClusterVersion> {
    [DEFAULT_CLUSTER_VERSION]
        .into_iter()
        .map(|version| ClusterVersion {
            cluster_version: Some(version.to_string()),
            cluster_parameter_group_family: Some(parameter_group_family(version)),
            description: Some(format!("Cluster version {version}")),
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
/// Derived from [`published_cluster_versions`], so the engine-version catalog
/// `DescribeClusterVersions` reports and the parameter-group families
/// `CreateClusterParameterGroup` accepts cannot disagree.
pub fn default_parameter_groups() -> Vec<StoredParameterGroup> {
    published_cluster_versions()
        .into_iter()
        .filter_map(|version| {
            let engine_version = version.cluster_version?;
            let family = version
                .cluster_parameter_group_family
                .unwrap_or_else(|| parameter_group_family(&engine_version));
            Some(StoredParameterGroup {
                group: ClusterParameterGroup {
                    parameter_group_name: Some(default_parameter_group(&engine_version)),
                    parameter_group_family: Some(family.clone()),
                    description: Some(default_parameter_group_description(&family)),
                    tags: Some(Vec::new()),
                },
                default_group: true,
                // A default group reports its family's engine defaults and
                // nothing else: nobody has changed a value on it.
                parameter_values: BTreeMap::new(),
            })
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

/// What `ModifyClusterParameterGroup` reports as the group's
/// `ParameterGroupStatus`.
///
/// Source: `smithy_query(query="doc", shape="ClusterParameterGroupNameMessage",
/// member="ParameterGroupStatus")` — "if you made a change to a parameter group
/// name-value pair, then the change could be pending a reboot of an associated
/// cluster" — and the sentence the Amazon Redshift API/CLI reference prints for
/// `modify-cluster-parameter-group`, which is this one verbatim.
pub const PARAMETER_GROUP_STATUS_PENDING_REBOOT: &str =
    "Your parameter group has been updated but changes won't get applied until you reboot the associated Clusters.";

/// How many parameters one `ModifyClusterParameterGroup` request may carry.
///
/// Source: `smithy_query(query="doc", shape="ModifyClusterParameterGroupMessage",
/// member="Parameters")` — "A maximum of 20 parameters can be modified in a
/// single request."
pub const MAX_MODIFIABLE_PARAMETERS: usize = 20;

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
/// Source: the Amazon Redshift Cluster Management Guide, "Amazon Redshift
/// parameter groups": its default-parameter table lists exactly these settings
/// with these default values, which is what
/// `aws redshift describe-default-cluster-parameters --parameter-group-family
/// redshift-1.0` reports for the one family this service publishes. The same
/// guide is where each [`EngineParameter::apply_type`] comes from: a change to
/// `enable_case_sensitive_identifier`, `enable_user_activity_logging`,
/// `require_ssl`, `use_fips_ssl` or `wlm_json_configuration` is seen by an
/// associated cluster only after a reboot, while every other setting applies
/// dynamically. Every setting in the table is one a caller may change, hence
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
        description: "Requires SSL for every connection to the database.",
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
        description: "Aborts any statement that takes over the specified number of \
                      milliseconds; 0 turns the limit off.",
        data_type: DATA_TYPE_INTEGER,
        allowed_values: Some("0,100-2147483647"),
        apply_type: APPLY_TYPE_DYNAMIC,
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
/// publishes: [`ENGINE_DEFAULT_PARAMETERS`] for a family some published engine
/// version offers, and NOTHING for any other — a group of an unknown family
/// reports no parameters rather than another family's.
///
/// The family catalogue is read off [`published_cluster_versions`], the same
/// place `CreateClusterParameterGroup` checks a requested family against, so a
/// group that could be created always has parameters to report.
pub fn family_parameters(family: &str) -> &'static [EngineParameter] {
    let published = published_cluster_versions().into_iter().any(|version| {
        version.cluster_parameter_group_family.as_deref() == Some(family)
    });
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

/// `ClusterStatus` while the cluster is being provisioned.
pub const STATUS_CREATING: &str = "creating";
/// `ClusterStatus` / snapshot `Status` once provisioning is done.
pub const STATUS_AVAILABLE: &str = "available";
/// `ClusterStatus` a `DeleteCluster` response reports.
pub const STATUS_DELETING: &str = "deleting";

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

/// How many `SortingEntities` one `DescribeClusterSnapshots` request may carry.
///
/// `SnapshotAttributeToSortBy` declares three values, so a request naming more
/// entities than there are attributes can only repeat one; the cap keeps the
/// per-request sort work a small constant multiple of one pass over the store,
/// which matters because the sort runs while the shared state lock is held.
pub const MAX_SORTING_ENTITIES: usize = 3;

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
    /// When the `creating` -> `available` transition falls due.
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

/// Per-service state for com-amazonaws-redshift-redshiftserviceversion20121201. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsRedshiftRedshiftserviceversion20121201State {
    /// Every provisioned cluster, keyed by `ClusterIdentifier`, in creation order.
    pub clusters: Vec<StoredCluster>,
    /// Every snapshot, keyed by `SnapshotIdentifier`, in creation order.
    pub snapshots: Vec<StoredSnapshot>,
    /// Every cluster parameter group, keyed by `ParameterGroupName`, in creation
    /// order — the default parameter groups first, then the ones the account
    /// created.
    pub parameter_groups: Vec<StoredParameterGroup>,
    /// The engine versions this service offers, in catalog order. Unlike the
    /// collections above this one is SEEDED when the store is created — the
    /// versions exist before any cluster does, which is why
    /// `DescribeClusterVersions` can be called "even before creating any
    /// clusters" — and no operation adds to or removes from it.
    pub cluster_versions: Vec<ClusterVersion>,
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
            snapshots: Vec::new(),
            parameter_groups: default_parameter_groups(),
            cluster_versions: published_cluster_versions(),
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
        for stored in &mut self.clusters {
            if stored.cluster.cluster_status.as_deref() == Some(STATUS_CREATING)
                && now >= stored.available_at
            {
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
        }
        for stored in &mut self.snapshots {
            if stored.snapshot.status.as_deref() == Some(STATUS_CREATING)
                && now >= stored.available_at
            {
                stored.snapshot.status = Some(STATUS_AVAILABLE.to_string());
            }
        }
    }

    /// The stored cluster with this identifier, if any.
    pub fn cluster(&self, identifier: &str) -> Option<&StoredCluster> {
        self.clusters
            .iter()
            .find(|c| c.cluster.cluster_identifier.as_deref() == Some(identifier))
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
        self.snapshots.iter().find(|s| {
            identifier.is_some_and(|wanted| {
                s.snapshot
                    .snapshot_identifier
                    .as_deref()
                    .is_some_and(|stored| stored.eq_ignore_ascii_case(wanted))
            }) || snapshot_arn
                .is_some_and(|wanted| s.snapshot.snapshot_arn.as_deref() == Some(wanted))
        })
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
}

/// `ClusterAvailabilityStatus` for a `ClusterStatus`.
///
/// Source: `smithy_query(query="doc", shape="Cluster",
/// member="ClusterAvailabilityStatus")` — "Available - The cluster is available
/// for queries" and "Unavailable - The cluster is not available for queries". A
/// cluster still being provisioned, or on its way out, is not available for
/// queries at all, so it reports `Unavailable` rather than the `Modifying` value,
/// which the same doc reserves for a cluster that is "intermittently available …
/// due to changes that modify the cluster".
pub fn availability_status(cluster_status: &str) -> &'static str {
    match cluster_status {
        STATUS_AVAILABLE => "Available",
        _ => "Unavailable",
    }
}

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

/// Resolve one page of `total` records from the request's `Marker` / `MaxRecords`.
///
/// Returns the half-open range of the page and the `Marker` to hand back, which
/// is `Some` only when records remain after it.
pub fn page_bounds(
    marker: Option<&str>,
    max_records: Option<i32>,
    total: usize,
) -> Result<(usize, usize, Option<String>), String> {
    let page_size = match max_records {
        None => MAX_RECORDS_DEFAULT,
        Some(value) if (MAX_RECORDS_MIN..=MAX_RECORDS_MAX).contains(&value) => value,
        Some(value) => {
            return Err(format!(
                "Invalid value for MaxRecords: {value}. MaxRecords must be between {MAX_RECORDS_MIN} and {MAX_RECORDS_MAX}."
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
/// A Region's zones are `<region><suffix>`; the emulator presents three, which
/// is the smallest count AWS publishes for a Region, so every zone name it hands
/// out has the shape a caller parses. Derived from the Region alone, so the same
/// Region always reports the same zones.
pub fn region_availability_zones(region: &str) -> Vec<String> {
    ZONE_SUFFIXES
        .iter()
        .map(|suffix| format!("{region}{suffix}"))
        .collect()
}

/// The suffixes [`region_availability_zones`] appends to a Region name.
const ZONE_SUFFIXES: [char; 3] = ['a', 'b', 'c'];

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
/// Only a cluster created with a `ClusterSubnetGroupName` is in a VPC, so this
/// is called only for one.
pub fn derived_vpc_id(cluster_subnet_group_name: &str) -> String {
    format!(
        "vpc-{}",
        short_digest(&format!("vpc/{cluster_subnet_group_name}"), 17)
    )
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
pub fn snapshot_of(
    cluster: &Cluster,
    snapshot_identifier: &str,
    created_at: ::aws_smithy_types::DateTime,
    manual_retention_period: i32,
    tags: Vec<crate::ops::types::Tag>,
    region: &str,
) -> Snapshot {
    let cluster_identifier = cluster.cluster_identifier.clone().unwrap_or_default();
    Snapshot {
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
        manual_snapshot_retention_period: Some(manual_retention_period),
        master_password_secret_arn: cluster.master_password_secret_arn.clone(),
        master_password_secret_kms_key_id: cluster.master_password_secret_kms_key_id.clone(),
        snapshot_arn: Some(snapshot_arn(
            &cluster_identifier,
            snapshot_identifier,
            region,
        )),
        ..Default::default()
    }
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
    Ok((kind.to_string(), nodes))
}

/// Whether a resource's tags satisfy a request's `TagKeys` / `TagValues` filter.
///
/// Each filter is independent and inclusive: a resource passes `TagKeys` when it
/// carries a tag whose key is one of them, and `TagValues` when it carries a tag
/// whose value is one of them. An absent filter constrains nothing.
pub fn tags_match(
    tags: &[crate::ops::types::Tag],
    tag_keys: Option<&[String]>,
    tag_values: Option<&[String]>,
) -> bool {
    if let Some(keys) = tag_keys {
        if !keys.is_empty()
            && !tags
                .iter()
                .any(|t| t.key.as_ref().is_some_and(|k| keys.contains(k)))
        {
            return false;
        }
    }
    if let Some(values) = tag_values {
        if !values.is_empty()
            && !tags
                .iter()
                .any(|t| t.value.as_ref().is_some_and(|v| values.contains(v)))
        {
            return false;
        }
    }
    true
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
