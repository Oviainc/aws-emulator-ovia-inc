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
//! the request that created the cluster — or from the `UpdateCluster` that
//! changed it since — or derived from the stored state
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
//! settles into `ACTIVE`, and the `DELETING` an accepted delete starts settles
//! into the terminal `DELETED` the record then keeps. The instant is stored and
//! never slept on — [`ComAmazonawsMedialiveMedialiveState::settle`], which every
//! operation runs first, is what materialises the ones the clock has already
//! made due.
//!
//! A DELETE never drops the record: a deleted cluster stays readable in that
//! terminal state, so `DescribeCluster` and `ListClusters` go on reporting it
//! there instead of losing it, while a caller may no longer CHANGE it — every
//! mutation addressed to a cluster resolves its id through
//! [`ComAmazonawsMedialiveMedialiveState::live_cluster`]. A cluster is deleted
//! only once it is idle, which is what
//! [`ComAmazonawsMedialiveMedialiveState::delete_cluster`] resolves against the
//! rest of the store.
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
    ClusterNetworkSettingsUpdateRequest, DescribeClusterSummary, InterfaceMapping,
};

use super::{ComAmazonawsMedialiveMedialiveState, TRANSIENT_STATE_SPAN};

/// The `ClusterState` values this slice uses, spelled exactly as the model
/// declares them (`smithy_query(query="shape", shape="ClusterState")`: CREATING
/// | CREATE_FAILED | ACTIVE | DELETING | DELETE_FAILED | DELETED). The two the
/// model declares but this slice does not name are the ones nothing here can
/// reach: `CREATE_FAILED` and `DELETE_FAILED` report a failure of the
/// provisioning the real service performs for a Cluster, which this emulator
/// does not perform and therefore never claims.
pub mod cluster_state {
    /// The cluster is coming up; the clock owes it [`ACTIVE`].
    pub const CREATING: &str = "CREATING";
    /// The cluster is up.
    pub const ACTIVE: &str = "ACTIVE";
    /// A delete has been accepted; the clock owes the terminal [`DELETED`].
    pub const DELETING: &str = "DELETING";
    /// The accepted delete has come due. TERMINAL: the record keeps this state,
    /// and stays readable in it.
    pub const DELETED: &str = "DELETED";
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

/// The most `InterfaceMapping` entries this emulator stores for ONE cluster.
///
/// A ceiling for the same reason as [`MAX_CLUSTERS`], and the one that bounds
/// the per-read derivation on the network side
/// ([`ComAmazonawsMedialiveMedialiveState::clusters_using_network`]): a mapping
/// list that grew without bound would make both the store and that scan grow
/// without bound. "You need only one mapping for each interface because all the
/// Nodes share the same interfaces" (`ClusterNetworkSettings$InterfaceMappings`,
/// `smithy_query(query="shape", shape="ClusterNetworkSettings")`), so a list this
/// long already names more interfaces than a node has; `CreateCluster` refuses a
/// longer one with its own modelled `BadRequestException` rather than storing it.
pub const MAX_CLUSTER_INTERFACE_MAPPINGS: usize = 32;

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

/// Why [`ComAmazonawsMedialiveMedialiveState::apply_cluster_changes`] changed
/// nothing. Each variant is one refusal `UpdateCluster` answers with, and each
/// leaves the stored cluster exactly as it was.
#[derive(Debug)]
pub enum ClusterUpdateRefusal {
    /// The `ClusterId` names no cluster this store holds — the same resolution a
    /// read of the cluster performs.
    NoSuchCluster,
    /// The store still holds the cluster — a delete keeps the record readable —
    /// but that delete has been accepted, so the id names nothing a caller may
    /// still change.
    AlreadyDeleted,
    /// Another cluster of the account already carries the new `Name`.
    NameTaken,
    /// The request named more `InterfaceMapping` entries than
    /// [`MAX_CLUSTER_INTERFACE_MAPPINGS`]. Carries how many it named.
    TooManyInterfaceMappings { count: usize },
    /// One `InterfaceMapping` entry of the request named a `NetworkId` this
    /// account holds no live network under. Carries the entry's position in the
    /// list the request sent and the id it spelled, both read off the request.
    UnknownMappedNetwork { index: usize, network_id: String },
}

/// What an `UpdateCluster` request asks to change.
///
/// Each member is `Some` only when the request carried it — "Include this
/// parameter only if you want to change the current ..." on both of them — so an
/// omitted member leaves that setting exactly as it was. One struct rather than
/// two positional `Option`s on
/// [`ComAmazonawsMedialiveMedialiveState::apply_cluster_changes`], so a handler
/// cannot pass the name where the network settings belong.
#[derive(Debug, Clone, Default)]
pub struct ClusterChanges {
    pub name: Option<String>,
    pub network_settings: Option<ClusterNetworkSettingsUpdateRequest>,
}

/// Why [`ComAmazonawsMedialiveMedialiveState::delete_cluster`] deleted nothing.
/// Each variant is one refusal `DeleteCluster` answers with, and each leaves the
/// stored cluster exactly as it was.
#[derive(Debug)]
pub enum ClusterDeleteRefusal {
    /// The `ClusterId` names no cluster this store holds.
    NoSuchCluster,
    /// The cluster still holds resources, so it is not idle. Carries them as the
    /// refusal names them, in the order
    /// [`ComAmazonawsMedialiveMedialiveState::resources_in_cluster`] derives
    /// them.
    NotIdle { resources: Vec<String> },
}

/// The message a delete refused on a cluster that still holds resources
/// carries: the cluster the REQUEST named, how many resources are still in it
/// and which they are, then the rule the delete is held to ("Delete a Cluster.
/// The Cluster must be idle").
///
/// The same shape as the network side's own refusal for the same rejection
/// class, so the two cannot word "this resource still holds something" two
/// ways. Every value is read off the request and the store.
pub fn cluster_not_idle_message(cluster_id: &str, resources: &[String]) -> String {
    format!(
        "Cluster {cluster_id} still has {} resource(s) in it ({}); a cluster must be idle \
         before it can be deleted",
        resources.len(),
        resources.join(", ")
    )
}

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
    /// The request named more `InterfaceMapping` entries than
    /// [`MAX_CLUSTER_INTERFACE_MAPPINGS`]. Carries how many it named.
    TooManyInterfaceMappings { count: usize },
    /// One `InterfaceMapping` entry of the request named a `NetworkId` this
    /// account holds no live network under. Carries the entry's position in the
    /// list the request sent and the id it spelled, both read off the request.
    UnknownMappedNetwork { index: usize, network_id: String },
}

