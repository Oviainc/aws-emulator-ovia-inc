// SPDX-License-Identifier: CC0-1.0

use super::*;

/// One snapshot, as this emulator persists it: the source cluster's
/// configuration as it stood when the snapshot was taken, plus the snapshot's
/// own identity and status.
#[derive(Debug, Clone)]
pub struct SnapshotRecord {
    /// The snapshot's identity, lowercased — `CreateSnapshot$SnapshotName` "is
    /// stored as a lowercase string".
    pub snapshot_name: String,
    pub cache_cluster_id: Option<String>,
    /// The source replication group. Always absent in this slice: every
    /// replication-group operation is a scaffolded 501, so no group can be
    /// snapshotted; it is what `DescribeSnapshots$ReplicationGroupId` filters
    /// on, so that filter reads stored state rather than a constant.
    pub replication_group_id: Option<String>,
    /// One of `Snapshot$SnapshotStatus`'s values (`creating | available |
    /// restoring | copying | deleting`), which spell the first two exactly as
    /// [`STATUS_CREATING`] and [`STATUS_AVAILABLE`] do.
    pub status: String,
    /// The status change the clock owes this snapshot, if any.
    pub pending_status: Option<PendingTransition>,
    /// When the clock owes this snapshot's REMOVAL, set by `DeleteSnapshot`:
    /// "ElastiCache immediately begins deleting the snapshot", so the record
    /// answers [`STATUS_DELETING`] from that call and is dropped once the span
    /// has passed. `None` on a snapshot nobody has deleted.
    pub pending_removal: Option<SystemTime>,
    /// [`SNAPSHOT_SOURCE_MANUAL`] or [`SNAPSHOT_SOURCE_AUTOMATED`].
    pub source: String,
    /// When the snapshot was taken. `Snapshot` has no member for it — each
    /// `NodeSnapshot$SnapshotCreateTime` reports it — so it is kept once here
    /// and copied onto the node snapshots.
    pub created_at: SystemTime,
    pub cache_node_type: Option<String>,
    pub engine: String,
    pub engine_version: String,
    pub num_cache_nodes: i32,
    pub preferred_availability_zone: Option<String>,
    pub cache_cluster_create_time: SystemTime,
    pub preferred_maintenance_window: String,
    pub topic_arn: Option<String>,
    pub port: i32,
    pub cache_parameter_group_name: String,
    pub cache_subnet_group_name: Option<String>,
    pub vpc_id: Option<String>,
    pub auto_minor_version_upgrade: Option<bool>,
    /// The source cluster's `SnapshotRetentionLimit` when the snapshot was
    /// taken: "For manual snapshots, this field reflects the
    /// SnapshotRetentionLimit for the source cluster when the snapshot was
    /// created. This field is otherwise ignored: Manual snapshots do not
    /// expire."
    pub snapshot_retention_limit: Option<i32>,
    pub snapshot_window: Option<String>,
    pub kms_key_id: Option<String>,
    pub arn: String,
    pub node_snapshots: Vec<NodeSnapshotRecord>,
    /// The tags `CreateSnapshot` was given. The ONE copy of this snapshot's tags:
    /// `ListTagsForResource` reports it and `AddTagsToResource` /
    /// `RemoveTagsFromResource` change it, through
    /// [`ComAmazonawsElasticacheAmazonelasticachev9State::resource_tags`].
    pub tags: Vec<(String, String)>,
}

impl SnapshotRecord {
    /// A snapshot of `cluster` as it stands now.
    ///
    /// Every reported value is COPIED out of the stored cluster record, which is
    /// what makes the snapshot "a copy of an entire cluster as of the time when
    /// the snapshot was taken" rather than a second view of the live cluster.
    pub fn from_cluster(cluster: &CacheClusterRecord, new: NewSnapshot) -> Self {
        SnapshotRecord {
            arn: snapshot_arn(&cluster.region, &new.snapshot_name),
            node_snapshots: cluster
                .nodes
                .iter()
                .map(|node| NodeSnapshotRecord {
                    cache_cluster_id: cluster.cache_cluster_id.clone(),
                    cache_node_id: node.cache_node_id.clone(),
                    node_group_id: None,
                    node_group_configuration: None,
                    cache_node_create_time: node.create_time,
                    snapshot_create_time: new.now,
                })
                .collect(),
            snapshot_name: new.snapshot_name,
            cache_cluster_id: Some(cluster.cache_cluster_id.clone()),
            replication_group_id: cluster.replication_group_id.clone(),
            status: STATUS_CREATING.to_string(),
            pending_status: Some(PendingTransition {
                due: new.now + SNAPSHOT_TRANSITION,
                status: STATUS_AVAILABLE.to_string(),
            }),
            pending_removal: None,
            source: new.source,
            created_at: new.now,
            cache_node_type: cluster.cache_node_type.clone(),
            engine: cluster.engine.clone(),
            engine_version: cluster.engine_version.clone(),
            num_cache_nodes: cluster.num_cache_nodes,
            preferred_availability_zone: cluster.preferred_availability_zone(),
            cache_cluster_create_time: cluster.created_at,
            preferred_maintenance_window: cluster.preferred_maintenance_window.clone(),
            topic_arn: cluster.notification_topic_arn.clone(),
            port: cluster.endpoint_port,
            cache_parameter_group_name: cluster.cache_parameter_group_name.clone(),
            cache_subnet_group_name: cluster.cache_subnet_group_name.clone(),
            vpc_id: new.vpc_id,
            auto_minor_version_upgrade: cluster.auto_minor_version_upgrade,
            snapshot_retention_limit: cluster.snapshot_retention_limit,
            snapshot_window: cluster.snapshot_window.clone(),
            kms_key_id: new.kms_key_id,
            tags: new.tags,
        }
    }

