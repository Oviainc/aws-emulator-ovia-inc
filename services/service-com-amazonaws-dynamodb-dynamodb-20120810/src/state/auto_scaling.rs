// SPDX-License-Identifier: CC0-1.0

//! The AUTO SCALING settings of a table's replicas: the capacity window and the
//! target-tracking policy each capacity of each regional table is scaled by.
//!
//! "Updates auto scaling settings on your global tables at once" and "describes auto
//! scaling settings across replicas of the global table at once" (the
//! `UpdateTableReplicaAutoScaling` and `DescribeTableReplicaAutoScaling` docs), so the
//! settings are a per-REPLICA record: the Region a replica lives in is what addresses its
//! read-capacity settings, and the write-capacity settings the request states for the
//! table belong to the whole replication group — which is why the update carries
//! `ReplicaUpdates` per Region and `ProvisionedWriteCapacityAutoScalingUpdate` once for
//! the table. Each record is held on the regional table it scales
//! ([`super::Table::auto_scaling`]), so it is created, copied into a new replica and
//! removed with the table itself.
//!
//! A capacity nothing has configured has NO record, which is what the describing operation
//! reports by leaving that capacity's settings out: the settings are what a caller
//! configured, never a default this emulator invents.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::{
    AutoScalingPolicyDescription, AutoScalingSettingsDescription, AutoScalingSettingsUpdate,
    AutoScalingTargetTrackingScalingPolicyConfigurationDescription, ReplicaAutoScalingDescription,
    ReplicaGlobalSecondaryIndexAutoScalingDescription, TableAutoScalingDescription,
    UpdateTableReplicaAutoScalingInput,
};

use super::{ComAmazonawsDynamodbDynamodb20120810State, RequestError, Table};

/// Which capacity of a scaled resource one settings record steers.
///
/// A provisioned table — and each of its global secondary indexes — has two capacities,
/// provisioned and scaled independently of each other, which is why the update states the
/// read one per replica and the write one for the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AutoScalingCapacity {
    /// The read capacity, named by `ReplicaAutoScalingUpdate`'s
    /// `ReplicaProvisionedReadCapacityAutoScalingUpdate`.
    Read,
    /// The write capacity, named by `UpdateTableReplicaAutoScaling`'s
    /// `ProvisionedWriteCapacityAutoScalingUpdate`.
    Write,
}

impl AutoScalingCapacity {
    /// The name DynamoDB gives the target-tracking policy it manages for this capacity:
    /// the capacity's own utilisation metric, then the resource the policy steers — a
    /// table, or one global secondary index of it.
    ///
    /// Used for a `ScalingPolicyUpdate` that names no `PolicyName`, so the policy a
    /// request leaves unnamed still has the one identity under its target that makes a
    /// later configuration of the same capacity an update of it rather than a second
    /// policy on one metric.
    fn default_policy_name(self, table_name: &str, index_name: Option<&str>) -> String {
        let metric = match self {
            Self::Read => "DynamoDBReadCapacityUtilization",
            Self::Write => "DynamoDBWriteCapacityUtilization",
        };
        match index_name {
            Some(index_name) => format!("{metric}:table/{table_name}/index/{index_name}"),
            None => format!("{metric}:table/{table_name}"),
        }
    }
}

/// WHAT one settings record scales: one capacity of the replica's table, or that capacity
/// of one of its global secondary indexes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct AutoScalingTarget {
    /// The global secondary index whose capacity this record scales, or `None` for the
    /// table's own.
    pub index_name: Option<String>,
    /// Which of the resource's two capacities it scales.
    pub capacity: AutoScalingCapacity,
}

impl AutoScalingTarget {
    /// One capacity of the table itself, or — with an index name — of that index.
    pub fn new(capacity: AutoScalingCapacity, index_name: Option<&str>) -> Self {
        Self {
            index_name: index_name.map(ToString::to_string),
            capacity,
        }
    }
}

