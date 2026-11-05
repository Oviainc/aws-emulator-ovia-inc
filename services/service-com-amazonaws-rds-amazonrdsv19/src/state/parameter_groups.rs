// SPDX-License-Identifier: CC0-1.0

use super::*;

/// The prefix of every parameter group name Amazon RDS provides ITSELF.
///
/// Alongside each engine-version family RDS creates a default DB parameter group and a
/// default DB cluster parameter group that no request creates, named `default.` followed by
/// that family ([`super::default_db_parameter_group_name`], `default.mysql8.0`). The dot is
/// outside the alphabet a created group's name may use
/// ([`super::validate_parameter_group_name`]), so a name of this form can only ever be one
/// of RDS's own groups and never a record this store holds.
pub const DEFAULT_PARAMETER_GROUP_PREFIX: &str = "default.";

/// Whether `name` names one of the parameter groups Amazon RDS provides itself.
///
/// Those groups EXIST for every family the engine-version catalogue publishes, so a request
/// that names one names a group that is there — which is why a name of this form is not
/// answered as missing — but they are RDS's, not the account's, so a request that would
/// change one is refused ([`CANNOT_MODIFY_DEFAULT_PARAMETER_GROUP_MESSAGE`]). The name is
/// compared without case, the way a group name is stored.
pub fn is_default_parameter_group_name(name: &str) -> bool {
    name.to_ascii_lowercase()
        .starts_with(DEFAULT_PARAMETER_GROUP_PREFIX)
}

/// How Amazon RDS refuses a request that would change one of the parameter groups it
/// provides itself: ONE fixed sentence, which is why it is a constant — it names neither the
/// group nor the member, so the same text answers every such request, for an instance group
/// and a cluster group alike.
pub const CANNOT_MODIFY_DEFAULT_PARAMETER_GROUP_MESSAGE: &str =
    "Cannot modify a default parameter group.";

/// How Amazon RDS words the DB-cluster-parameter-group not-found fault: the `<type> not
/// found: <name>` form it uses for a resource a request NAMED, carrying the request's own
/// spelling of the name. Stated once here so every operation that resolves a cluster
/// parameter group answers the one text.
pub fn db_cluster_parameter_group_not_found_message(named: &str) -> String {
    format!("DBClusterParameterGroup not found: {named}")
}

/// How Amazon RDS words the DB-parameter-group not-found fault; the instance-scoped sibling
/// of [`db_cluster_parameter_group_not_found_message`].
pub fn db_parameter_group_not_found_message(named: &str) -> String {
    format!("DBParameterGroup not found: {named}")
}

impl ComAmazonawsRdsAmazonrdsv19State {
    /// Whether the DB CLUSTER parameter group `named` is one a request may associate a
    /// cluster with in `region`: either a group this region holds, or one of the default
    /// groups RDS provides ([`is_default_parameter_group_name`]), which no request creates
    /// and this store therefore has no record of.
    pub fn db_cluster_parameter_group_exists(&self, region: &str, named: &str) -> bool {
        let folded = named.to_ascii_lowercase();
        is_default_parameter_group_name(&folded)
            || self
                .db_cluster_parameter_groups
                .contains_key(&(region.to_string(), folded))
    }

    /// Whether the DB parameter group `named` is one a request may associate an instance
    /// with in `region`; the instance-scoped sibling of
    /// [`Self::db_cluster_parameter_group_exists`].
    pub fn db_parameter_group_exists(&self, region: &str, named: &str) -> bool {
        let folded = named.to_ascii_lowercase();
        is_default_parameter_group_name(&folded)
            || self
                .db_parameter_groups
                .contains_key(&(region.to_string(), folded))
    }
}

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
        let spec = crate::ops::describe_engine_default_cluster_parameters::cluster_parameter_spec(
            &self.family,
            name,
        );
        Parameter {
            parameter_name: Some(name.to_string()),
            parameter_value: setting.map(|setting| setting.value.clone()),
            description: spec.map(|spec| spec.description.to_string()),
            source: Some(PARAMETER_SOURCE_USER.to_string()),
            apply_type: spec.map(|spec| spec.apply_type.to_string()),
            data_type: spec.map(|spec| spec.data_type.to_string()),
            allowed_values: spec.map(|spec| spec.allowed_values.to_string()),
            is_modifiable: spec.map(|spec| spec.is_modifiable),
            // Answered about THIS group's family; see
            // [`cluster_engine_default_parameter_to_wire`].
            minimum_engine_version: spec
                .map(|spec| minimum_engine_version_in_family(&self.family, spec)),
            apply_method: setting.map(|setting| setting.apply_method.clone()),
            supported_engine_modes: Some(
                crate::ops::describe_engine_default_cluster_parameters::family_engine_modes(
                    &self.family,
                ),
            ),
        }
    }

    /// Every parameter of this group at once, which is what a describe that names no
    /// `Source` reports: each parameter the group's family publishes, under the value and
    /// apply method the GROUP sets for it where it sets one (`Source` `user`) and under the
    /// engine's own default where it does not (`Source` `engine-default`), plus any
    /// parameter the group carries that the family publishes nothing for.
    ///
    /// A group inherits its family's whole parameter list the moment it is created — "the
    /// detailed parameter list for a particular DB cluster parameter group" is that list,
    /// with each parameter's source saying where its current value came from — so a group
    /// nobody has modified reports the family's parameters at their engine defaults rather
    /// than nothing at all.
    ///
    /// Keyed by parameter name, so the report is in name order whichever half an entry came
    /// from — the order its pagination marker walks.
    pub fn every_parameter(&self) -> BTreeMap<String, Parameter> {
        let mut reported = self.published_defaults();
        for name in self.parameters.keys() {
            reported.insert(name.clone(), self.parameter_to_wire(name));
        }
        reported
    }

    /// The parameters this group's family publishes, each with the default value the engine
    /// runs with and `Source` `engine-default`: what a `Source` naming the engine reports,
    /// and the half of [`Self::every_parameter`] the group has set nothing for.
    pub fn published_defaults(&self) -> BTreeMap<String, Parameter> {
        crate::ops::describe_engine_default_cluster_parameters::cluster_parameter_catalog(
            &self.family,
        )
        .unwrap_or_default()
        .iter()
        .map(|spec| {
            (
                spec.name.to_string(),
                cluster_engine_default_parameter_to_wire(&self.family, spec),
            )
        })
        .collect()
    }
}