/// The message a write carrying more `InterfaceMapping` entries than this
/// emulator stores for ONE cluster is refused with: the member as the wire
/// spells it, the ceiling and the count the request carried.
///
/// One helper for both writers of the list, so a create and an update cannot
/// word the one bound two ways. Every value is read off the request and the
/// ceiling.
pub fn too_many_cluster_interface_mappings_message(count: usize) -> String {
    format!(
        "networkSettings.interfaceMappings takes at most {MAX_CLUSTER_INTERFACE_MAPPINGS} entries, \
         but was {count}"
    )
}

/// The member path the wire spells the `NetworkId` of ONE `InterfaceMapping`
/// entry under: the list member, the entry's position in the list the request
/// sent, and the member inside the entry.
///
/// One helper, so the create and the update cannot name the member two ways in
/// the refusal they word with it.
pub fn mapped_network_member(index: usize) -> String {
    format!("networkSettings.interfaceMappings[{index}].networkId")
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

/// One created cluster, as `CreateCluster` stored it, as `UpdateCluster` has
/// changed it since, plus its lifecycle state and its alert log.
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
    /// specified none, never invented. `UpdateCluster`'s own `Name` is
    /// documented "Include this parameter only if you want to change the current
    /// name of the Cluster", so this is also what a rename writes.
    pub name: Option<String>,
    /// "The hardware type for the Cluster", as the request spelled it.
    pub cluster_type: Option<String>,
    /// "The ARN of the IAM role for the Node in this Cluster", as the request
    /// spelled it. Stored verbatim: this emulator runs no IAM, so it neither
    /// resolves nor validates the role beyond its ARN syntax.
    pub instance_role_arn: Option<String>,
    /// The network settings the request supplied, in the response shape
    /// ([`described_cluster_network_settings`]), member by member as the latest
    /// `UpdateCluster` left them.
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

    /// Whether this cluster's deletion has come due, which is what puts it in
    /// the terminal `DELETED` state it then keeps.
    pub fn is_deleted(&self) -> bool {
        self.state == cluster_state::DELETED
    }

    /// Whether a delete has been accepted for this cluster, in either half of
    /// the deletion lifecycle — so a repeated `DeleteCluster` is idempotent
    /// rather than restarting the clock, a cluster on its way out no longer
    /// holds the name it was created under
    /// ([`ComAmazonawsMedialiveMedialiveState::cluster_named`]), and no mutation
    /// addressed to it still resolves
    /// ([`ComAmazonawsMedialiveMedialiveState::live_cluster`]).
    pub fn is_delete_accepted(&self) -> bool {
        self.state == cluster_state::DELETING || self.is_deleted()
    }

    /// Accept a delete for this cluster: it moves into `DELETING` and owes the
    /// terminal `DELETED` once [`TRANSIENT_STATE_SPAN`] has passed.
    ///
    /// Deleting a cluster is asynchronous — the model answers `DeleteCluster`
    /// with `202 Accepted` and the response reports the cluster in `DELETING` —
    /// so the caller polls `DescribeCluster` for the terminal state, which the
    /// record then keeps.
    ///
    /// IDEMPOTENT: a delete already accepted keeps the instant the first call
    /// set, so a repeat can neither restart the clock nor revive a deleted
    /// cluster. A creation still in flight is REPLACED rather than carried: the
    /// cluster is leaving the account, so the one transition the clock still
    /// owes it is the deletion.
    fn begin_delete(&mut self, now: SystemTime) {
        if self.is_delete_accepted() {
            return;
        }
        self.state = cluster_state::DELETING.to_string();
        self.transition = Some(ClusterTransition {
            due: now + TRANSIENT_STATE_SPAN,
            to: cluster_state::DELETED,
        });
    }

    /// Materialise the transition the clock has already made due — a cluster AWS
    /// answers `CREATING` becomes `ACTIVE` once the span has passed, and one
    /// whose delete was accepted becomes the terminal `DELETED` — answering
    /// whether THIS call is the one that settled the deletion.
    fn settle(&mut self, now: SystemTime) -> bool {
        let Some(transition) = self.transition else {
            return false;
        };
        if now >= transition.due {
            self.state = transition.to.to_string();
            self.transition = None;
            return self.is_deleted();
        }
        false
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
        // Only a cluster that settles into the terminal `DELETED` on THIS call
        // is collected, so the work here does not grow with the number of
        // clusters that have already reached that state.
        let mut just_deleted: Vec<String> = Vec::new();
        for (id, cluster) in &mut self.clusters {
            if cluster.settle(now) {
                just_deleted.push(id.clone());
            }
        }
        // A token never outlives the cluster it minted: once that cluster has
        // reached the terminal `DELETED` state its idempotency window is over,
        // so a retried create mints a new cluster rather than answering with a
        // deleted one — the rule the channel and network tokens follow too.
        for id in just_deleted {
            self.cluster_tokens.forget(&id);
        }
    }

    /// The next cluster id, as AWS shapes MediaLive resource ids: a numeric
    /// string.
    fn mint_cluster_id(&mut self) -> String {
        (FIRST_CLUSTER_ID + self.mints.next(|mints| &mut mints.cluster)).to_string()
    }

    /// The id of the cluster stored under `name`, if the account holds one.
    ///
    /// Case-SENSITIVE, because `Name` documents "Names are case-sensitive", so
    /// two clusters whose names differ only in case are two different names.
    ///
    /// A cluster whose delete this service has accepted is skipped: it is on its
    /// way out of the account, so the name it was created under is free for the
    /// next create even though its record stays readable under that name — the
    /// rule the network side applies to a network that is leaving
    /// ([`ComAmazonawsMedialiveMedialiveState::network_named`]).
    ///
    /// The uniqueness both the create and the UPDATE resolve against: a rename
    /// is what `UpdateCluster$Name` is for, and this lookup is what tells it
    /// apart from a clash with another cluster of the account (see
    /// [`ComAmazonawsMedialiveMedialiveState::apply_cluster_changes`]).
    pub fn cluster_named(&self, name: &str) -> Option<&str> {
        self.clusters
            .values()
            .find(|cluster| {
                cluster.name.as_deref() == Some(name) && !cluster.is_delete_accepted()
            })
            .map(|cluster| cluster.id.as_str())
    }

    /// The cluster `id` names, if a caller may still CHANGE it.
    ///
    /// A delete keeps the record READABLE, but an id whose delete has been
    /// accepted names no cluster a write may still reach, so every mutation
    /// addressed to a cluster — the cluster's own update, and the creates of the
    /// resources that live inside one — resolves its id through here and answers
    /// a dead id as not found rather than with the conflict a cluster that still
    /// exists gets.
    pub fn live_cluster(&self, id: &str) -> Option<&ClusterRecord> {
        self.clusters
            .get(id)
            .filter(|cluster| !cluster.is_delete_accepted())
    }

    /// Store a cluster created in `region` and answer its minted id, or refuse.
    ///
    /// The documented name uniqueness, the bound on the mapping list, the
    /// resolution of every network the request's mappings name
    /// ([`Self::unresolved_mapped_network`]), the store's cap, the mint and the
    /// insert live together here, so a handler cannot check one of them and
    /// write past another; the handler's job is only to turn a
    /// [`ClusterRefusal`] into the operation's own modelled error.
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
        // The list is bounded BEFORE anything is resolved or minted, for the
        // reason on [`MAX_CLUSTER_INTERFACE_MAPPINGS`]: it is the bound that
        // keeps both this store and the network side's per-read derivation
        // bounded, so a refused create stores no cluster at all.
        if let Some(count) = new
            .network_settings
            .as_ref()
            .and_then(|settings| settings.interface_mappings.as_ref())
            .map(Vec::len)
            .filter(|count| *count > MAX_CLUSTER_INTERFACE_MAPPINGS)
        {
            return Err(ClusterRefusal::TooManyInterfaceMappings { count });
        }
        // Every network the mappings name is resolved against this account
        // BEFORE anything is minted, so a refused create stores no cluster and
        // no stored cluster carries a reference no read of the account can
        // resolve.
        if let Some((index, network_id)) = self.unresolved_mapped_network(
            new.network_settings
                .as_ref()
                .and_then(|settings| settings.interface_mappings.as_ref())
                .into_iter()
                .flatten()
                .map(|mapping| mapping.network_id.as_deref()),
        ) {
            return Err(ClusterRefusal::UnknownMappedNetwork { index, network_id });
        }
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

    /// Apply an `UpdateCluster` request's changes to the cluster `id` names, or
    /// refuse.
    ///
    /// Every refusal is checked BEFORE the first write, so a refused update
    /// leaves the stored cluster exactly as it was. A member the request omitted
    /// is not changed ("Include this parameter only if you want to change the
    /// current ..." on `Name`, on `NetworkSettings$DefaultRoute` and on
    /// `NetworkSettings$InterfaceMappings`), and neither is a member no update
    /// request carries at all: the hardware type, the instance role, the tags and
    /// the ARN are not something this operation can move.
    ///
    /// The documented account-wide name uniqueness, the stored mapping bound
    /// and the resolution of every network the mappings name are the create's
    /// own rules, checked here too so the two cannot disagree about what a
    /// stored cluster may look like. A rename is accepted: the
    /// `Name` member exists on the update request to "change the current name of
    /// the Cluster", and the "You can't change the name" sentence beside it is
    /// the create's own wording carried over (see
    /// `crate::ops::update_cluster`).
    pub fn apply_cluster_changes(
        &mut self,
        id: &str,
        changes: ClusterChanges,
    ) -> Result<(), ClusterUpdateRefusal> {
        // Resolved through the LIVE record, like every other mutation of this
        // service: a delete keeps the record readable, but an id whose delete
        // has been accepted names nothing a caller may still change, and that
        // resolution comes before anything about the request is looked at.
        match self.clusters.get(id) {
            None => return Err(ClusterUpdateRefusal::NoSuchCluster),
            Some(cluster) if cluster.is_delete_accepted() => {
                return Err(ClusterUpdateRefusal::AlreadyDeleted)
            }
            Some(_) => {}
        }
        // Bounded before anything is written, for the reason on
        // [`MAX_CLUSTER_INTERFACE_MAPPINGS`]: it is the bound that keeps the
        // network side's per-read derivation bounded too, so an update cannot
        // grow a stored list past what a create may store.
        if let Some(mappings) = changes
            .network_settings
            .as_ref()
            .and_then(|settings| settings.interface_mappings.as_ref())
        {
            if mappings.len() > MAX_CLUSTER_INTERFACE_MAPPINGS {
                return Err(ClusterUpdateRefusal::TooManyInterfaceMappings {
                    count: mappings.len(),
                });
            }
            // The create's own resolution, applied to the list this update
            // would store: an update may no more leave a cluster mapped onto a
            // network the account does not hold than a create may store one.
            if let Some((index, network_id)) = self.unresolved_mapped_network(
                mappings.iter().map(|mapping| mapping.network_id.as_deref()),
            ) {
                return Err(ClusterUpdateRefusal::UnknownMappedNetwork { index, network_id });
            }
        }
        // A name another cluster of the account holds is refused, while
        // re-sending the cluster's own name is not a collision with itself.
        if let Some(name) = changes.name.as_deref() {
            if self.cluster_named(name).is_some_and(|holder| holder != id) {
                return Err(ClusterUpdateRefusal::NameTaken);
            }
        }

        let cluster = self
            .clusters
            .get_mut(id)
            .ok_or(ClusterUpdateRefusal::NoSuchCluster)?;
        if let Some(name) = changes.name {
            cluster.name = Some(name);
        }
        if let Some(settings) = changes.network_settings {
            // A cluster created without network settings gains the ones this
            // request carries, rather than dropping them: the members the
            // request named are what it asked to be connected by.
            let stored = cluster.network_settings.get_or_insert_with(Default::default);
            update_cluster_network_settings(stored, settings);
        }
        Ok(())
    }

    /// The first `InterfaceMapping` entry of `network_ids` whose `NetworkId`
    /// names no live network of this account, as its position in the list and
    /// the id it spelled — or `None` when every entry that names a network
    /// resolves.
    ///
    /// `NetworkId` is "The ID of the network that you want to connect to the
    /// specified logicalInterfaceName. You can use the ListNetworks operation to
    /// discover all the IDs" (`smithy_query(query="shape",
    /// shape="InterfaceMappingUpdateRequest")`), so the ids that resolve are
    /// exactly the ones a listing of this account reports — which is what
    /// [`Self::live_network`] answers. A network whose delete this service has
    /// accepted is on its way out of the account and resolves no longer, so
    /// nothing new is connected to it.
    ///
    /// ONE resolution for every operation that WRITES a cluster's mappings
    /// (`CreateCluster` through [`Self::insert_cluster`], `UpdateCluster`
    /// through [`Self::apply_cluster_changes`]), taking the ids as the entries
    /// spell them, so the two cannot accept a reference the other refuses
    /// whichever of the three `InterfaceMapping` shapes the request carried. An
    /// entry that names no network at all connects that interface to none, so it
    /// is no dangling reference and is left as it is.
    pub fn unresolved_mapped_network<'a>(
        &self,
        network_ids: impl IntoIterator<Item = Option<&'a str>>,
    ) -> Option<(usize, String)> {
        network_ids
            .into_iter()
            .enumerate()
            .filter_map(|(index, network_id)| network_id.map(|id| (index, id)))
            .find(|(_, network_id)| self.live_network(network_id).is_none())
            .map(|(index, network_id)| (index, network_id.to_string()))
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
    /// on. A channel whose delete this service has ACCEPTED is no longer
    /// associated with anything, so it is not reported — the one cut point every
    /// derived "currently using" list of this store applies.
    pub fn channels_in_cluster(&self, cluster_id: &str) -> Vec<String> {
        self.channels
            .values()
            .filter(|channel| !channel.is_delete_accepted())
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

    /// Everything the cluster `cluster_id` still holds, each labelled by its
    /// resource type and named by its id \u2014 what makes a cluster NOT idle, and
    /// what the refusal of its delete reports.
    ///
    /// A cluster is the container of MediaLive Anywhere's three per-cluster
    /// resources: the channels that run on it (the association
    /// [`Self::channels_in_cluster`] derives), the channel placement groups
    /// created in it and the nodes registered with it. A member stops being held
    /// when it has reached its OWN terminal state \u2014 the deleted channel's and
    /// group's `DELETED`, the node's `DEREGISTERED` \u2014 because a resource in that
    /// state is no longer in the cluster, which is the same reading
    /// [`Self::channels_in_cluster`] already applies to a deleted channel.
    ///
    /// BOUNDED work: each store it scans is bounded by its own ceiling
    /// ([`super::MAX_CHANNELS`], [`super::MAX_CHANNEL_PLACEMENT_GROUPS`],
    /// [`super::MAX_NODES`]).
    pub fn resources_in_cluster(&self, cluster_id: &str) -> Vec<String> {
        let channels = self
            .channels_in_cluster(cluster_id)
            .into_iter()
            .map(|id| format!("Channel {id}"));
        let groups = self
            .channel_placement_groups
            .values()
            .filter(|group| group.cluster_id == cluster_id && !group.is_deleted())
            .map(|group| format!("ChannelPlacementGroup {}", group.id));
        let nodes = self
            .nodes
            .values()
            .filter(|node| node.cluster_id == cluster_id && !node.is_deregistered())
            .map(|node| format!("Node {}", node.id));
        channels.chain(groups).chain(nodes).collect()
    }

    /// Accept a delete for the cluster `id` names and answer the cluster as it
    /// then stands, or refuse.
    ///
    /// "Delete a Cluster. The Cluster must be idle": a cluster that still holds
    /// any of the resources that live inside one ([`Self::resources_in_cluster`])
    /// is refused, and the refusal is taken BEFORE the first write, so a refused
    /// delete leaves the stored cluster exactly as it was. An accepted delete
    /// moves the cluster into `DELETING` and owes it the terminal `DELETED`,
    /// which the record keeps while staying readable.
    ///
    /// IDEMPOTENT: a delete already accepted writes nothing and is accepted
    /// again \u2014 the state the caller is asking for is the one the cluster is
    /// already heading for or in, so nothing about the request conflicts with
    /// the stored cluster, and a repeat neither restarts the clock nor revives a
    /// deleted cluster. It is also not held to the idle rule a second time: the
    /// resources it held are free to leave in any order after the acceptance.
    pub fn delete_cluster(
        &mut self,
        id: &str,
        now: SystemTime,
    ) -> Result<DescribeClusterSummary, ClusterDeleteRefusal> {
        let Some(cluster) = self.clusters.get(id) else {
            return Err(ClusterDeleteRefusal::NoSuchCluster);
        };
        if !cluster.is_delete_accepted() {
            let resources = self.resources_in_cluster(id);
            if !resources.is_empty() {
                return Err(ClusterDeleteRefusal::NotIdle { resources });
            }
            self.clusters
                .get_mut(id)
                .ok_or(ClusterDeleteRefusal::NoSuchCluster)?
                .begin_delete(now);
        }
        // Read back out of the store, so the answer reports the cluster as it
        // now stands rather than one assembled here.
        self.cluster_description(id)
            .ok_or(ClusterDeleteRefusal::NoSuchCluster)
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
/// `NetworkId` names one of the Networks the cluster is associated with, and it
/// is stored as spelled — a network of this account, because the write that
/// stored it resolved every mapped id first
/// ([`ComAmazonawsMedialiveMedialiveState::unresolved_mapped_network`]). It is
/// what the network side of the store reads that association back out of
/// ([`ComAmazonawsMedialiveMedialiveState::clusters_using_network`]).
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

/// Write a `ClusterNetworkSettingsUpdateRequest` onto the stored
/// `ClusterNetworkSettings`, member by member.
///
/// Each member of the update request is documented "Include this parameter only
/// if you want to change the current ...", so one the request omitted keeps the
/// value the cluster already had, and one it carried REPLACES that value whole —
/// `InterfaceMappings` is "an array of interfaceMapping objects for this
/// Cluster", the list as it is to be from now on, not entries to add to it.
///
/// The per-entry move is the same request-to-response move
/// [`described_cluster_network_settings`] makes for the create, so a mapping
/// stored by an update is indistinguishable from one stored by a create.
fn update_cluster_network_settings(
    stored: &mut ClusterNetworkSettings,
    changes: ClusterNetworkSettingsUpdateRequest,
) {
    if let Some(default_route) = changes.default_route {
        stored.default_route = Some(default_route);
    }
    if let Some(mappings) = changes.interface_mappings {
        stored.interface_mappings = Some(
            mappings
                .into_iter()
                .map(|mapping| InterfaceMapping {
                    logical_interface_name: mapping.logical_interface_name,
                    network_id: mapping.network_id,
                })
                .collect(),
        );
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
/// per-operation FLAT cluster output shapes (`CreateClusterOutput`,
/// `DescribeClusterOutput`, `DeleteClusterOutput` and `UpdateClusterOutput`).
///
/// One macro rather than a hand-written copy of the same eight-member move per
/// operation, so the operations that report a cluster cannot drift from each
/// other, and adding a member to the model cannot be defaulted in silently.
///
/// The second form is for an output shape the MODEL gives no `InstanceRoleArn`
/// member (`smithy_query(query="operation", operation="UpdateCluster")`): the
/// seven members it does declare are the same stored projection, and the role is
/// dropped because that response does not carry it — not because the store
/// forgot it.
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
    ($out:ident, $cluster:expr, without_instance_role_arn) => {{
        let cluster: $crate::ops::types::DescribeClusterSummary = $cluster;
        $out {
            arn: cluster.arn,
            channel_ids: cluster.channel_ids,
            cluster_type: cluster.cluster_type,
            id: cluster.id,
            name: cluster.name,
            network_settings: cluster.network_settings,
            state: cluster.state,
        }
    }};
}

pub(crate) use cluster_description_output;