/// One capacity's auto scaling settings as a caller configured them: the window the
/// capacity is kept inside, whether scaling is off, the role it is performed under and the
/// one target-tracking policy that steers it.
///
/// Every member is as the request left it — a member no update has ever named is absent
/// rather than defaulted, because the settings are the caller's own statement about the
/// capacity and nothing else configures them.
#[derive(Debug, Clone, Default)]
pub struct AutoScalingSettings {
    /// "The minimum capacity units that a global table or global secondary index should
    /// be scaled down to" (`AutoScalingSettingsDescription$MinimumUnits`).
    pub minimum_units: Option<i64>,
    /// "The maximum capacity units that a global table or global secondary index should be
    /// scaled up to" (that shape's `MaximumUnits`).
    pub maximum_units: Option<i64>,
    /// "Disabled auto scaling for this global table or global secondary index" (its
    /// `AutoScalingDisabled`).
    pub auto_scaling_disabled: Option<bool>,
    /// "Role ARN used for configuring the auto scaling policy" (its
    /// `AutoScalingRoleArn`).
    pub auto_scaling_role_arn: Option<String>,
    /// The target-tracking policy steering this capacity, or `None` while no update has
    /// configured one.
    ///
    /// At most ONE: the update carries a single `ScalingPolicyUpdate` per capacity and a
    /// policy's name is its identity under its target, so configuring the capacity again
    /// replaces the policy rather than adding a second one to the same metric.
    pub policy: Option<AutoScalingPolicy>,
}

/// The target-tracking policy one capacity is scaled by: its name, and the configuration
/// that says what it tracks and how it is allowed to move.
#[derive(Debug, Clone)]
pub struct AutoScalingPolicy {
    /// "The name of the scaling policy" (`AutoScalingPolicyDescription$PolicyName`) — the
    /// one the update named, or the name DynamoDB manages this capacity's policy under
    /// ([`AutoScalingCapacity::default_policy_name`]).
    pub policy_name: String,
    /// "Indicates whether scale in by the target tracking policy is disabled."
    pub disable_scale_in: Option<bool>,
    /// "The amount of time, in seconds, after a scale in activity completes before another
    /// scale in activity can start."
    pub scale_in_cooldown: Option<i32>,
    /// "The amount of time, in seconds, after a scale out activity completes before
    /// another scale out activity can start."
    pub scale_out_cooldown: Option<i32>,
    /// "The target value for the metric" — the only member the configuration requires.
    pub target_value: f64,
}

/// The service principal DynamoDB's provisioned capacity is scaled through: Application
/// Auto Scaling's principal under this service's own label, which is the principal the
/// role below is linked to.
const AUTO_SCALING_SERVICE_PRINCIPAL: &str = "dynamodb.application-autoscaling.amazonaws.com";

/// The name of the service-linked role that scales a DynamoDB table's provisioned
/// capacity, as the Application Auto Scaling User Guide publishes it for this service
/// ("Service-linked roles for Application Auto Scaling": one role per supported service,
/// each named `AWSServiceRoleForApplicationAutoScaling_<resource type>`).
const AUTO_SCALING_SERVICE_LINKED_ROLE: &str = "AWSServiceRoleForApplicationAutoScaling_DynamoDBTable";

/// The role ARN a configured capacity's scaling is PERFORMED under when the request that
/// configured it named none: the service-linked role Application Auto Scaling scales
/// DynamoDB tables through, which is "created if it does not yet exist" rather than
/// supplied per call — so a settings record always reports the role in force.
pub fn auto_scaling_service_linked_role_arn() -> String {
    ::engine::emulator_arn(
        "iam",
        "",
        &format!(
            "role/aws-service-role/{AUTO_SCALING_SERVICE_PRINCIPAL}/\
             {AUTO_SCALING_SERVICE_LINKED_ROLE}"
        ),
    )
}

/// The role ARN one auto scaling update names that this service cannot scale a replica
/// through, or `None` when every settings update the request carries names the role that
/// performs the scaling, or no role at all.
///
/// A replica's capacities are scaled through the service-linked role
/// ([`auto_scaling_service_linked_role_arn`]), which the service assumes on its own
/// behalf and "creates if it does not yet exist" rather than taking per call. Scaling them
/// through any OTHER role would mean PASSING that role to the scaling service, which the
/// caller has to be authorized to do ([`auto_scaling_role_pass_denied`]).
pub fn unpassable_auto_scaling_role(input: &UpdateTableReplicaAutoScalingInput) -> Option<String> {
    let in_force = auto_scaling_service_linked_role_arn();
    named_auto_scaling_roles(input)
        .into_iter()
        .find(|named| named != &in_force)
}