    /// A copy of this snapshot stored under another name.
    ///
    /// A copy is "a copy of an existing snapshot", so everything that describes
    /// WHAT was captured — the source cluster's id and configuration, the
    /// instant the data was taken at, and the per-node snapshots — is carried
    /// over unchanged rather than re-read from the live cluster or re-stamped
    /// with the copy's own clock: copying does not retake the snapshot. Only the
    /// copy's own identity (name and ARN), its fresh status, its provenance and
    /// the members the copy request itself carries (KMS key, tags) differ.
    ///
    /// A copy is always a MANUAL snapshot, whatever the source's
    /// `SnapshotSource` was: copying an automated backup is how the ElastiCache
    /// User Guide has a caller keep it past its retention limit, and a manual
    /// snapshot is the one that "does not expire".
    pub fn copy_to(&self, new: SnapshotCopy) -> SnapshotRecord {
        SnapshotRecord {
            arn: snapshot_arn(&new.region, &new.target_snapshot_name),
            snapshot_name: new.target_snapshot_name,
            status: STATUS_CREATING.to_string(),
            pending_status: Some(PendingTransition {
                due: new.now + SNAPSHOT_TRANSITION,
                status: STATUS_AVAILABLE.to_string(),
            }),
            // The copy is its own resource: whatever the source's deletion
            // owes, nothing is owed on the record stored under the new name.
            pending_removal: None,
            source: SNAPSHOT_SOURCE_MANUAL.to_string(),
            // "The ID of the KMS key used to encrypt the target snapshot": the
            // copy keeps the source's key when the request names none.
            kms_key_id: new.kms_key_id.or_else(|| self.kms_key_id.clone()),
            // `Tags` are "a list of tags to be added to this resource" — the
            // copy's own, not the source's.
            tags: new.tags,
            ..self.clone()
        }
    }

    /// Materialise the status change the clock owes this snapshot, if `now` is
    /// past its due instant, and hand back the change it made. The one write a
    /// reporting operation may make; see [`CacheClusterRecord::settle`].
    pub fn settle(&mut self, now: SystemTime) -> Option<SettledTransition> {
        let pending = self.pending_status.as_ref()?;
        if now < pending.due {
            return None;
        }
        let settled = SettledTransition {
            due: pending.due,
            from: self.status.clone(),
            to: pending.status.clone(),
        };
        self.status = pending.status.clone();
        self.pending_status = None;
        Some(settled)
    }

    /// Put this snapshot into [`STATUS_DELETING`] and let the clock owe its
    /// removal [`SNAPSHOT_TRANSITION`] later.
    ///
    /// `DeleteSnapshot` documents exactly this: "When you receive a successful
    /// response from this operation, ElastiCache immediately begins deleting the
    /// snapshot; you cannot cancel or revert this operation" — so the record is
    /// still there, reporting `deleting`, and the removal is what the clock
    /// materialises ([`ComAmazonawsElasticacheAmazonelasticachev9State::purge_deleted_snapshots`]).
    /// Any status change that was still pending is dropped: a snapshot that is
    /// going away is not owed `available` any more.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        self.status = STATUS_DELETING.to_string();
        self.pending_status = None;
        self.pending_removal = Some(now + SNAPSHOT_TRANSITION);
    }

    /// This record as the modelled `Snapshot`.
    pub fn to_modeled(&self, include_node_group_config: bool) -> Snapshot {
        Snapshot {
            snapshot_name: Some(self.snapshot_name.clone()),
            replication_group_id: self.replication_group_id.clone(),
            // The description of the source replication group: a snapshot of a
            // cache cluster has no source group to take one from.
            replication_group_description: None,
            cache_cluster_id: self.cache_cluster_id.clone(),
            snapshot_status: Some(self.status.clone()),
            snapshot_source: Some(self.source.clone()),
            cache_node_type: self.cache_node_type.clone(),
            engine: Some(self.engine.clone()),
            engine_version: Some(self.engine_version.clone()),
            num_cache_nodes: Some(self.num_cache_nodes),
            preferred_availability_zone: self.preferred_availability_zone.clone(),
            // Outposts: `CreateCacheCluster` refuses every Outpost member, so
            // no cluster here was placed in one.
            preferred_outpost_arn: None,
            cache_cluster_create_time: Some(::aws_smithy_types::DateTime::from(
                self.cache_cluster_create_time,
            )),
            preferred_maintenance_window: Some(self.preferred_maintenance_window.clone()),
            topic_arn: self.topic_arn.clone(),
            port: Some(self.port),
            cache_parameter_group_name: Some(self.cache_parameter_group_name.clone()),
            cache_subnet_group_name: self.cache_subnet_group_name.clone(),
            vpc_id: self.vpc_id.clone(),
            auto_minor_version_upgrade: self.auto_minor_version_upgrade,
            snapshot_retention_limit: self.snapshot_retention_limit,
            snapshot_window: self.snapshot_window.clone(),
            // The number of node groups (shards) and the automatic-failover
            // status are the source replication group's; a cache cluster's
            // snapshot carries neither.
            num_node_groups: None,
            automatic_failover: None,
            node_snapshots: Some(
                self.node_snapshots
                    .iter()
                    .map(|node| node.to_modeled(include_node_group_config))
                    .collect(),
            ),
            kms_key_id: self.kms_key_id.clone(),
            arn: Some(self.arn.clone()),
            // Data tiering is "only supported for replication groups using the
            // r6gd node type" and durability is set on a cluster this API
            // cannot create, so no snapshot here was taken with either.
            data_tiering: None,
            durability: None,
        }
    }
}
