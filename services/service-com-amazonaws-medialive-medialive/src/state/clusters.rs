// SPDX-License-Identifier: CC0-1.0

//! The cluster slice of this service's state — MediaLive Anywhere's `Cluster`
//! resource and the alerts raised against one.
//!
//! One file for the whole slice — the record, its bounds, its refusals, the
//! store's methods and the output projections — so the slice can be read, and
//! merged, on its own. The store itself is the `clusters` member of
//! [`ComAmazonawsMedialiveMedialiveState`] in the parent module, beside the
//! channels and the multiplexes, so every operation that reaches a cluster reads
//! the one copy.
//!
//! Every value a cluster-shaped response carries is either stored verbatim from
//! the request that created the cluster or derived from the stored state
//! ([`ComAmazonawsMedialiveMedialiveState::cluster_description`]) — no operation
//! builds a cluster out of its own request. `ChannelIds` is the derived one:
//! "An array of the IDs of the Channels that are associated with this Cluster"
//! comes from the channels this store holds
//! ([`ComAmazonawsMedialiveMedialiveState::channels_in_cluster`]), never from a
//! list recorded at create time.
//!
//! A cluster's LIFECYCLE is one stored pending transition per record (a
//! [`ClusterTransition`]: the instant it comes due and the `ClusterState` it
//! settles into), the same shape as the multiplex slice's own: `CREATING`
//! settles into `ACTIVE`. The instant is stored and never slept on —
//! [`ComAmazonawsMedialiveMedialiveState::settle`], which every operation runs
//! first, is what materialises the ones the clock has already made due.
//!
//! The ALERTS a cluster carries live on the [`ClusterRecord`] that owns them:
//! one [`ClusterAlertRecord`] per alert raised against the cluster, in the order
//! they were raised, which is the sequence `ListClusterAlerts` reports and
//! paginates over. An alert's `State` is DERIVED from its cleared timestamp
//! ("SET - The alert is actively happening. CLEARED - The alert is no longer
//! happening", beside `ClearedTimestamp`: "The time when the alert was
//! cleared"), so the two can never disagree.
//!
//! NOTHING in this emulator raises a cluster alert, and nothing here invents
//! one: the real service raises them from the Nodes and the encoding hardware it
//! runs for a Cluster (an `AlertType` out of a catalog AWS publishes nowhere in
//! this model), and this emulator runs neither. So a cluster created here
//! carries an empty alert log, and `ListClusterAlerts` reports that log —
//! whatever is in it — rather than a fabricated alert.
//! [`ComAmazonawsMedialiveMedialiveState::raise_cluster_alert`] is the one way
//! an alert gets into the log, bounded by [`MAX_CLUSTER_ALERTS`].

use std::collections::HashMap;
use std::time::SystemTime;

use crate::ops::types::{
    ClusterAlert, ClusterNetworkSettings, ClusterNetworkSettingsCreateRequest,
    DescribeClusterSummary, InterfaceMapping,
};

use super::{ComAmazonawsMedialiveMedialiveState, TRANSIENT_STATE_SPAN};

/// The `ClusterState` values this slice uses, spelled exactly as the model
/// declares them (`smithy_query(query="shape", shape="ClusterState")`: CREATING
/// | CREATE_FAILED | ACTIVE | DELETING | DELETE_FAILED | DELETED). The four the
/// model declares but this slice does not name are the ones nothing here can
/// reach: `CREATE_FAILED` and `DELETE_FAILED` report a failure of the
/// provisioning the real service performs for a Cluster, which this emulator
/// does not perform and therefore never claims, and `DELETING`/`DELETED` belong
/// to `DeleteCluster`, which is still a scaffold (its own issue,
/// `20261024T010738Z-medialive-deletecluster-stub.md`) and so is where that half
/// of the lifecycle lands.
pub mod cluster_state {
    pub const CREATING: &str = "CREATING";
    pub const ACTIVE: &str = "ACTIVE";
}

/// The `ClusterAlertState` values, spelled exactly as the model declares them
/// (`smithy_query(query="shape", shape="ClusterAlertState")`: SET | CLEARED —
/// "SET - The alert is actively happening. CLEARED - The alert is no longer
/// happening").
///
/// The same two values every alert-state enum of this service declares, so they
/// live once in [`super::alerts`] and this is the cluster slice's name for them.
pub use super::alerts::alert_state as cluster_alert_state;