/// The refusal a request that names a role of its own earns: the authorization failure the
/// pass of that role answers, as the scaling service raised it.
///
/// A role named in the request is passed to the service that would scale the capacity with
/// it, which the calling principal must hold `iam:PassRole` on that role to do; nothing
/// grants a caller of this emulator that permission over a role of its own, so the pass is
/// refused and the refusal is what the request is answered with. Worded by the one
/// statement of this refusal class in this crate
/// ([`super::authorization::not_authorized`]), so it reads exactly as the data plane's own
/// authorization refusals do.
pub fn auto_scaling_role_pass_denied(caller_arn: &str, role_arn: &str) -> String {
    const PASS_ROLE: &str = "iam:PassRole";
    format!(
        "AccessDeniedException: {}",
        super::authorization::not_authorized(
            caller_arn,
            PASS_ROLE,
            role_arn,
            &format!("because no identity-based policy allows the {PASS_ROLE} action"),
        )
    )
}

/// Every `AutoScalingRoleArn` one request names, across the table's own write capacity,
/// each index's, and each replica's read capacity and its indexes' — the member can be
/// carried by every settings update in the request, so they are all held to the same rule.
fn named_auto_scaling_roles(input: &UpdateTableReplicaAutoScalingInput) -> Vec<String> {
    let table_write = input
        .provisioned_write_capacity_auto_scaling_update
        .iter()
        .chain(
            input
                .global_secondary_index_updates
                .iter()
                .flatten()
                .filter_map(|index| {
                    index
                        .provisioned_write_capacity_auto_scaling_update
                        .as_ref()
                }),
        );
    let replica_read = input.replica_updates.iter().flatten().flat_map(|replica| {
        replica
            .replica_provisioned_read_capacity_auto_scaling_update
            .iter()
            .chain(
                replica
                    .replica_global_secondary_index_updates
                    .iter()
                    .flatten()
                    .filter_map(|index| {
                        index.provisioned_read_capacity_auto_scaling_update.as_ref()
                    }),
            )
    });
    table_write
        .chain(replica_read)
        .filter_map(|update| update.auto_scaling_role_arn.clone())
        .collect()
}

impl AutoScalingSettings {
    /// Apply one `AutoScalingSettingsUpdate` to these settings: a member the update names
    /// becomes the setting in force, and a member it leaves out keeps the value it had.
    ///
    /// "Represents the auto scaling settings to be MODIFIED" (the update shape's doc): the
    /// update states the changes rather than the whole record, so a second call that names
    /// only the window leaves the policy configured by the first in place.
    fn apply(
        &mut self,
        update: &AutoScalingSettingsUpdate,
        target: &AutoScalingTarget,
        table_name: &str,
    ) {
        if let Some(minimum_units) = update.minimum_units {
            self.minimum_units = Some(minimum_units);
        }
        if let Some(maximum_units) = update.maximum_units {
            self.maximum_units = Some(maximum_units);
        }
        if let Some(disabled) = update.auto_scaling_disabled {
            self.auto_scaling_disabled = Some(disabled);
        }
        if let Some(role_arn) = update.auto_scaling_role_arn.clone() {
            self.auto_scaling_role_arn = Some(role_arn);
        }
        if let Some(policy) = update.scaling_policy_update.as_ref() {
            let configuration = &policy.target_tracking_scaling_policy_configuration;
            self.policy = Some(AutoScalingPolicy {
                policy_name: policy.policy_name.clone().unwrap_or_else(|| {
                    target
                        .capacity
                        .default_policy_name(table_name, target.index_name.as_deref())
                }),
                disable_scale_in: configuration.disable_scale_in,
                scale_in_cooldown: configuration.scale_in_cooldown,
                scale_out_cooldown: configuration.scale_out_cooldown,
                target_value: configuration.target_value,
            });
        }
    }