/// A persisted DB (instance) parameter group.
///
/// The name is stored lowercased and is half of this record's key:
/// `CreateDBParameterGroupMessage$DBParameterGroupName` states "This value is stored as a
/// lowercase string."
#[derive(Debug, Clone)]
pub struct DbParameterGroupRecord {
    /// The lowercased `DBParameterGroupName`.
    pub name: String,
    /// `DBParameterGroupFamily`, as the caller named it.
    pub family: String,
    /// `Description`.
    pub description: String,
    /// The region whose credential scope the create was signed with.
    pub region: String,
    /// The tags `CreateDBParameterGroup$Tags` assigned to the group. The modelled
    /// `DBParameterGroup` shape carries no tag member, so nothing reports them yet —
    /// `ListTagsForResource`, which does, is still scaffolded — but the caller's input is
    /// kept rather than dropped, so it is here when that operation lands.
    pub tags: Vec<Tag>,
    /// The parameters a caller has set on the group, by `ParameterName`, exactly as the
    /// cluster-level groups above carry theirs: `ModifyDBParameterGroup` writes them and
    /// `DescribeDBParameters` reports them with `Source` [`PARAMETER_SOURCE_USER`]. A
    /// freshly created group carries NONE — the engine's own defaults belong to the FAMILY,
    /// which publishes them ([`published_db_parameters`]), and are not materialised into a
    /// group.
    pub parameters: BTreeMap<String, ParameterSetting>,
}

impl DbParameterGroupRecord {
    /// `DBParameterGroupArn`. The resource type is `pg`, the one AWS uses for a DB
    /// parameter group under this service's `rds` ARN namespace.
    pub fn arn(&self) -> String {
        emulator_server::emulator_arn(ARN_NAMESPACE, &self.region, &format!("pg:{}", self.name))
    }

    /// This record as the modelled `DBParameterGroup`.
    pub fn to_wire(&self) -> DBParameterGroup {
        DBParameterGroup {
            db_parameter_group_name: Some(self.name.clone()),
            db_parameter_group_family: Some(self.family.clone()),
            description: Some(self.description.clone()),
            db_parameter_group_arn: Some(self.arn()),
        }
    }

    /// The parameter `name` as the modelled `Parameter`: the value and apply method the
    /// caller set, plus the metadata this group's family publishes for it when it publishes
    /// any — the instance-level counterpart of
    /// [`DbClusterParameterGroupRecord::parameter_to_wire`].
    pub fn parameter_to_wire(&self, name: &str) -> Parameter {
        let setting = self.parameters.get(name);
        let spec = published_db_parameter(&self.family, name);
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

/// The parameters the DB (instance) parameter group family `family` publishes, or `None`
/// when this emulator publishes none for it.
///
/// This is the catalog `DescribeEngineDefaultParameters` reports for the same family — one
/// family has ONE published parameter list, whichever operation is asked — so the two can
/// never disagree about what a parameter admits. It is a SAMPLE of that list rather than the
/// whole of it (an engine publishes hundreds of parameters per family), which is why
/// [`validate_db_parameter_setting`] checks the value of a parameter the catalog carries and
/// admits any name it does not.
///
/// A family the catalog is empty for, and a string that names no family at all, publish
/// nothing.
///
/// The cluster-level catalog of a family is the separate [`published_parameters`], which the
/// DB CLUSTER parameter group operations read.
pub fn published_db_parameters(family: &str) -> Option<&'static [ClusterParameterSpec]> {
    crate::ops::describe_engine_default_parameters::engine_default_parameters(family)
        .filter(|published| !published.is_empty())
}

/// The spec the DB (instance) parameter group family `family` publishes for the parameter
/// named `name`, if any; see [`published_db_parameters`].
pub fn published_db_parameter(family: &str, name: &str) -> Option<&'static ClusterParameterSpec> {
    spec_in_catalog(published_db_parameters(family), name)
}

