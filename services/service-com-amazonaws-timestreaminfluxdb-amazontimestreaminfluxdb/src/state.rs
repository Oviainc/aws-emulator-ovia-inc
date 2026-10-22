// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb service.
//!
//! This module owns the persisted DB-instance records plus the small helpers every
//! DB-instance handler shares: minting the service-generated `id`, formatting the `arn`
//! and `endpoint`, settling the clock-owed status transition, and projecting a stored
//! record into each operation's own output shape. The handlers in `src/ops/` hold no
//! state of their own — they read and write the records here.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    DbBackupConfiguration, DbBackupConfigurationOutput, DbInstanceSummary,
    LogDeliveryConfiguration, MaintenanceSchedule, Parameters,
};

/// `Status` values this service's records use, each one a member of
/// `crate::ops::types::STATUS_VALUES` (the modelled `Status` enum).
pub const STATUS_CREATING: &str = "CREATING";
/// See [`STATUS_CREATING`].
pub const STATUS_AVAILABLE: &str = "AVAILABLE";
/// See [`STATUS_CREATING`].
pub const STATUS_DELETING: &str = "DELETING";
/// See [`STATUS_CREATING`].
pub const STATUS_UPDATING: &str = "UPDATING";
/// See [`STATUS_CREATING`].
pub const STATUS_UPDATING_INSTANCE_TYPE: &str = "UPDATING_INSTANCE_TYPE";
/// See [`STATUS_CREATING`].
pub const STATUS_UPDATING_DEPLOYMENT_TYPE: &str = "UPDATING_DEPLOYMENT_TYPE";

/// The `resourceType` this service reports on `ConflictException` /
/// `ResourceNotFoundException` for a DB instance — a member of
/// `crate::ops::types::RESOURCE_TYPE_VALUES` (the modelled `ResourceType` enum).
pub const RESOURCE_TYPE_DB_INSTANCE: &str = "DB_INSTANCE";

/// The `resourceType` this service reports on `ConflictException` /
/// `ResourceNotFoundException` for a DB parameter group.
///
/// Those exceptions model `resourceType` as a plain `String`, NOT as the `ResourceType`
/// enum (`smithy_query(query="shape", shape="ConflictException,ResourceType")`), so the
/// parameter-group resource type is reportable even though the enum lists only the two
/// compute resources.
pub const RESOURCE_TYPE_DB_PARAMETER_GROUP: &str = "DB_PARAMETER_GROUP";

/// The `deploymentType` that places a standby in a second Availability Zone, from the
/// modelled `DeploymentType` enum (`crate::ops::types::DEPLOYMENT_TYPE_VALUES`).
pub const DEPLOYMENT_TYPE_WITH_MULTIAZ_STANDBY: &str = "WITH_MULTIAZ_STANDBY";

/// The region a request's ARNs are scoped to when its SigV4 credential scope names none.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's `aws.api#service$arnNamespace` / sigv4 signing name, used for both the
/// ARN service segment and the endpoint suffix.
pub const ARN_NAMESPACE: &str = "timestream-influxdb";

/// How long a DB instance stays in a transient status before the clock settles it to
/// `AVAILABLE`.
///
/// NOT an AWS-documented duration (a real instance takes minutes): a chosen span, inside
/// the 250–750 ms band this project requires, so a caller that reads straight back still
/// observes the transient status while a test does not have to wait on a real provision.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// The default `maxResults` for the paginated list operations: the maximum the model's
/// `MaxResults` range (1..100) allows, so an unbounded request pages at the modelled cap.
pub const MAX_RESULTS_DEFAULT: i32 = 100;