    /// These settings as `AutoScalingSettingsDescription` reports them.
    ///
    /// `AutoScalingRoleArn` is the role the scaling is performed under: the one the
    /// request named, or the service-linked role that scales this service's capacities
    /// when it named none ([`auto_scaling_service_linked_role_arn`]) — a configured
    /// capacity is always scaled by SOME role, so the member is reported either way.
    ///
    /// `ScalingPolicies` is the policy steering this capacity, and an EMPTY list for a
    /// capacity configured with a window but no policy: "information about the scaling
    /// policies" of a capacity that has none is the fact that there are none.
    pub fn describe(&self) -> AutoScalingSettingsDescription {
        AutoScalingSettingsDescription {
            minimum_units: self.minimum_units,
            maximum_units: self.maximum_units,
            auto_scaling_disabled: self.auto_scaling_disabled,
            auto_scaling_role_arn: Some(
                self.auto_scaling_role_arn
                    .clone()
                    .unwrap_or_else(auto_scaling_service_linked_role_arn),
            ),
            scaling_policies: Some(
                self.policy
                    .iter()
                    .map(|policy| AutoScalingPolicyDescription {
                        policy_name: Some(policy.policy_name.clone()),
                        target_tracking_scaling_policy_configuration: Some(
                            AutoScalingTargetTrackingScalingPolicyConfigurationDescription {
                                disable_scale_in: policy.disable_scale_in,
                                scale_in_cooldown: policy.scale_in_cooldown,
                                scale_out_cooldown: policy.scale_out_cooldown,
                                target_value: policy.target_value,
                            },
                        ),
                    })
                    .collect(),
            ),
        }
    }
}

/// Every auto scaling record ONE regional table holds: one per capacity of the table and
/// of each of its global secondary indexes that a caller has configured.
///
/// One map rather than a field per capacity, so a reader and a writer address a record by
/// the same key ([`AutoScalingTarget`]) however many indexes the table carries, and a
/// capacity nothing configured simply has no entry.
#[derive(Debug, Clone, Default)]
pub struct TableAutoScaling {
    settings: BTreeMap<AutoScalingTarget, AutoScalingSettings>,
}

impl TableAutoScaling {
    /// The settings in force for one capacity, or `None` while nothing has configured it.
    pub fn settings(&self, target: &AutoScalingTarget) -> Option<&AutoScalingSettings> {
        self.settings.get(target)
    }

    /// The record one update is applied to, empty the first time that capacity is
    /// configured.
    fn settings_mut(&mut self, target: AutoScalingTarget) -> &mut AutoScalingSettings {
        self.settings.entry(target).or_default()
    }

    /// Apply one capacity's update, under the table name whose managed policy name an
    /// unnamed policy takes.
    fn apply(
        &mut self,
        target: AutoScalingTarget,
        table_name: &str,
        update: &AutoScalingSettingsUpdate,
    ) {
        let key = target.clone();
        self.settings_mut(target).apply(update, &key, table_name);
    }
}

impl Table {
    /// This replica's auto scaling settings as `ReplicaAutoScalingDescription` reports
    /// them: its own read and write capacity records, and one entry per global secondary
    /// index the table carries.
    ///
    /// `status` is the replica's own `ReplicaStatus`, which the group's record of it holds
    /// rather than the regional table ([`super::TableReplica::status`]).
    fn describe_replica_auto_scaling(
        &self,
        status: &str,
        now: SystemTime,
    ) -> ReplicaAutoScalingDescription {
        let indexes: Vec<ReplicaGlobalSecondaryIndexAutoScalingDescription> = self
            .global_secondary_indexes
            .iter()
            .map(|index| ReplicaGlobalSecondaryIndexAutoScalingDescription {
                index_name: Some(index.name.clone()),
                index_status: Some(index.status(now).as_str().to_string()),
                provisioned_read_capacity_auto_scaling_settings: self
                    .auto_scaling
                    .settings(&AutoScalingTarget::new(
                        AutoScalingCapacity::Read,
                        Some(&index.name),
                    ))
                    .map(AutoScalingSettings::describe),
                provisioned_write_capacity_auto_scaling_settings: self
                    .auto_scaling
                    .settings(&AutoScalingTarget::new(
                        AutoScalingCapacity::Write,
                        Some(&index.name),
                    ))
                    .map(AutoScalingSettings::describe),
            })
            .collect();
        ReplicaAutoScalingDescription {
            region_name: Some(self.region.clone()),
            // "Replica-specific global secondary index auto scaling settings": no list at
            // all for a table that carries no global secondary index, exactly as its
            // description reports no index list.
            global_secondary_indexes: (!indexes.is_empty()).then_some(indexes),
            replica_provisioned_read_capacity_auto_scaling_settings: self
                .auto_scaling
                .settings(&AutoScalingTarget::new(AutoScalingCapacity::Read, None))
                .map(AutoScalingSettings::describe),
            replica_provisioned_write_capacity_auto_scaling_settings: self
                .auto_scaling
                .settings(&AutoScalingTarget::new(AutoScalingCapacity::Write, None))
                .map(AutoScalingSettings::describe),
            replica_status: Some(status.to_string()),
        }
    }
}

