// SPDX-License-Identifier: CC0-1.0

use super::*;

/// A stored DB CLUSTER snapshot: the cluster's settings as they stood when the snapshot
/// was taken, which is why they are copied rather than re-read later.
///
/// `CreateDBClusterSnapshot` writes one (a snapshot OF a cluster, through
/// [`DbClusterSnapshotRecord::from_cluster`]), `CopyDBClusterSnapshot` writes one (a copy
/// of another snapshot) and `DeleteDBClusterSnapshot` removes one; the final snapshot
/// `DeleteDBCluster` takes is still scaffolded in this crate and is its own stub issue, so
/// it writes none yet. The reading operations are implemented against these records rather
/// than against a canned answer, so they report exactly what is stored.
#[derive(Debug, Clone)]
pub struct DbClusterSnapshotRecord {
    /// `DBClusterSnapshotIdentifier`, lowercased, and this record's key within its
    /// region: the member documents "This value is stored as a lowercase string".
    pub snapshot_identifier: String,
    /// The region the snapshot lives in; the scope of its ARN.
    pub region: String,
    /// `DBClusterIdentifier` of the cluster the snapshot was taken from.
    pub db_cluster_identifier: String,
    /// `DbClusterResourceId`, which `DescribeDBClusterSnapshots` can select on.
    pub db_cluster_resource_id: String,
    pub engine: String,
    pub engine_version: Option<String>,
    pub engine_mode: Option<String>,
    /// `AllocatedStorage` of the cluster the snapshot was taken from, which the snapshot
    /// reports until its own size is known; see [`Self::to_wire`].
    pub allocated_storage: Option<i32>,
    /// Whether the cluster the snapshot was taken from had a DB INSTANCE in it, i.e.
    /// whether the snapshot holds any data at all.
    pub holds_data: bool,
    pub master_username: Option<String>,
    pub availability_zones: Vec<String>,
    pub port: i32,
    pub storage_type: Option<String>,
    pub storage_encrypted: bool,
    pub kms_key_id: Option<String>,
    pub license_model: Option<String>,
    /// `VpcId`: the VPC of the cluster the snapshot was taken from.
    pub vpc_id: String,
    /// `BackupRetentionPeriod`, as the source cluster retained automated backups when the
    /// snapshot was taken.
    pub backup_retention_period: Option<i32>,
    /// `PreferredBackupWindow`, as the source cluster carried it when the snapshot was
    /// taken.
    pub preferred_backup_window: Option<String>,
    /// `IAMDatabaseAuthenticationEnabled`, as the source cluster carried it.
    pub iam_database_authentication_enabled: bool,
    /// `SnapshotType`: `manual` for every snapshot a caller asked for.
    pub snapshot_type: String,
    /// `Status`: [`SNAPSHOT_STATUS_COPYING`] while a copy is being made,
    /// [`SNAPSHOT_STATUS_AVAILABLE`] once the clock has made it due.
    pub status: String,
    /// The status this record owes, and when; see [`PendingStatus`].
    pub pending_status: Option<PendingStatus>,
    pub cluster_create_time: ::aws_smithy_types::DateTime,
    pub snapshot_create_time: ::aws_smithy_types::DateTime,
    /// `SourceDBClusterSnapshotArn`: the snapshot this one was copied from, absent for a
    /// snapshot taken from a cluster.
    pub source_snapshot_arn: Option<String>,
    /// The accounts the `restore` attribute authorizes, which
    /// `ModifyDBClusterSnapshotAttribute` adds to and removes from and
    /// `DescribeDBClusterSnapshotAttributes` reports; see [`SNAPSHOT_RESTORE_ATTRIBUTE`].
    /// A set, so authorizing an account twice changes nothing and the report is ordered.
    pub restore_attribute: BTreeSet<String>,
    pub tags: Vec<Tag>,
}

impl DbClusterSnapshotRecord {
    /// Take a snapshot OF `cluster`: its settings as they stand now, copied onto the
    /// snapshot so a later change to the cluster cannot rewrite what the snapshot holds.
    ///
    /// `SnapshotType` is `manual` — the class AWS gives every snapshot a caller asked for,
    /// `automated` being reserved for the backups the service schedules itself — and the
    /// snapshot answers [`SNAPSHOT_STATUS_CREATING`] with the clock owing it
    /// [`SNAPSHOT_STATUS_AVAILABLE`], so a caller that reads it straight back sees the
    /// snapshot still being taken. `source_snapshot_arn` is absent because nothing was
    /// copied: that member names "the DB cluster snapshot that was copied to create this
    /// one".
    pub fn from_cluster(
        cluster: &DbClusterRecord,
        cluster_has_instances: bool,
        snapshot_identifier: &str,
        tags: Vec<Tag>,
    ) -> Self {
        Self {
            snapshot_identifier: snapshot_identifier.to_string(),
            region: cluster.region.clone(),
            db_cluster_identifier: cluster.identifier.clone(),
            db_cluster_resource_id: cluster.resource_id.clone(),
            engine: cluster.engine.clone(),
            engine_version: cluster.engine_version.clone(),
            engine_mode: Some(cluster.engine_mode.clone()),
            // "The allocated storage size of the DB cluster snapshot in gibibytes (GiB)":
            // the storage of the cluster the snapshot is being taken from, which is what
            // the API reports until the snapshot's own size is known ([`Self::to_wire`]).
            allocated_storage: cluster.reported_allocated_storage(),
            holds_data: cluster_has_instances,
            master_username: cluster.master_username.clone(),
            availability_zones: cluster.availability_zones.clone(),
            // "The port that the DB cluster was listening on at the time of the snapshot":
            // a fact about the cluster's DB INSTANCES. A cluster with none had nothing
            // listening, so the snapshot was taken on no port at all.
            port: if cluster_has_instances { cluster.port } else { 0 },
            storage_type: cluster.storage_type.clone(),
            storage_encrypted: cluster.storage_encrypted,
            kms_key_id: cluster.kms_key_id.clone(),
            // The license the engine the snapshot was taken from runs under; see
            // [`default_license_model`] for where each engine's comes from.
            license_model: default_license_model(&cluster.engine).map(str::to_string),
            // The cluster settings the snapshot reports members of its own for, copied here
            // with the rest so a later change to the cluster cannot rewrite what the
            // snapshot reports.
            vpc_id: cluster.vpc_id(),
            backup_retention_period: Some(cluster.backup_retention_period),
            preferred_backup_window: Some(cluster.preferred_backup_window.clone()),
            iam_database_authentication_enabled: cluster.iam_database_authentication_enabled,
            snapshot_type: SNAPSHOT_TYPE_MANUAL.to_string(),
            status: SNAPSHOT_STATUS_CREATING.to_string(),
            pending_status: Some(PendingStatus::in_a_moment(SNAPSHOT_STATUS_AVAILABLE)),
            cluster_create_time: cluster.cluster_create_time,
            snapshot_create_time: now_as_wire_timestamp(),
            source_snapshot_arn: None,
            restore_attribute: BTreeSet::new(),
            tags,
        }
    }