/// How many DB instances the store holds before `CreateDbInstance` refuses with the
/// modelled `ServiceQuotaExceededException`.
///
/// Amazon Timestream for InfluxDB publishes an adjustable quota on DB instances per
/// account per Region, and this emulator models neither Service Quotas nor an account's
/// adjustments — so this is the emulator's OWN bound on the store rather than a claim
/// about any account's limit. It is set far above any workflow an emulator serves, and it
/// is what stops the state growing without end: every record it holds is released by
/// `DeleteDbInstance`, which also forgets that instance's name, so the name index can
/// never outgrow this bound either. It also caps the per-request work `ListDbInstances`
/// does when it settles the clock-owed statuses.
pub const DB_INSTANCE_QUOTA: usize = 1_000;

/// How many DB parameter groups the store holds before `CreateDbParameterGroup` refuses
/// with the modelled `ServiceQuotaExceededException`.
///
/// Like [`DB_INSTANCE_QUOTA`], this is the emulator's OWN bound on the state rather than
/// a claim about any account's adjustable Service Quotas limit: it is what stops the
/// store (and the parameter-group name index beside it) growing without end, and it sits
/// far above any workflow an emulator serves.
pub const DB_PARAMETER_GROUP_QUOTA: usize = 1_000;

/// One persisted Timestream for InfluxDB DB instance: the single source of truth every
/// DB-instance operation reads and writes.
#[derive(Debug, Clone)]
pub struct DbInstance {
    pub id: String,
    pub name: String,
    pub arn: String,
    /// The region this instance was created in; its ARN, endpoint and Availability
    /// Zones are all scoped to it, so a later operation derives them from here rather
    /// than from the calling request.
    pub region: String,
    pub status: String,
    pub endpoint: String,
    pub port: i32,
    pub network_type: Option<String>,
    pub db_instance_type: String,
    pub db_storage_type: Option<String>,
    pub allocated_storage: i32,
    pub deployment_type: Option<String>,
    pub vpc_subnet_ids: Vec<String>,
    pub vpc_security_group_ids: Vec<String>,
    pub publicly_accessible: Option<bool>,
    pub db_parameter_group_identifier: Option<String>,
    pub availability_zone: Option<String>,
    pub secondary_availability_zone: Option<String>,
    pub log_delivery_configuration: Option<LogDeliveryConfiguration>,
    pub maintenance_schedule: Option<MaintenanceSchedule>,
    pub db_backup_configurations: Option<Vec<DbBackupConfiguration>>,
    pub influx_auth_parameters_secret_arn: String,
    pub kms_key_id: Option<String>,
    /// Stored because `CreateDbInstance` accepts them for the initial InfluxDB admin
    /// user; they are not members of any DB-instance output shape, so nothing echoes
    /// them back. The `password` is deliberately NOT kept: it is `@sensitive`, no output
    /// shape carries it, and the record only needs the secret ARN that stands for it.
    pub username: Option<String>,
    pub organization: Option<String>,
    pub bucket: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    /// When the clock owes this record its settle to `AVAILABLE`; `None` once settled.
    pub settles_at: Option<SystemTime>,
}

impl DbInstance {
    /// Materialise the status change the clock already made due.
    ///
    /// A reporting operation calls this before answering: the transition was owed at
    /// `settles_at` whether or not anybody called, so applying it on read is not a
    /// read-time mutation of the resource's own lifecycle.
    pub fn settle(&mut self, now: SystemTime) {
        if self.settles_at.is_some_and(|due| now >= due) {
            self.status = STATUS_AVAILABLE.to_string();
            self.settles_at = None;
        }
    }

    /// Put the record into `status` and make the settle back to `AVAILABLE` due one
    /// [`TRANSITION_SPAN`] from `now`.
    pub fn begin_transition(&mut self, status: &str, now: SystemTime) {
        self.status = status.to_string();
        self.settles_at = Some(now + TRANSITION_SPAN);
    }