/// How an auto scaling update that names no `IndexName` is refused: the entry addresses
/// "the global secondary index" whose capacity is to be scaled, so an entry without the
/// name addresses nothing.
fn index_name_required() -> RequestError {
    RequestError::Validation(
        "One or more parameter values were invalid: IndexName is required to scale a global \
         secondary index"
            .to_string(),
    )
}

impl ComAmazonawsDynamodbDynamodb20120810State {
    /// The Regions one table's auto scaling settings span: the Region of the addressed
    /// table, which is a replica of the group itself, and every Region its replication
    /// group records beside it ([`Table::replicas`]).
    ///
    /// A table that belongs to no group spans its own Region alone — the one regional
    /// table whose capacities there are to scale.
    fn auto_scaling_regions(&self, region: &str, name: &str) -> Result<Vec<String>, RequestError> {
        let table = self.table(region, name)?;
        let mut regions = vec![region.to_string()];
        regions.extend(table.replicas.iter().map(|replica| replica.region.clone()));
        Ok(regions)
    }

    /// The `ReplicaStatus` the addressed table's record holds for one Region of its group:
    /// the local Region's is the table's own status, and every other Region's is the one
    /// the replica record reports ([`super::TableReplica::status`]).
    fn replica_auto_scaling_status(
        &self,
        table: &Table,
        replica_region: &str,
        now: SystemTime,
    ) -> String {
        if replica_region == table.region {
            return table.status(now).as_str().to_string();
        }
        table
            .replica(replica_region)
            .map_or(super::REPLICA_ACTIVE, |replica| replica.status(now))
            .to_string()
    }

    /// One table's auto scaling settings across the replicas of its group, as
    /// `DescribeTableReplicaAutoScaling` reports them.
    ///
    /// Each replica's settings are read from the regional table that holds them, so a
    /// Region whose read capacity was configured on its own reports its own window and
    /// policy while the group's write settings are the same on every replica.
    pub fn described_replica_auto_scaling(
        &self,
        region: &str,
        name: &str,
        now: SystemTime,
    ) -> Result<TableAutoScalingDescription, RequestError> {
        let table = self.table(region, name)?;
        let replicas: Vec<ReplicaAutoScalingDescription> = self
            .auto_scaling_regions(region, name)?
            .iter()
            // A Region the group records but whose regional table this store does not hold
            // has no capacities to report, so it is left out rather than reported from
            // another replica's record.
            .filter_map(|replica_region| {
                let held = self.table(replica_region, name).ok()?;
                let status = self.replica_auto_scaling_status(table, replica_region, now);
                Some(held.describe_replica_auto_scaling(&status, now))
            })
            .collect();
        Ok(TableAutoScalingDescription {
            table_name: Some(table.name.clone()),
            table_status: Some(table.status(now).as_str().to_string()),
            replicas: Some(replicas),
        })
    }