    /// Apply a change whose due instant has passed. Returns whether the record is now
    /// gone, exactly as [`DbInstanceRecord::settle`] does.
    pub(super) fn settle(&mut self, now: Instant) -> bool {
        let due = self
            .pending_status
            .as_ref()
            .is_some_and(|pending| now >= pending.due_at);
        if !due {
            return false;
        }
        match self.pending_status.take().map(|pending| pending.change) {
            Some(PendingChange::Status(status)) => self.status = status,
            Some(PendingChange::Removal) => return true,
            // Unreachable: the change was there a line ago, under this one `&mut self`.
            // Answered as "nothing was due" rather than with a panic in a request path.
            None => {}
        }
        false
    }

    /// `arn:aws:rds:<region>:<account>:cluster-snapshot:<identifier>`.
    pub fn arn(&self) -> String {
        emulator_server::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("cluster-snapshot:{}", self.snapshot_identifier),
        )
    }

    /// Whether this snapshot is public — its `restore` attribute authorizes every
    /// account; see [`SNAPSHOT_ATTRIBUTE_ALL`].
    pub fn is_public(&self) -> bool {
        self.restore_attribute.contains(SNAPSHOT_ATTRIBUTE_ALL)
    }

    /// Project the stored record onto the modelled `DBClusterSnapshot` output shape.
    pub fn to_wire(&self) -> DBClusterSnapshot {
        DBClusterSnapshot {
            availability_zones: Some(self.availability_zones.clone()),
            db_cluster_snapshot_identifier: Some(self.snapshot_identifier.clone()),
            db_cluster_identifier: Some(self.db_cluster_identifier.clone()),
            snapshot_create_time: Some(self.snapshot_create_time),
            engine: Some(self.engine.clone()),
            engine_mode: self.engine_mode.clone(),
            // The storage the snapshot holds. That size is only known once the snapshot has
            // been taken: while it is still being made the API reports the allocated
            // storage of the CLUSTER it is being taken from, and a finished snapshot of a
            // cluster that had no DB instance in it holds no data at all.
            allocated_storage: if self.status == SNAPSHOT_STATUS_AVAILABLE {
                Some(if self.holds_data {
                    self.allocated_storage.unwrap_or(0)
                } else {
                    0
                })
            } else {
                self.allocated_storage
            },
            status: Some(self.status.clone()),
            port: Some(self.port),
            cluster_create_time: Some(self.cluster_create_time),
            master_username: self.master_username.clone(),
            engine_version: self.engine_version.clone(),
            license_model: self.license_model.clone(),
            snapshot_type: Some(self.snapshot_type.clone()),
            // The progress of the copy this record stands for: complete once the
            // snapshot is available, and not yet begun while it is still being made.
            percent_progress: Some(if self.status == SNAPSHOT_STATUS_AVAILABLE {
                100
            } else {
                0
            }),
            storage_encrypted: Some(self.storage_encrypted),
            kms_key_id: self.kms_key_id.clone(),
            db_cluster_snapshot_arn: Some(self.arn()),
            source_db_cluster_snapshot_arn: self.source_snapshot_arn.clone(),
            tag_list: Some(self.tags.clone()),
            storage_type: self.storage_type.clone(),
            db_cluster_resource_id: Some(self.db_cluster_resource_id.clone()),
            vpc_id: Some(self.vpc_id.clone()),
            backup_retention_period: self.backup_retention_period,
            preferred_backup_window: self.preferred_backup_window.clone(),
            iam_database_authentication_enabled: Some(self.iam_database_authentication_enabled),
            ..DBClusterSnapshot::default()
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
    pub(super) fn settle(&mut self, now: Instant) -> bool {
        let due = self
            .pending_status
            .as_ref()
            .is_some_and(|pending| now >= pending.due_at);
        if !due {
            return false;
        }
        match self.pending_status.take().map(|pending| pending.change) {
            Some(PendingChange::Status(status)) => self.status = status,
            Some(PendingChange::Removal) => return true,
            // Unreachable: the change was there a line ago, under this one `&mut self`.
            // Answered as "nothing was due" rather than with a panic in a request path.
            None => {}
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
            status: SNAPSHOT_STATUS_CREATING.to_string(),
            pending_status: Some(PendingStatus::in_a_moment(SNAPSHOT_STATUS_AVAILABLE)),
            snapshot_type: SNAPSHOT_TYPE_MANUAL.to_string(),
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