/// The `StateFilter` value that asks for every alert whatever its state, from
/// `ListClusterAlertsRequest$StateFilter`'s own documentation: "SET - Return
/// only alerts with SET state. CLEARED - Return only alerts with CLEARED state.
/// ALL - Return all alerts." Shared with every other alert listing of this
/// service ([`super::alerts::ALERT_STATE_FILTER_ALL`]), whose member documents
/// the same set.
pub const CLUSTER_ALERT_FILTER_ALL: &str = super::alerts::ALERT_STATE_FILTER_ALL;

/// Every `StateFilter` value that request member documents, in the order it
/// documents them. The member targets `__string`, which carries no enum trait,
/// so the request boundary cannot refuse any other value and this is the
/// documented value set `ListClusterAlerts` refuses against — the shared
/// [`super::alerts::ALERT_STATE_FILTERS`], so two alert listings of this service
/// cannot accept different values.
pub const CLUSTER_ALERT_FILTERS: [&str; 3] = super::alerts::ALERT_STATE_FILTERS;

/// The id the first cluster of a process gets.
///
/// MediaLive resource ids are numeric strings, so they are minted from a counter
/// at a fixed seven-digit width, which keeps the `BTreeMap` key order equal to
/// the mint order. A range of its own keeps the service's id spaces apart: a
/// caller that passes a channel's or a multiplex's id where a cluster id belongs
/// gets a `NotFoundException` rather than a coincidental hit.
const FIRST_CLUSTER_ID: u64 = 8_000_000;

/// The most clusters this emulator stores for one account.
///
/// A ceiling, not a guess at AWS's published quota: it exists so a create loop
/// cannot grow the emulator's heap without bound. `CreateCluster` models
/// `TooManyRequestsException`, which is the refusal once the store is full.
pub const MAX_CLUSTERS: usize = 100;

/// The most `RequestId` entries kept for `CreateCluster` idempotency — the same
/// bound, for the same reason, as [`super::MAX_IDEMPOTENCY_TOKENS`] on channels:
/// a token is only useful while the cluster it created still exists.
pub const MAX_CLUSTER_TOKENS: usize = MAX_CLUSTERS;

/// The most alerts this emulator keeps for ONE cluster.
///
/// A ceiling for the same reason as [`MAX_CLUSTERS`]: an alert log that grew
/// without bound would grow the emulator's heap without bound. The OLDEST alert
/// is dropped once the log is full, because an alert log is a history and the
/// recent end of it is the useful one.
pub const MAX_CLUSTER_ALERTS: usize = 100;

/// Why [`ComAmazonawsMedialiveMedialiveState::insert_cluster`] stored nothing.
/// Each variant is one refusal `CreateCluster` models.
#[derive(Debug)]
pub enum ClusterRefusal {
    /// The request named a `Name` the account already holds a cluster under
    /// ("Specify a name that is unique in the AWS account ... Names are
    /// case-sensitive").
    NameAlreadyInUse,
    /// The store already holds [`MAX_CLUSTERS`] clusters.
    StoreFull,
}

/// One alert raised against a cluster.
///
/// The members mirror the model's `ClusterAlert` shape minus the one it derives:
/// `State` comes from [`Self::state`], out of whether the alert has been
/// cleared, rather than from a field that could disagree with
/// `ClearedTimestamp`.
#[derive(Debug, Clone)]
pub struct ClusterAlertRecord {
    /// "The type of the alert" — the condition that raised it.
    pub alert_type: Option<String>,
    /// "The further subtype of this alert" (the model's `Id` member), which is
    /// NOT a unique identifier of the alert, so it is not what the log is keyed
    /// or paginated by.
    pub id: Option<String>,
    /// "The user facing alert message which can have more context".
    pub message: Option<String>,
    /// "The ID of the channel this alert is associated with", when the condition
    /// is about one channel.
    pub channel_id: Option<String>,
    /// "The ID of the node this alert is associated with", when the condition is
    /// about one node.
    pub node_id: Option<String>,
    /// "The time when the alert was set".
    pub set_at: SystemTime,
    /// "The time when the alert was cleared", absent while the alert is still
    /// happening — which is exactly what makes it `SET`.
    pub cleared_at: Option<SystemTime>,
}

impl ClusterAlertRecord {
    /// An alert raised at `set_at` and still happening, with no channel, node,
    /// subtype or message of its own. The optional members are then filled in by
    /// whatever raised it.
    pub fn raised(alert_type: impl Into<String>, set_at: SystemTime) -> Self {
        Self {
            alert_type: Some(alert_type.into()),
            id: None,
            message: None,
            channel_id: None,
            node_id: None,
            set_at,
            cleared_at: None,
        }
    }