    /// Apply the auto scaling settings one `UpdateTableReplicaAutoScaling` states.
    ///
    /// The write capacity it names is the GROUP's — "updates auto scaling settings on your
    /// global tables", and the member is stated once for the table rather than per replica
    /// — so it is written to every regional table of the group; a `ReplicaUpdates` entry
    /// names a Region, so its read capacity is written to that Region's table alone. The
    /// same split holds for the global secondary indexes each member carries.
    ///
    /// Nothing is written until every Region and index the request names has been
    /// resolved, so a request that cannot apply in full applies in no part.
    pub fn apply_replica_auto_scaling(
        &mut self,
        region: &str,
        name: &str,
        input: &UpdateTableReplicaAutoScalingInput,
    ) -> Result<(), RequestError> {
        let regions = self.auto_scaling_regions(region, name)?;
        {
            // Every Region the settings are written to has to hold the regional table that
            // holds them, and every index the request names has to be one the group's
            // tables carry: resolved here, before anything is written, so a write meant for
            // the whole group cannot land in some Regions and be dropped in others.
            for replica_region in &regions {
                self.table(replica_region, name)?;
            }
            let table = self.table(region, name)?;
            for update in input.global_secondary_index_updates.iter().flatten() {
                check_scaled_index(table, update.index_name.as_deref())?;
            }
            for update in input.replica_updates.iter().flatten() {
                if !regions.iter().any(|held| held == &update.region_name) {
                    return Err(super::replica_not_in_group(&update.region_name));
                }
                for index in update.replica_global_secondary_index_updates.iter().flatten() {
                    check_scaled_index(table, index.index_name.as_deref())?;
                }
            }
        }

        if let Some(update) = input.provisioned_write_capacity_auto_scaling_update.as_ref() {
            self.scale_group(
                &regions,
                name,
                AutoScalingTarget::new(AutoScalingCapacity::Write, None),
                update,
            );
        }
        for index_update in input.global_secondary_index_updates.iter().flatten() {
            let (Some(index_name), Some(update)) = (
                index_update.index_name.as_deref(),
                index_update
                    .provisioned_write_capacity_auto_scaling_update
                    .as_ref(),
            ) else {
                continue;
            };
            self.scale_group(
                &regions,
                name,
                AutoScalingTarget::new(AutoScalingCapacity::Write, Some(index_name)),
                update,
            );
        }
        for replica_update in input.replica_updates.iter().flatten() {
            let replica_region = replica_update.region_name.clone();
            if let Some(update) = replica_update
                .replica_provisioned_read_capacity_auto_scaling_update
                .as_ref()
            {
                self.scale_replica(
                    &replica_region,
                    name,
                    AutoScalingTarget::new(AutoScalingCapacity::Read, None),
                    update,
                );
            }
            for index_update in replica_update
                .replica_global_secondary_index_updates
                .iter()
                .flatten()
            {
                let (Some(index_name), Some(update)) = (
                    index_update.index_name.as_deref(),
                    index_update
                        .provisioned_read_capacity_auto_scaling_update
                        .as_ref(),
                ) else {
                    continue;
                };
                self.scale_replica(
                    &replica_region,
                    name,
                    AutoScalingTarget::new(AutoScalingCapacity::Read, Some(index_name)),
                    update,
                );
            }
        }
        Ok(())
    }

    /// Write one capacity's update to every regional table of the group: the capacity it
    /// scales belongs to the global table rather than to one of its replicas.
    fn scale_group(
        &mut self,
        regions: &[String],
        name: &str,
        target: AutoScalingTarget,
        update: &AutoScalingSettingsUpdate,
    ) {
        for replica_region in regions {
            self.scale_replica(replica_region, name, target.clone(), update);
        }
    }

    /// Write one capacity's update to the regional table of ONE Region.
    ///
    /// The table is there: every Region this is called for was resolved before the first
    /// write ([`Self::apply_replica_auto_scaling`]), so nothing is dropped here.
    fn scale_replica(
        &mut self,
        region: &str,
        name: &str,
        target: AutoScalingTarget,
        update: &AutoScalingSettingsUpdate,
    ) {
        if let Ok(table) = self.table_mut(region, name) {
            table.auto_scaling.apply(target, name, update);
        }
    }
}

/// Hold the index an auto scaling update names to the indexes the table carries: the entry
/// addresses "the global secondary index" whose capacity is to be scaled, so a name no
/// index of the table answers to addresses nothing — "the operation tried to access a
/// nonexistent table or index" (the `ResourceNotFoundException` doc).
fn check_scaled_index(table: &Table, index_name: Option<&str>) -> Result<(), RequestError> {
    let Some(index_name) = index_name else {
        return Err(index_name_required());
    };
    if table.index(index_name).is_none() {
        return Err(super::index_not_found(&table.name, index_name));
    }
    Ok(())
}