    /// The list-shaped projection of this record.
    pub fn to_summary(&self) -> DbInstanceSummary {
        DbInstanceSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            arn: self.arn.clone(),
            status: Some(self.status.clone()),
            endpoint: Some(self.endpoint.clone()),
            port: Some(self.port),
            network_type: self.network_type.clone(),
            db_instance_type: Some(self.db_instance_type.clone()),
            db_storage_type: self.db_storage_type.clone(),
            allocated_storage: Some(self.allocated_storage),
            deployment_type: self.deployment_type.clone(),
        }
    }

    /// The backup configurations as the output list shape.
    ///
    /// `nextAutomatedBackupTime` stays absent: scheduling automated backups belongs to
    /// the backup operations, which are not implemented, so this emulator reports no
    /// time for a backup nothing has scheduled.
    fn backup_configurations_output(&self) -> Option<Vec<DbBackupConfigurationOutput>> {
        self.db_backup_configurations.as_ref().map(|configs| {
            configs
                .iter()
                .map(|config| DbBackupConfigurationOutput {
                    r#type: config.r#type.clone(),
                    retention_days: config.retention_days,
                    enabled: config.enabled,
                    custom_schedule: config.custom_schedule.clone(),
                    next_automated_backup_time: None,
                })
                .collect()
        })
    }
}

/// The four DB-instance operations whose output shapes are the same full record.
///
/// They are four distinct generated types with identical members, so one macro writes
/// one projection per shape instead of four copies that could drift apart.
macro_rules! db_instance_record_projections {
    ($($method:ident -> $out:path),* $(,)?) => {
        impl DbInstance {
            $(
                /// Project this stored record into the operation's output shape. Every
                /// member comes from the record; nothing is recomputed from the request.
                pub fn $method(&self) -> $out {
                    $out {
                        id: self.id.clone(),
                        name: self.name.clone(),
                        arn: self.arn.clone(),
                        status: Some(self.status.clone()),
                        endpoint: Some(self.endpoint.clone()),
                        port: Some(self.port),
                        network_type: self.network_type.clone(),
                        db_instance_type: Some(self.db_instance_type.clone()),
                        db_storage_type: self.db_storage_type.clone(),
                        allocated_storage: Some(self.allocated_storage),
                        deployment_type: self.deployment_type.clone(),
                        vpc_subnet_ids: self.vpc_subnet_ids.clone(),
                        publicly_accessible: self.publicly_accessible,
                        vpc_security_group_ids: Some(self.vpc_security_group_ids.clone()),
                        db_parameter_group_identifier: self
                            .db_parameter_group_identifier
                            .clone(),
                        availability_zone: self.availability_zone.clone(),
                        secondary_availability_zone: self.secondary_availability_zone.clone(),
                        log_delivery_configuration: self.log_delivery_configuration.clone(),
                        influx_auth_parameters_secret_arn: Some(
                            self.influx_auth_parameters_secret_arn.clone(),
                        ),
                        // A standalone DB instance belongs to no cluster, so it has no
                        // cluster id and no in-cluster role to report.
                        db_cluster_id: None,
                        instance_mode: None,
                        instance_modes: None,
                        maintenance_schedule: self.maintenance_schedule.clone(),
                        // No maintenance has run and none is scheduled by this slice.
                        last_maintenance_time: None,
                        next_maintenance_time: None,
                        db_backup_configurations: self.backup_configurations_output(),
                        kms_key_id: self.kms_key_id.clone(),
                    }
                }
            )*
        }
    };
}

db_instance_record_projections! {
    to_create_output -> crate::ops::types::CreateDbInstanceOutput,
    to_get_output -> crate::ops::types::GetDbInstanceOutput,
    to_update_output -> crate::ops::types::UpdateDbInstanceOutput,
    to_delete_output -> crate::ops::types::DeleteDbInstanceOutput,
}

/// One persisted Timestream for InfluxDB DB parameter group: the single source of truth
/// every DB-parameter-group operation reads and writes.
///
/// A parameter group has no lifecycle status in its model (no `status` member on any of
/// its shapes), so it is ready the moment it is created and owes the clock no transition.
#[derive(Debug, Clone)]
pub struct DbParameterGroup {
    pub id: String,
    pub name: String,
    pub arn: String,
    /// The region the group was created in; its ARN is scoped to it, and the name
    /// uniqueness rule is per region, so later operations read the region from here.
    pub region: String,
    pub description: Option<String>,
    pub parameters: Option<Parameters>,
    /// The tags the create request attached. No parameter-group output shape carries
    /// them, so nothing echoes them back; they are kept for the tagging operations.
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
}