    /// "The state of the alert": `SET` while it is actively happening, `CLEARED`
    /// once it is not. Derived from [`Self::cleared_at`], never stored, through
    /// the one derivation every alert surface of this service shares.
    pub fn state(&self) -> &'static str {
        super::alerts::alert_state_of(self.cleared_at)
    }

    /// Whether `ListClusterAlerts`' `StateFilter` admits this alert: `SET` and
    /// `CLEARED` admit the alerts in that state, `ALL` admits every one, and a
    /// request that named no filter is not filtered at all — the one predicate
    /// every alert listing of this service shares.
    pub fn admitted_by(&self, filter: Option<&str>) -> bool {
        super::alerts::alert_admitted_by(self.cleared_at, filter)
    }
}

/// One created cluster, as `CreateCluster` stored it plus its lifecycle state
/// and its alert log.
///
/// The members mirror the model's `DescribeClusterSummary` shape minus the two
/// it derives: `State` is the lifecycle below and `ChannelIds` comes from the
/// channels this store holds, so both are computed on every read rather than
/// recorded at create time.
#[derive(Debug, Clone)]
pub struct ClusterRecord {
    pub id: String,
    pub arn: String,
    /// "The name that you specified for the Cluster" — absent when the request
    /// specified none, never invented.
    pub name: Option<String>,
    /// "The hardware type for the Cluster", as the request spelled it.
    pub cluster_type: Option<String>,
    /// "The ARN of the IAM role for the Node in this Cluster", as the request
    /// spelled it. Stored verbatim: this emulator runs no IAM, so it neither
    /// resolves nor validates the role beyond its ARN syntax.
    pub instance_role_arn: Option<String>,
    /// The network settings the request supplied, in the response shape
    /// ([`described_cluster_network_settings`]).
    pub network_settings: Option<ClusterNetworkSettings>,
    pub tags: HashMap<String, String>,
    /// The cluster's current `ClusterState`.
    state: String,
    /// The transition the clock owes this cluster while it is still coming up;
    /// `None` once it has settled.
    transition: Option<ClusterTransition>,
    /// The alerts raised against this cluster, oldest first, which is the
    /// sequence `ListClusterAlerts` reports. Nothing in this emulator raises one
    /// (see the module docs), so it is empty unless
    /// [`ComAmazonawsMedialiveMedialiveState::raise_cluster_alert`] put
    /// something in it.
    alerts: Vec<ClusterAlertRecord>,
}

impl ClusterRecord {
    /// The cluster's current `ClusterState`, which is stored rather than derived
    /// so a refusal can name it.
    pub fn state(&self) -> &str {
        &self.state
    }

    /// This cluster's alerts, oldest first.
    pub fn alerts(&self) -> &[ClusterAlertRecord] {
        &self.alerts
    }

    /// Materialise the transition the clock has already made due: a cluster AWS
    /// answers `CREATING` becomes `ACTIVE` once the span has passed.
    fn settle(&mut self, now: SystemTime) {
        let Some(transition) = self.transition else {
            return;
        };
        if now >= transition.due {
            self.state = transition.to.to_string();
            self.transition = None;
        }
    }
}

/// A transition the clock owes a cluster: the instant it becomes due and the
/// `ClusterState` it settles into. Stored, never slept on — the same shape as
/// the channel and multiplex slices' own, for the same reason.
#[derive(Debug, Clone, Copy)]
struct ClusterTransition {
    due: SystemTime,
    to: &'static str,
}

/// What a `CreateCluster` request supplies, as the store takes it.
///
/// One struct rather than five positional parameters on
/// [`ComAmazonawsMedialiveMedialiveState::insert_cluster`], so a handler cannot
/// pass the name where the role ARN belongs.
#[derive(Debug, Clone, Default)]
pub struct NewCluster {
    pub name: Option<String>,
    pub cluster_type: Option<String>,
    pub instance_role_arn: Option<String>,
    pub network_settings: Option<ClusterNetworkSettings>,
    pub tags: HashMap<String, String>,
}

impl ComAmazonawsMedialiveMedialiveState {
    /// Materialise every cluster transition the clock had already made due as of
    /// `now`. Called from [`ComAmazonawsMedialiveMedialiveState::settle`], which
    /// every operation runs first.
    pub(super) fn settle_clusters(&mut self, now: SystemTime) {
        for cluster in self.clusters.values_mut() {
            cluster.settle(now);
        }
    }