/// Refuse a setting a DB (instance) parameter group of `family` cannot carry: a parameter
/// the family publishes that the engine does not let a caller modify, or a value outside the
/// ones that parameter admits.
///
/// A name the published catalog does not carry is ADMITTED, because that catalog is a sample
/// of the family's published list rather than the whole of it: see
/// [`published_db_parameters`] and [`validate_known_setting`], which carries the rule. The
/// cluster-level rule is [`validate_parameter_setting`], over a catalog that IS its family's
/// whole list.
pub fn validate_db_parameter_setting(
    family: &str,
    name: &str,
    value: &str,
) -> Result<(), String> {
    validate_known_setting(published_db_parameters(family), name, value)
}

/// The cluster parameters the DB CLUSTER parameter group family `family` publishes, or
/// `None` when this emulator publishes none for it.
///
/// This is the catalog `DescribeEngineDefaultClusterParameters` reports for the family and
/// the one a GROUP of that family reports its parameters' metadata from, so what a value is
/// validated against and what a describe publishes as that parameter's `AllowedValues` can
/// never disagree. It is a SAMPLE of the engine's published list rather than the whole of it,
/// which is why [`validate_cluster_parameter_setting`] checks the value of a parameter the
/// catalog carries and admits any name it does not.
///
/// The instance-level counterpart is [`published_db_parameters`].
pub fn published_cluster_parameters(family: &str) -> Option<&'static [ClusterParameterSpec]> {
    crate::ops::describe_engine_default_cluster_parameters::cluster_parameter_catalog(family)
}

/// Refuse a setting a DB CLUSTER parameter group of `family` cannot carry: a parameter the
/// family publishes that the engine does not let a caller modify, or a value outside the ones
/// that parameter admits — the `AllowedValues` its own published catalog entry carries,
/// whichever engine's family it is.
///
/// Two catalogs answer for a family and both are consulted, because each is the whole of what
/// it publishes: the shared one of [`validate_parameter_setting`], which IS its families'
/// complete parameter list and so refuses a name outside it, and the family's own published
/// cluster catalog ([`published_cluster_parameters`]), a sample in which an unpublished name
/// is admitted and a published one's value is checked ([`validate_known_setting`] carries
/// that rule).
///
/// `Err` carries the message to answer with, built from the request and from that parameter's
/// catalog entry; the instance-level rule is [`validate_db_parameter_setting`].
pub fn validate_cluster_parameter_setting(
    family: &str,
    name: &str,
    value: &str,
) -> Result<(), String> {
    validate_parameter_setting(family, name, value)?;
    validate_known_setting(published_cluster_parameters(family), name, value)
}

/// The apply method a change to a DB (instance) parameter gets when the caller asks for
/// none: the one the parameter's own apply type implies; see
/// [`service_common::rds_cluster_parameters::default_apply_method`] for the documented rule.
pub fn db_parameter_default_apply_method(family: &str, name: &str) -> &'static str {
    default_apply_method_for(published_db_parameter(family, name))
}

/// A parameter the engine publishes for a family, as the modelled `Parameter` with its
/// default value and `Source` [`PARAMETER_SOURCE_ENGINE_DEFAULT`]: what a describe reports
/// when the caller asks for the engine's own parameters rather than a group's.
///
/// `MinimumEngineVersion` is reported only where the catalog publishes the version the
/// parameter was introduced in; an entry that names none reports none rather than a guessed
/// version, since "the earliest engine version to which the parameter can apply" is a fact
/// about the engine's release history and not something this front door can derive.
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
        minimum_engine_version: (!spec.minimum_engine_version.is_empty())
            .then(|| spec.minimum_engine_version.to_string()),
        apply_method: None,
        supported_engine_modes: None,
    }
}

/// A CLUSTER parameter the family `family` publishes, as the modelled `Parameter`: what
/// [`engine_default_parameter_to_wire`] reports plus `SupportedEngineModes`, "the valid DB
/// engine modes", which a cluster parameter has and an instance parameter does not.
///
/// `MinimumEngineVersion` is answered about the FAMILY the report is for, not about the
/// engine as a whole: the earliest version the parameter can apply at inside that family is
/// the later of the version it was introduced in and the family's own first release
/// (`service_common::rds_cluster_parameters::minimum_engine_version_in_family`). A family
/// whose first release this front door does not publish keeps the introduction version
/// unchanged.
pub fn cluster_engine_default_parameter_to_wire(
    family: &str,
    spec: &ClusterParameterSpec,
) -> Parameter {
    Parameter {
        supported_engine_modes: Some(
            crate::ops::describe_engine_default_cluster_parameters::family_engine_modes(family),
        ),
        minimum_engine_version: (!spec.minimum_engine_version.is_empty())
            .then(|| minimum_engine_version_in_family(family, spec)),
        ..engine_default_parameter_to_wire(spec)
    }
}