/// The DB-parameter-group operations whose output shape is the same full record.
///
/// They are distinct generated types with identical members, so one macro writes one
/// projection per shape instead of copies that could drift apart.
macro_rules! db_parameter_group_record_projections {
    ($($method:ident -> $out:path),* $(,)?) => {
        impl DbParameterGroup {
            $(
                /// Project this stored record into the operation's output shape. Every
                /// member comes from the record; nothing is recomputed from the request.
                pub fn $method(&self) -> $out {
                    $out {
                        id: self.id.clone(),
                        name: self.name.clone(),
                        arn: self.arn.clone(),
                        description: self.description.clone(),
                        parameters: self.parameters.clone(),
                    }
                }
            )*
        }
    };
}

db_parameter_group_record_projections! {
    to_create_output -> crate::ops::types::CreateDbParameterGroupOutput,
    to_get_output -> crate::ops::types::GetDbParameterGroupOutput,
}

/// Per-service state for com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsTimestreaminfluxdbAmazontimestreaminfluxdbState {
    /// Live DB instances by their service-generated `id`. Ordered, so `ListDbInstances`
    /// pages deterministically.
    pub db_instances: BTreeMap<String, DbInstance>,
    /// `(region, name)` -> `id`: the uniqueness index behind `ConflictException` on a
    /// duplicate name. The key carries the region because the rule the model documents is
    /// "DB instance names must be unique per customer and per region", so the same name
    /// in two regions is two instances.
    pub db_instance_names: BTreeMap<(String, String), String>,
    /// Monotonic sequence mixed into every minted `id`, so two instances created with
    /// the same name in one process never collide.
    pub db_instance_seq: u64,
    /// Live DB parameter groups by their service-generated `id`. Ordered, so
    /// `ListDbParameterGroups` pages deterministically.
    pub db_parameter_groups: BTreeMap<String, DbParameterGroup>,
    /// `(region, name)` -> `id`: the uniqueness index behind `ConflictException` on a
    /// duplicate parameter-group name. The key carries the region because the rule the
    /// model documents is "The name must be unique per customer and per region".
    pub db_parameter_group_names: BTreeMap<(String, String), String>,
    /// Monotonic sequence mixed into every minted parameter-group `id`.
    pub db_parameter_group_seq: u64,
}

impl ComAmazonawsTimestreaminfluxdbAmazontimestreaminfluxdbState {
    /// Mint the next service-generated DB instance id: 10 lowercase hex characters, which
    /// satisfies the modelled `DbInstanceId` pattern `^[a-zA-Z0-9]+$` and length 3..64.
    ///
    /// Opaque to the caller but deterministic for a given (name, region, sequence), and
    /// unique because the monotonic sequence advances until the id is unused.
    pub fn mint_db_instance_id(&mut self, name: &str, region: &str) -> String {
        loop {
            self.db_instance_seq += 1;
            let seq = self.db_instance_seq;
            let digest = <sha2::Sha256 as sha2::Digest>::digest(
                format!("db-instance|{region}|{name}|{seq}").as_bytes(),
            );
            let id = hex::encode(digest)[..10].to_string();
            if !self.db_instances.contains_key(&id) {
                return id;
            }
        }
    }

    /// The id of the live DB instance holding `name` in `region`, if any.
    pub fn db_instance_id_for_name(&self, region: &str, name: &str) -> Option<&String> {
        self.db_instance_names
            .get(&(region.to_string(), name.to_string()))
    }

    /// Claim `name` in `region` for `id`.
    pub fn claim_db_instance_name(&mut self, region: &str, name: &str, id: String) {
        self.db_instance_names
            .insert((region.to_string(), name.to_string()), id);
    }