    /// The next cluster id, as AWS shapes MediaLive resource ids: a numeric
    /// string.
    fn mint_cluster_id(&mut self) -> String {
        let id = FIRST_CLUSTER_ID + self.next_cluster_id;
        self.next_cluster_id += 1;
        id.to_string()
    }

    /// The id of the cluster stored under `name`, if the account holds one.
    ///
    /// Case-SENSITIVE, because `Name` documents "Names are case-sensitive", so
    /// two clusters whose names differ only in case are two different names.
    pub fn cluster_named(&self, name: &str) -> Option<&str> {
        self.clusters
            .values()
            .find(|cluster| cluster.name.as_deref() == Some(name))
            .map(|cluster| cluster.id.as_str())
    }

    /// Store a cluster created in `region` and answer its minted id, or refuse.
    ///
    /// The documented name uniqueness, the store's cap, the mint and the insert
    /// live together here, so a handler cannot check one of them and write past
    /// another; the handler's job is only to turn a [`ClusterRefusal`] into the
    /// operation's own modelled error.
    ///
    /// A created cluster starts `CREATING` and owes `ACTIVE` once
    /// [`TRANSIENT_STATE_SPAN`] has passed, which is what the real service does:
    /// creation is asynchronous and a caller waits on `State`.
    pub fn insert_cluster(
        &mut self,
        region: &str,
        new: NewCluster,
        now: SystemTime,
    ) -> Result<String, ClusterRefusal> {
        if let Some(name) = new.name.as_deref() {
            if self.cluster_named(name).is_some() {
                return Err(ClusterRefusal::NameAlreadyInUse);
            }
        }
        if self.clusters.len() >= MAX_CLUSTERS {
            return Err(ClusterRefusal::StoreFull);
        }

        let id = self.mint_cluster_id();
        let arn = ::engine::emulator_arn("medialive", region, &format!("cluster:{id}"));
        self.clusters.insert(
            id.clone(),
            ClusterRecord {
                id: id.clone(),
                arn,
                name: new.name,
                cluster_type: new.cluster_type,
                instance_role_arn: new.instance_role_arn,
                network_settings: new.network_settings,
                tags: new.tags,
                state: cluster_state::CREATING.to_string(),
                transition: Some(ClusterTransition {
                    due: now + TRANSIENT_STATE_SPAN,
                    to: cluster_state::ACTIVE,
                }),
                alerts: Vec::new(),
            },
        );
        Ok(id)
    }

    /// Record `token` as having created the cluster `cluster_id`, evicting the
    /// oldest token first if that would push the map past
    /// [`MAX_CLUSTER_TOKENS`].
    pub fn record_cluster_token(&mut self, token: String, cluster_id: &str) {
        self.cluster_tokens
            .record(token, cluster_id, MAX_CLUSTER_TOKENS);
    }

    /// The cluster a stored `RequestId` created, if that cluster still exists.
    pub fn cluster_for_token(&self, request_id: &str) -> Option<&str> {
        let id = self.cluster_tokens.created(request_id)?;
        self.clusters.get(id).map(|cluster| cluster.id.as_str())
    }

    /// "An array of the IDs of the Channels that are associated with this
    /// Cluster."
    ///
    /// Derived from the channel store on every read, so it tracks every later
    /// create and delete rather than being recorded at create time. The
    /// association the store knows is the one a channel's own request carries:
    /// `AnywhereSettings$ClusterId`, "The ID of the cluster" the channel runs
    /// on. A channel that has reached the terminal `DELETED` state is no longer
    /// associated with anything, so it is not reported.
    pub fn channels_in_cluster(&self, cluster_id: &str) -> Vec<String> {
        self.channels
            .values()
            .filter(|channel| !channel.is_deleted())
            .filter(|channel| {
                channel
                    .anywhere_settings
                    .as_ref()
                    .and_then(|anywhere| anywhere.cluster_id.as_deref())
                    == Some(cluster_id)
            })
            .map(|channel| channel.id.clone())
            .collect()
    }