    /// Release the name a deleted record held, so the name can be reused.
    pub fn release_db_instance_name(&mut self, region: &str, name: &str) {
        self.db_instance_names
            .remove(&(region.to_string(), name.to_string()));
    }

    /// Mint the next service-generated DB parameter group id: 10 lowercase hex
    /// characters, which satisfies the modelled `DbParameterGroupId` pattern
    /// `^[a-zA-Z0-9]+$` and length 3..64.
    ///
    /// Opaque to the caller but deterministic for a given (name, region, sequence), and
    /// unique because the monotonic sequence advances until the id is unused.
    pub fn mint_db_parameter_group_id(&mut self, name: &str, region: &str) -> String {
        loop {
            self.db_parameter_group_seq += 1;
            let seq = self.db_parameter_group_seq;
            let digest = <sha2::Sha256 as sha2::Digest>::digest(
                format!("db-parameter-group|{region}|{name}|{seq}").as_bytes(),
            );
            let id = hex::encode(digest)[..10].to_string();
            if !self.db_parameter_groups.contains_key(&id) {
                return id;
            }
        }
    }

    /// The id of the DB parameter group holding `name` in `region`, if any.
    pub fn db_parameter_group_id_for_name(&self, region: &str, name: &str) -> Option<&String> {
        self.db_parameter_group_names
            .get(&(region.to_string(), name.to_string()))
    }

    /// Claim `name` in `region` for `id`.
    pub fn claim_db_parameter_group_name(&mut self, region: &str, name: &str, id: String) {
        self.db_parameter_group_names
            .insert((region.to_string(), name.to_string()), id);
    }
}

/// The region a request is scoped to: the region of its SigV4 credential scope, else
/// [`DEFAULT_REGION`].
///
/// Constrained to the characters the modelled `Arn` pattern allows in its region segment
/// (`[a-z0-9\-]+`), so an ARN built from it is always a well-formed ARN.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of a DB instance: `arn:aws:timestream-influxdb:<region>:<account>:db-instance/<id>`.
pub fn db_instance_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("db-instance/{id}"))
}

/// The ARN of a DB parameter group:
/// `arn:aws:timestream-influxdb:<region>:<account>:db-parameter-group/<id>`, one of the
/// four resource segments the modelled `Arn` pattern allows.
pub fn db_parameter_group_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("db-parameter-group/{id}"))
}

/// The endpoint a client connects to InfluxDB on.
///
/// The DB instance name is the prefix, as `CreateDbInstance`'s `name` documents ("This
/// name will also be a prefix included in the endpoint"), followed by the instance id so
/// two instances that reused a name across regions never share an endpoint.
pub fn db_instance_endpoint(name: &str, id: &str, region: &str) -> String {
    format!("{name}-{id}.{ARN_NAMESPACE}.{region}.on.aws")
}

/// The ARN of the Secrets Manager secret holding the initial InfluxDB authorization
/// parameters for a DB instance, as `influxAuthParametersSecretArn` documents.
pub fn influx_auth_parameters_secret_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        "secretsmanager",
        region,
        &format!("secret:{ARN_NAMESPACE}-{id}"),
    )
}

/// The Availability Zone a DB instance resides in, and the standby's zone.
///
/// A second zone is reported only for the deployment type that places a standby in one,
/// as `secondaryAvailabilityZone` documents ("when deploying with a MultiAZ standby
/// instance").
pub fn availability_zones(region: &str, deployment_type: Option<&str>) -> (String, Option<String>) {
    let primary = format!("{region}a");
    let secondary = (deployment_type == Some(DEPLOYMENT_TYPE_WITH_MULTIAZ_STANDBY))
        .then(|| format!("{region}b"));
    (primary, secondary)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsTimestreaminfluxdbAmazontimestreaminfluxdbState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsTimestreaminfluxdbAmazontimestreaminfluxdbState>)
}