    /// The full description of one stored cluster, or `None` when no cluster has
    /// that id.
    ///
    /// `DescribeClusterSummary` is the model's own cluster shape (it is what
    /// `ListClusters` reports), and the flat per-operation cluster outputs carry
    /// exactly its members, so this is the one projection behind all of them
    /// ([`cluster_description_output`]) and they cannot drift.
    pub fn cluster_description(&self, id: &str) -> Option<DescribeClusterSummary> {
        let cluster = self.clusters.get(id)?;
        Some(DescribeClusterSummary {
            arn: Some(cluster.arn.clone()),
            channel_ids: Some(self.channels_in_cluster(id)),
            cluster_type: cluster.cluster_type.clone(),
            id: Some(cluster.id.clone()),
            instance_role_arn: cluster.instance_role_arn.clone(),
            name: cluster.name.clone(),
            network_settings: cluster.network_settings.clone(),
            state: Some(cluster.state.clone()),
        })
    }

    /// Append `alert` to the alert log of the cluster `cluster_id` names,
    /// answering whether a cluster with that id took it.
    ///
    /// Bounded, evicting write: the log keeps at most [`MAX_CLUSTER_ALERTS`]
    /// alerts and drops the oldest first, so a cluster's history cannot grow the
    /// emulator's heap without bound.
    ///
    /// The real service raises these from the Nodes and the encoding hardware it
    /// runs for a Cluster; this emulator runs neither, so no operation in it
    /// calls this and a cluster created through `CreateCluster` has an empty log
    /// (see the module docs). It is the ONE way an alert reaches the log, so
    /// `ListClusterAlerts` reports what was raised rather than anything it
    /// invented.
    pub fn raise_cluster_alert(&mut self, cluster_id: &str, alert: ClusterAlertRecord) -> bool {
        let Some(cluster) = self.clusters.get_mut(cluster_id) else {
            return false;
        };
        while cluster.alerts.len() >= MAX_CLUSTER_ALERTS {
            cluster.alerts.remove(0);
        }
        cluster.alerts.push(alert);
        true
    }
}

/// `ClusterNetworkSettingsCreateRequest` (request) -> `ClusterNetworkSettings`
/// (response), the same request-to-response move the channel settings make in
/// the parent module.
///
/// Every member is the value the request supplied: an `InterfaceMapping`'s
/// `NetworkId` names one of the Networks the cluster is associated with, and
/// this emulator holds no network store yet (`CreateNetwork` is still a
/// scaffold), so it is stored as spelled rather than resolved.
pub fn described_cluster_network_settings(
    settings: &ClusterNetworkSettingsCreateRequest,
) -> ClusterNetworkSettings {
    ClusterNetworkSettings {
        default_route: settings.default_route.clone(),
        interface_mappings: settings.interface_mappings.as_ref().map(|mappings| {
            mappings
                .iter()
                .map(|mapping| InterfaceMapping {
                    logical_interface_name: mapping.logical_interface_name.clone(),
                    network_id: mapping.network_id.clone(),
                })
                .collect()
        }),
    }
}

/// Project one stored [`ClusterAlertRecord`] onto the model's `ClusterAlert`.
///
/// `State` is derived from the record ([`ClusterAlertRecord::state`]) and the
/// timestamps are the stored instants in the model's `__timestampIso8601` type,
/// so no member here is read from a request.
pub fn cluster_alert_output(alert: &ClusterAlertRecord) -> ClusterAlert {
    ClusterAlert {
        alert_type: alert.alert_type.clone(),
        channel_id: alert.channel_id.clone(),
        cleared_timestamp: alert.cleared_at.map(super::timestamp),
        id: alert.id.clone(),
        message: alert.message.clone(),
        node_id: alert.node_id.clone(),
        set_timestamp: Some(super::timestamp(alert.set_at)),
        state: Some(alert.state().to_string()),
    }
}

/// Project a `DescribeClusterSummary` onto one of the structurally identical
/// per-operation FLAT cluster output shapes (`CreateClusterOutput` and
/// `DescribeClusterOutput` today; `UpdateClusterOutput` and `DeleteClusterOutput`
/// when their own issues land).
///
/// One macro rather than a hand-written copy of the same eight-member move per
/// operation, so the operations that report a cluster cannot drift from each
/// other, and adding a member to the model cannot be defaulted in silently.
macro_rules! cluster_description_output {
    ($out:ident, $cluster:expr) => {{
        let cluster: $crate::ops::types::DescribeClusterSummary = $cluster;
        $out {
            arn: cluster.arn,
            channel_ids: cluster.channel_ids,
            cluster_type: cluster.cluster_type,
            id: cluster.id,
            instance_role_arn: cluster.instance_role_arn,
            name: cluster.name,
            network_settings: cluster.network_settings,
            state: cluster.state,
        }
    }};
}

pub(crate) use cluster_description_output;
