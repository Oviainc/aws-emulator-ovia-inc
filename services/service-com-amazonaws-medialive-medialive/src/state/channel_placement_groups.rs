// SPDX-License-Identifier: CC0-1.0

//! The channel placement group slice of this service's state.
//!
//! One file for the whole slice — the record, its bounds, its refusals, the
//! store's methods and the output projection — so the slice can be read, and
//! merged, on its own, exactly as the [`super::alarm_template_groups`] slice is.
//! The store itself is the `channel_placement_groups` member of
//! [`ComAmazonawsMedialiveMedialiveState`] in the parent module.
//!
//! A group belongs to the CLUSTER it was created in: every operation on one
//! carries both `ClusterId` and `ChannelPlacementGroupId` as path labels
//! (`smithy_query(query="operation", operation="DescribeChannelPlacementGroup")`),
//! so the pair is what names the resource and a group is never visible through
//! another cluster's id.
//!
//! Two members of the response are DERIVED on every read rather than recorded at
//! create time: `Channels` ("Used in ListChannelPlacementGroupsResult", i.e. the
//! channels placed in this group) comes from the channel store — a channel names
//! its group in `AnywhereSettings$ChannelPlacementGroupId` — and `State` comes
//! from the group's own lifecycle, which the clock moves.
//!
//! A DELETE never drops the record, exactly as the channel's and the input's do
//! not: the group enters the transient `DELETING` state its own enum models and
//! the clock owes it the terminal `DELETED`, which it then keeps, so a reader
//! goes on reporting the group there instead of losing it. A group a delete has
//! been accepted for is on its way out of the account, so it reserves its name
//! no longer (the record stays readable under the name it was created with) and
//! associates no node (see
//! [`ChannelPlacementGroupRecord::is_delete_accepted`]).
//!
//! The ASSIGNMENT has two clock-owed steps of its own, because a group's node
//! association can be both made and unmade: `UpdateChannelPlacementGroup`
//! ("Change the settings for a ChannelPlacementGroup") can give a group a node,
//! swap the one it has, or take its last one away, so a group that now names a
//! node is `ASSIGNING` until `ASSIGNED` comes due and one whose nodes have just
//! been emptied is `UNASSIGNING` until `UNASSIGNED` does
//! ([`ChannelPlacementGroupRecord::begin_assignment`]). Every such instant is
//! stored and never slept on.

use std::collections::HashMap;
use std::time::SystemTime;

use super::{ComAmazonawsMedialiveMedialiveState, TRANSIENT_STATE_SPAN};

/// The `ChannelPlacementGroupState` enum values this slice uses, spelled exactly
/// as the model declares them (`smithy_query(query="shape",
/// shape="ChannelPlacementGroupState")`: UNASSIGNED | ASSIGNING | ASSIGNED |
/// DELETING | DELETE_FAILED | DELETED | UNASSIGNING).
pub mod channel_placement_group_state {
    /// A group that is associated with no node.
    pub const UNASSIGNED: &str = "UNASSIGNED";
    /// A group whose association with a node is in flight — the transient state
    /// the clock owes [`ASSIGNED`].
    pub const ASSIGNING: &str = "ASSIGNING";
    /// A group that is associated with its node.
    pub const ASSIGNED: &str = "ASSIGNED";
    /// A group whose last node is being taken away — the transient state the
    /// clock owes [`UNASSIGNED`]. Reached by an update that empties `Nodes`.
    pub const UNASSIGNING: &str = "UNASSIGNING";
    /// A group whose delete has been accepted; the clock owes [`DELETED`].
    /// What `DeleteChannelPlacementGroup` writes, and what the read path then
    /// reports until the deletion comes due.
    pub const DELETING: &str = "DELETING";
    /// A group whose delete has completed. Terminal.
    pub const DELETED: &str = "DELETED";
}

/// The id the first channel placement group of a process gets.
///
/// `Id` is "the resource-id portion of the ARN" and the model constrains it no
/// further (a plain `__string`), so it is minted from a counter at a fixed
/// seven-digit width, as every other id this service mints is: the `BTreeMap`
/// key order then equals the mint order, which is what makes a later
/// `ListChannelPlacementGroups` deterministic. A range of its own keeps the id
/// spaces of this service's resource types apart, as the cluster's and the SDI
/// source's own first-id constants do, so an id that names a cluster, a channel
/// or an SDI source does not accidentally hit a placement group.
const FIRST_CHANNEL_PLACEMENT_GROUP_ID: u64 = 9_000_000;

/// The most channel placement groups this emulator stores, across every cluster
/// this process has served.
///
/// A ceiling, not a guess at an AWS quota: it exists so a create loop cannot
/// grow the emulator's heap without bound. `CreateChannelPlacementGroup` models
/// `TooManyRequestsException`, which is the refusal once the store is full.
pub const MAX_CHANNEL_PLACEMENT_GROUPS: usize = 100;

/// The most `RequestId` entries kept for `CreateChannelPlacementGroup`
/// idempotency — kept for the same reason as
/// [`super::MAX_IDEMPOTENCY_TOKENS`] on channels (a token is only useful while
/// the group it created still exists), sized to this slice's own store cap.
pub const MAX_CHANNEL_PLACEMENT_GROUP_TOKENS: usize = MAX_CHANNEL_PLACEMENT_GROUPS;

/// The most node ids one channel placement group may name.
///
/// `CreateChannelPlacementGroupRequest$Nodes` is documented "An array of one ID
/// for the Node that you want to associate with the ChannelPlacementGroup. (You
/// can't associate more than one Node with the ChannelPlacementGroup.)", and
/// `Nodes` on the response shapes is "An array with one item, which is the
/// single Node that is associated with the ChannelPlacementGroup" — so one is
/// the documented maximum, not a ceiling this emulator chose.
pub const MAX_CHANNEL_PLACEMENT_GROUP_NODES: usize = 1;

/// The message a request that named more nodes than the member allows carries:
/// the member, the documented bound and the count the request carried, so the
/// caller can fix the request from that text alone.
///
/// One helper, so the create and the update cannot word the one refusal two
/// ways.
pub fn too_many_nodes_message(count: usize) -> String {
    format!("Nodes accepts at most {MAX_CHANNEL_PLACEMENT_GROUP_NODES} node id, but {count} were supplied")
}

/// The message a request that named a node this cluster has nothing to associate
/// carries: the member, the id the request supplied and the cluster it was
/// resolved against, all built from the request itself so the caller can fix it
/// from that text alone.
///
/// One helper, so the create and the update cannot word the one refusal two
/// ways.
pub fn unassociable_node_message(cluster_id: &str, node_id: &str) -> String {
    format!(
        "Nodes names {node_id}, which is not a Node of Cluster {cluster_id} that a \
         ChannelPlacementGroup can be associated with"
    )
}

/// Why [`ComAmazonawsMedialiveMedialiveState::insert_channel_placement_group`]
/// stored nothing. Each variant is one refusal, and each leaves the store
/// exactly as it was.
#[derive(Debug)]
pub enum ChannelPlacementGroupRefusal {
    /// No cluster in the store carries the `ClusterId` the request's path named,
    /// so there is nothing to place the group in.
    NoSuchCluster,
    /// The request named more node ids than
    /// [`MAX_CHANNEL_PLACEMENT_GROUP_NODES`]. Carries how many it named.
    TooManyNodes { count: usize },
    /// A node id the request named is not a node of this cluster that a group
    /// can be associated with. Carries that id, so the refusal can name it.
    UnassociableNode { node_id: String },
    /// A group in the same cluster already carries that name, which `Name` is
    /// documented to make unique there ("Specify a name that is unique in the
    /// Cluster").
    NameTaken,
    /// The store already holds [`MAX_CHANNEL_PLACEMENT_GROUPS`] groups.
    StoreFull,
}

/// Why [`ComAmazonawsMedialiveMedialiveState::delete_channel_placement_group`]
/// accepted no delete. Each variant leaves the store exactly as it was.
#[derive(Debug)]
pub enum ChannelPlacementGroupDeleteRefusal {
    /// The `ClusterId`/`ChannelPlacementGroupId` pair the request's path carried
    /// names no group in this store.
    NotFound,
    /// Channels are still placed in the group, so it is in use: deleting it
    /// would leave those channels naming a group the account no longer has.
    InUse,
}

/// The message a group delete refused because channels are still placed in the
/// group carries.
///
/// It names the identifier the CALLER supplied and the state it must reach —
/// the channels holding the group are state the caller did not name (they are
/// what the group's own `Channels` reports), so they are not interpolated, for
/// the same reason [`super::busy_input_message`] leaves the attaching channels
/// out of an input's refusal.
pub fn channel_placement_group_in_use_message(group_id: &str) -> String {
    format!(
        "ChannelPlacementGroup {group_id} still has channels placed in it, \
         it cannot be deleted"
    )
}

/// Why
/// [`ComAmazonawsMedialiveMedialiveState::apply_channel_placement_group_changes`]
/// changed nothing. Each variant is one refusal the update answers with, and
/// each leaves the stored group exactly as it was.
#[derive(Debug)]
pub enum ChannelPlacementGroupUpdateRefusal {
    /// The (`ClusterId`, `ChannelPlacementGroupId`) pair names no group in this
    /// store — the same resolution a read performs, so a group of another
    /// cluster is not found here either.
    NoSuchGroup,
    /// A delete has already been accepted for the group the pair names, so that
    /// id names no resource a caller may still change (the rule every mutation
    /// of this service follows).
    AlreadyDeleted,
    /// The request named more node ids than
    /// [`MAX_CHANNEL_PLACEMENT_GROUP_NODES`]. Carries how many it named.
    TooManyNodes { count: usize },
    /// A node id the request named is not a node of this cluster that a group
    /// can be associated with. Carries that id, so the refusal can name it.
    UnassociableNode { node_id: String },
    /// Another group of the same cluster already carries the new `Name`.
    NameTaken,
}

/// One created channel placement group, as `CreateChannelPlacementGroup` stored
/// it and `UpdateChannelPlacementGroup` has changed it since.
///
/// `Channels` is NOT a member: it is derived from the channel store on every
/// read (see
/// [`ComAmazonawsMedialiveMedialiveState::channels_in_channel_placement_group`]),
/// so a channel created or deleted after this group changes what the group
/// reports. `State` is private, because the only ways into it are the create
/// below, the update's [`ChannelPlacementGroupRecord::begin_assignment`] and the
/// clock (this record's own `settle`).
#[derive(Debug, Clone)]
pub struct ChannelPlacementGroupRecord {
    pub id: String,
    pub arn: String,
    /// The cluster this group belongs to, from the creating request's
    /// `ClusterId` path label. A group is only reachable through this id.
    pub cluster_id: String,
    /// The `Name` the group carries: the one the create supplied, or the one the
    /// latest update changed it to. A group created without one has none,
    /// rather than an empty one.
    pub name: Option<String>,
    /// The node ids the group is associated with — at most
    /// [`MAX_CHANNEL_PLACEMENT_GROUP_NODES`] of them.
    pub nodes: Vec<String>,
    /// The request's `Tags`. No response shape of this resource carries a
    /// `Tags` member, so the tag operations are what report them: they reach
    /// this map through [`ComAmazonawsMedialiveMedialiveState::resource_tags`].
    pub tags: HashMap<String, String>,
    /// The state the group is in NOW, one of
    /// [`channel_placement_group_state`]'s values.
    state: String,
    /// The transition the clock owes this group — the association still going
    /// through, or the deletion that has been accepted. STORED, never slept on;
    /// `None` once nothing is owed.
    pending: Option<PendingAssignment>,
}

impl ChannelPlacementGroupRecord {
    /// The state this group is in, as the response's `State` reports it.
    pub fn state(&self) -> &str {
        &self.state
    }

    /// Whether a delete has already been accepted for this group — it is in the
    /// transient `DELETING` state the clock owes the terminal one, or in that
    /// terminal `DELETED` state itself. The notion a MUTATION of the group
    /// resolves against, exactly as [`super::ChannelRecord::is_delete_accepted`]
    /// does for a channel.
    pub fn is_delete_accepted(&self) -> bool {
        self.state == channel_placement_group_state::DELETING
            || self.is_deleted()
    }

    /// Whether this group's delete has come due, which is what puts it in the
    /// terminal `DELETED` state it then keeps — and what takes it out of the
    /// cluster it was created in
    /// ([`ComAmazonawsMedialiveMedialiveState::resources_in_cluster`]), since a
    /// group in that state is no longer a resource the cluster holds.
    pub fn is_deleted(&self) -> bool {
        self.state == channel_placement_group_state::DELETED
    }

    /// Materialise the transition the clock has already made due as of `now`,
    /// answering whether THIS call is the one that settled the deletion.
    ///
    /// A group created with a node is `ASSIGNING` and becomes `ASSIGNED` once
    /// [`TRANSIENT_STATE_SPAN`] has passed, and a group whose delete has been
    /// accepted is `DELETING` and becomes the terminal `DELETED` the same span
    /// after the call that accepted it — whether or not anybody called, so
    /// running this on a read never changes what a later caller would otherwise
    /// have seen. An update that changed the node set owes the same kind of
    /// step, into whichever state the new set implies.
    fn settle(&mut self, now: SystemTime) -> bool {
        let Some(pending) = self.pending else {
            return false;
        };
        if now < pending.due {
            return false;
        }
        self.state = pending.settled.to_string();
        self.pending = None;
        self.is_deleted()
    }

    /// Accept a delete for this group: it enters the transient `DELETING` state
    /// and the clock owes it the terminal `DELETED` one
    /// [`TRANSIENT_STATE_SPAN`] later.
    ///
    /// IDEMPOTENT — a group a delete has already been accepted for keeps the
    /// instant the first call set, so a repeat can neither restart the clock nor
    /// revive a deleted group.
    ///
    /// An association still in flight is REPLACED rather than carried: the group
    /// is leaving the account, so nothing is going to finish associating it with
    /// a node, and the one transition the clock still owes it is the deletion.
    fn begin_delete(&mut self, now: SystemTime) {
        if self.is_delete_accepted() {
            return;
        }
        self.state = channel_placement_group_state::DELETING.to_string();
        self.pending = Some(PendingAssignment {
            due: now + TRANSIENT_STATE_SPAN,
            settled: channel_placement_group_state::DELETED,
        });
    }

    /// Start the association the clock owes a group whose node set has just
    /// become [`Self::nodes`]: the group reports the transient state now and
    /// settles into the one that node set implies once
    /// [`TRANSIENT_STATE_SPAN`] has passed.
    ///
    /// One of the two writers of [`Self::state`] after the insert (the other
    /// being [`Self::begin_delete`]), so the state a group is in can never
    /// disagree with the node set it was computed from.
    fn begin_assignment(&mut self, now: SystemTime) {
        let (transient, settled) = assignment_states(&self.nodes);
        self.state = transient.to_string();
        self.pending = Some(PendingAssignment {
            due: now + TRANSIENT_STATE_SPAN,
            settled,
        });
    }
}

/// A transition the clock owes a group: the instant it comes due and the
/// `ChannelPlacementGroupState` it settles into — the association a create or an
/// update started, or the deletion a delete accepted. Stored, never slept on —
/// the same shape as the cluster and multiplex slices' own, for the same reason.
#[derive(Debug, Clone, Copy)]
struct PendingAssignment {
    due: SystemTime,
    settled: &'static str,
}

/// The state a group whose node set has just become `nodes` is in right now, and
/// the one the clock then owes it.
///
/// Associating a node, and taking one away, is work the real service performs on
/// the Nodes, so it is asynchronous and the caller waits on `State`: a group
/// that now names a node is `ASSIGNING` until `ASSIGNED` comes due, and a group
/// whose nodes have just been emptied is `UNASSIGNING` until `UNASSIGNED` does.
fn assignment_states(nodes: &[String]) -> (&'static str, &'static str) {
    if nodes.is_empty() {
        (
            channel_placement_group_state::UNASSIGNING,
            channel_placement_group_state::UNASSIGNED,
        )
    } else {
        (
            channel_placement_group_state::ASSIGNING,
            channel_placement_group_state::ASSIGNED,
        )
    }
}

/// What an `UpdateChannelPlacementGroup` request asks to change.
///
/// Each member is `Some` only when the request carried it — "Include this
/// parameter only if you want to change the current ..." on both of them — so an
/// omitted member leaves that setting exactly as it was. One struct rather than
/// two positional `Option`s on
/// [`ComAmazonawsMedialiveMedialiveState::apply_channel_placement_group_changes`],
/// so a handler cannot pass the name where the nodes belong.
#[derive(Debug, Clone, Default)]
pub struct ChannelPlacementGroupChanges {
    pub name: Option<String>,
    pub nodes: Option<Vec<String>>,
}

/// One channel placement group as the response shapes carry it: the stored
/// record plus the two members derived from the rest of the state.
///
/// Every operation that reports a group (`CreateChannelPlacementGroup`,
/// `DescribeChannelPlacementGroup`, `UpdateChannelPlacementGroup` and
/// `DeleteChannelPlacementGroup`) is modelled with its OWN flat output shape
/// carrying the same seven members, so the projection is built once here and the
/// `channel_placement_group_output` macro below moves it onto whichever shape
/// the operation declares.
#[derive(Debug, Clone)]
pub struct ChannelPlacementGroupDescription {
    pub arn: Option<String>,
    pub channels: Option<Vec<String>>,
    pub cluster_id: Option<String>,
    pub id: Option<String>,
    pub name: Option<String>,
    pub nodes: Option<Vec<String>>,
    pub state: Option<String>,
}

impl ComAmazonawsMedialiveMedialiveState {
    /// Materialise every channel placement group transition the clock had
    /// already made due as of `now`. Called from
    /// [`ComAmazonawsMedialiveMedialiveState::settle`], beside the other
    /// resources' clock-owed work.
    pub(super) fn settle_channel_placement_groups(&mut self, now: SystemTime) {
        // Only a group that settles into the terminal `DELETED` on THIS call is
        // collected, so the work here does not grow with the number of groups
        // that have already reached that state.
        let mut just_deleted: Vec<String> = Vec::new();
        for (id, group) in &mut self.channel_placement_groups {
            if group.settle(now) {
                just_deleted.push(id.clone());
            }
        }
        // A token never outlives the group it minted: once that group has
        // reached the terminal `DELETED` state its idempotency window is over,
        // so a retried create mints a new group rather than answering with a
        // deleted one — the rule the cluster, channel and network tokens follow
        // too.
        for id in just_deleted {
            self.channel_placement_group_tokens.forget(&id);
        }
    }

    /// The next channel placement group id: seven decimal digits, for the
    /// reasons on [`FIRST_CHANNEL_PLACEMENT_GROUP_ID`].
    fn mint_channel_placement_group_id(&mut self) -> String {
        (FIRST_CHANNEL_PLACEMENT_GROUP_ID
            + self.mints.next(|mints| &mut mints.channel_placement_group))
            .to_string()
    }

    /// Store a channel placement group in `cluster_id` and answer its minted
    /// id, or refuse.
    ///
    /// The documented node bound, the name-uniqueness rule, the store's cap, the
    /// mint and the insert live together here, so a handler cannot check one of
    /// them and write past another; the handler's job is only to turn a
    /// [`ChannelPlacementGroupRefusal`] into the operation's own modelled error.
    ///
    /// The cluster has to EXIST: a group is created inside one Cluster ("The
    /// Node and the ChannelPlacementGroup must be in the same Cluster"), so a
    /// `ClusterId` this store holds no cluster for is refused rather than
    /// recorded, which is what keeps every stored group reachable through a real
    /// cluster.
    ///
    /// Each node id the request carries is RESOLVED against that cluster's nodes
    /// ([`ComAmazonawsMedialiveMedialiveState::first_unassociable_node`]) rather than
    /// recorded verbatim: `Nodes` names "the Node that you want to associate
    /// with the ChannelPlacementGroup", so an id that names none is a request
    /// there is nothing to carry out for. Refusing it is what keeps a group's
    /// `Nodes` and a node's own `ChannelPlacementGroups` two views of ONE
    /// association rather than two lists that can disagree.
    ///
    /// A group created with a node starts in the transient `ASSIGNING` state and
    /// the clock owes it `ASSIGNED`; a group created with no node is
    /// `UNASSIGNED`, which is what it is — there is no association in flight, so
    /// the clock owes it nothing.
    pub fn insert_channel_placement_group(
        &mut self,
        region: &str,
        cluster_id: &str,
        name: Option<String>,
        nodes: Vec<String>,
        tags: HashMap<String, String>,
        now: SystemTime,
    ) -> Result<String, ChannelPlacementGroupRefusal> {
        // Resolved through the LIVE cluster: a cluster whose delete has been
        // accepted is on its way out of the account, so no new group is created
        // in it even though its record stays readable.
        if self.live_cluster(cluster_id).is_none() {
            return Err(ChannelPlacementGroupRefusal::NoSuchCluster);
        }
        // The store's own ceiling is checked before anything about the
        // request's content, so which refusal a full store answers with does
        // not depend on the name or the nodes that request happened to carry.
        if self.channel_placement_groups.len() >= MAX_CHANNEL_PLACEMENT_GROUPS {
            return Err(ChannelPlacementGroupRefusal::StoreFull);
        }
        if nodes.len() > MAX_CHANNEL_PLACEMENT_GROUP_NODES {
            return Err(ChannelPlacementGroupRefusal::TooManyNodes {
                count: nodes.len(),
            });
        }
        // A node id is a REFERENCE to a node of this cluster, so it is resolved
        // before anything is written: a group that named a node the cluster does
        // not have would claim an association with nothing.
        if let Some(node_id) = self.first_unassociable_node(cluster_id, &nodes) {
            return Err(ChannelPlacementGroupRefusal::UnassociableNode { node_id });
        }
        if let Some(name) = name.as_deref() {
            if self
                .channel_placement_group_named(cluster_id, name)
                .is_some()
            {
                return Err(ChannelPlacementGroupRefusal::NameTaken);
            }
        }

        let id = self.mint_channel_placement_group_id();
        // `Id` is "the resource-id portion of the ARN", so the ARN is built from
        // the region this request was signed for and the minted id, and the id
        // is the tail of that ARN.
        let arn = ::engine::emulator_arn(
            "medialive",
            region,
            &format!("channelPlacementGroup:{id}"),
        );
        // A group created with no node has nothing in flight, so it starts in
        // the settled `UNASSIGNED` with nothing owed; one created with a node
        // starts the association, whose two states come from the one mapping
        // from node set to state ([`assignment_states`]).
        let (state, pending) = if nodes.is_empty() {
            (channel_placement_group_state::UNASSIGNED, None)
        } else {
            let (transient, settled) = assignment_states(&nodes);
            (
                transient,
                Some(PendingAssignment {
                    due: now + TRANSIENT_STATE_SPAN,
                    settled,
                }),
            )
        };
        self.channel_placement_groups.insert(
            id.clone(),
            ChannelPlacementGroupRecord {
                id: id.clone(),
                arn,
                cluster_id: cluster_id.to_string(),
                name,
                nodes,
                tags,
                state: state.to_string(),
                pending,
            },
        );
        Ok(id)
    }

    /// The first id in `nodes` that names no node of `cluster_id` a channel
    /// placement group can be associated with, or `None` when every id resolves.
    ///
    /// `Nodes` names the Node a group is associated with, and "The Node and the
    /// ChannelPlacementGroup must be in the same Cluster", so an id is resolved
    /// against THIS cluster's nodes — a node of another cluster resolves no
    /// better than one the account never created. A node whose deregistration
    /// has been accepted is on its way out of the account, so it is nothing a
    /// new association can be made with either: it is the same notion of "a node
    /// a write may still name" the tag writers resolve
    /// ([`super::NodeRecord::is_delete_accepted`]).
    ///
    /// The ONE resolver, so the create and the update cannot disagree about
    /// which node ids may reach a stored group.
    fn first_unassociable_node(&self, cluster_id: &str, nodes: &[String]) -> Option<String> {
        nodes
            .iter()
            .find(|node_id| {
                self.node(cluster_id, node_id)
                    .is_none_or(|node| node.is_delete_accepted())
            })
            .cloned()
    }

    /// The group `id` names in `cluster_id`, or `None` when that cluster holds
    /// no group with that id.
    ///
    /// Both path labels are part of the resource's name, so a group is not
    /// visible through another cluster's id.
    pub fn channel_placement_group(
        &self,
        cluster_id: &str,
        id: &str,
    ) -> Option<&ChannelPlacementGroupRecord> {
        self.channel_placement_groups
            .get(id)
            .filter(|group| group.cluster_id == cluster_id)
    }

    /// The same group, to write through — the one place a group is resolved
    /// mutably, so the "both path labels have to match" rule of
    /// [`Self::channel_placement_group`] is stated once however many mutations
    /// this slice grows, exactly as the node slice's own `node_mut` does.
    fn channel_placement_group_mut(
        &mut self,
        cluster_id: &str,
        id: &str,
    ) -> Option<&mut ChannelPlacementGroupRecord> {
        self.channel_placement_groups
            .get_mut(id)
            .filter(|group| group.cluster_id == cluster_id)
    }

    /// The group called `name` in `cluster_id`, if one exists. `Name` is
    /// documented "Specify a name that is unique in the Cluster. You can't
    /// change the name. Names are case-sensitive", so at most one group answers
    /// and the comparison is case-sensitive.
    ///
    /// A group whose delete has been accepted reserves its name no longer: it
    /// is on its way out of the account, so the name it carried is free for a
    /// new group, the same rule [`super::nodes`]' own derivation applies to a
    /// group that is leaving. Its record stays readable, under that name.
    ///
    /// The uniqueness is what both the create and the UPDATE resolve against:
    /// `UpdateChannelPlacementGroup`'s own `Name` is documented "Include this
    /// parameter only if you want to change the current name of the
    /// ChannelPlacementGroup", so a rename IS possible and this lookup is what
    /// tells it apart from a clash with another group of the same cluster (see
    /// [`ComAmazonawsMedialiveMedialiveState::apply_channel_placement_group_changes`]).
    fn channel_placement_group_named(
        &self,
        cluster_id: &str,
        name: &str,
    ) -> Option<&ChannelPlacementGroupRecord> {
        self.channel_placement_groups.values().find(|group| {
            group.cluster_id == cluster_id
                && !group.is_delete_accepted()
                && group.name.as_deref() == Some(name)
        })
    }

    /// Apply an `UpdateChannelPlacementGroup` request's changes to the group the
    /// pair names, or refuse.
    ///
    /// Every refusal is checked BEFORE the first write, so a refused update
    /// leaves the stored group exactly as it was. A member the request omitted
    /// is not changed ("Include this parameter only if you want to change the
    /// current ..."), and a node set it carried that the group already holds is
    /// not a change either — so re-sending the stored nodes leaves the group in
    /// the state it had reached instead of starting a fresh association.
    ///
    /// The documented in-cluster name uniqueness and node bound are the create's
    /// own rules, checked here too so the two cannot disagree about what a
    /// stored group may look like.
    pub fn apply_channel_placement_group_changes(
        &mut self,
        cluster_id: &str,
        id: &str,
        changes: ChannelPlacementGroupChanges,
        now: SystemTime,
    ) -> Result<(), ChannelPlacementGroupUpdateRefusal> {
        match self.channel_placement_group(cluster_id, id) {
            None => return Err(ChannelPlacementGroupUpdateRefusal::NoSuchGroup),
            Some(group) if group.is_delete_accepted() => {
                return Err(ChannelPlacementGroupUpdateRefusal::AlreadyDeleted)
            }
            Some(_) => {}
        }
        if let Some(nodes) = changes.nodes.as_deref() {
            if nodes.len() > MAX_CHANNEL_PLACEMENT_GROUP_NODES {
                return Err(ChannelPlacementGroupUpdateRefusal::TooManyNodes {
                    count: nodes.len(),
                });
            }
            // The same resolution the create performs, for the same reason: the
            // two operations write the same member of the same record, so a node
            // id reaches it only if it names a node of this cluster.
            if let Some(node_id) = self.first_unassociable_node(cluster_id, nodes) {
                return Err(ChannelPlacementGroupUpdateRefusal::UnassociableNode { node_id });
            }
        }
        // A name another group of this cluster holds is refused, while
        // re-sending the group's own name is not a collision with itself.
        if let Some(name) = changes.name.as_deref() {
            if self
                .channel_placement_group_named(cluster_id, name)
                .is_some_and(|holder| holder.id != id)
            {
                return Err(ChannelPlacementGroupUpdateRefusal::NameTaken);
            }
        }

        let group = self
            .channel_placement_groups
            .get_mut(id)
            .ok_or(ChannelPlacementGroupUpdateRefusal::NoSuchGroup)?;
        if let Some(name) = changes.name {
            group.name = Some(name);
        }
        if let Some(nodes) = changes.nodes {
            if nodes != group.nodes {
                group.nodes = nodes;
                // The node set really changed, so the association the real
                // service makes on the Nodes starts over: the group reports the
                // transient state and settles into the one the new set implies.
                group.begin_assignment(now);
            }
        }
        Ok(())
    }

    /// The ids of the channels currently placed in the group `group_id` names in
    /// `cluster_id` — a channel names both the cluster it runs on and the group
    /// it is placed in, through `AnywhereSettings$ClusterId` and
    /// `AnywhereSettings$ChannelPlacementGroupId`. Derived from the channel
    /// store, so it tracks every later create and delete rather than being
    /// recorded on the group.
    ///
    /// A channel whose delete this service has ACCEPTED is no longer placed
    /// anywhere, so it is not reported — the same rule
    /// [`ComAmazonawsMedialiveMedialiveState::channels_using_security_group`]
    /// follows.
    ///
    /// BOTH halves of the channel's own `AnywhereSettings` are matched, so the
    /// invariant is local to this derivation rather than resting on the mint
    /// counter making group ids unique store-wide: a channel is placed here only
    /// if it names THIS group and THIS cluster, exactly as
    /// [`ComAmazonawsMedialiveMedialiveState::channels_in_cluster`] matches the
    /// cluster half.
    pub fn channels_in_channel_placement_group(
        &self,
        cluster_id: &str,
        group_id: &str,
    ) -> Vec<String> {
        self.channels
            .values()
            .filter(|channel| !channel.is_delete_accepted())
            .filter(|channel| {
                let Some(settings) = channel.anywhere_settings.as_ref() else {
                    return false;
                };
                settings.channel_placement_group_id.as_deref() == Some(group_id)
                    && settings.cluster_id.as_deref() == Some(cluster_id)
            })
            .map(|channel| channel.id.clone())
            .collect()
    }

    /// The full description of the group `id` names in `cluster_id`, or `None`
    /// when that cluster holds no group with that id.
    ///
    /// Every member comes out of the store: the ones the create stored verbatim,
    /// and `Channels` and `State`, which are derived from the rest of the state
    /// (see [`ChannelPlacementGroupDescription`]).
    pub fn channel_placement_group_description(
        &self,
        cluster_id: &str,
        id: &str,
    ) -> Option<ChannelPlacementGroupDescription> {
        let group = self.channel_placement_group(cluster_id, id)?;
        Some(ChannelPlacementGroupDescription {
            arn: Some(group.arn.clone()),
            channels: Some(
                self.channels_in_channel_placement_group(&group.cluster_id, &group.id),
            ),
            cluster_id: Some(group.cluster_id.clone()),
            id: Some(group.id.clone()),
            name: group.name.clone(),
            nodes: Some(group.nodes.clone()),
            state: Some(group.state().to_string()),
        })
    }

    /// Accept a delete for the group `id` names in `cluster_id` and answer the
    /// group as the acceptance left it, or refuse.
    ///
    /// The precondition, the state write and the projection live together here,
    /// so the response reports the state that was stored rather than one a
    /// handler assembled: `State` is `DELETING` as of this call (or the terminal
    /// `DELETED` on a repeat whose clock has already run) and every other member
    /// is the stored record's.
    ///
    /// Both path labels name the resource, so an id that names no group of THIS
    /// cluster is [`ChannelPlacementGroupDeleteRefusal::NotFound`] exactly as it
    /// is for a read.
    ///
    /// A group that still holds channels is IN USE and refused: a channel placed
    /// in a group names it in `AnywhereSettings$ChannelPlacementGroupId`, so
    /// deleting the group under it would leave that channel naming a resource
    /// the account no longer has. The holders are derived from the channel store
    /// ([`Self::channels_in_channel_placement_group`]), so deleting the channels
    /// first is what clears the refusal.
    ///
    /// A delete already accepted is accepted again rather than refused: the
    /// state the caller is asking for is the one the group is already heading
    /// for or in, so nothing about the request conflicts with the stored group —
    /// the same answer [`Self::deregister_node_description`] gives a repeated
    /// node delete. The in-use precondition is therefore evaluated once, at the
    /// call that accepts the delete: from then on the group is leaving the
    /// account and a repeat cannot un-accept that, so what a later channel
    /// create does with its `AnywhereSettings` is that create's concern rather
    /// than this delete's.
    pub fn delete_channel_placement_group(
        &mut self,
        cluster_id: &str,
        id: &str,
        now: SystemTime,
    ) -> Result<ChannelPlacementGroupDescription, ChannelPlacementGroupDeleteRefusal> {
        // The holders are derived before the record is taken mutably, so the
        // precondition and the write are decided from one resolution of the
        // pair: an id that names no group derives no holders and the mutable
        // lookup below is what refuses it.
        let holders = self.channels_in_channel_placement_group(cluster_id, id);
        let group = self
            .channel_placement_group_mut(cluster_id, id)
            .ok_or(ChannelPlacementGroupDeleteRefusal::NotFound)?;
        if !group.is_delete_accepted() && !holders.is_empty() {
            return Err(ChannelPlacementGroupDeleteRefusal::InUse);
        }
        group.begin_delete(now);

        self.channel_placement_group_description(cluster_id, id)
            .ok_or(ChannelPlacementGroupDeleteRefusal::NotFound)
    }

    /// Record `token`, as presented for `cluster_id`, as having created the
    /// group `group_id`, evicting the oldest token first if that would push the
    /// map past [`MAX_CHANNEL_PLACEMENT_GROUP_TOKENS`].
    pub fn record_channel_placement_group_token(
        &mut self,
        cluster_id: &str,
        token: &str,
        group_id: &str,
    ) {
        self.channel_placement_group_tokens.record(
            channel_placement_group_token_key(cluster_id, token),
            group_id,
            MAX_CHANNEL_PLACEMENT_GROUP_TOKENS,
        );
    }

    /// The group a `RequestId` presented for `cluster_id` created, if that group
    /// is still one a caller may use.
    ///
    /// The cluster is part of the key AND re-checked on the record, so a token
    /// can only ever surface a group of the cluster the request named.
    ///
    /// LIVENESS is part of the lookup, through the same
    /// [`ChannelPlacementGroupRecord::is_delete_accepted`] a mutation of a group
    /// resolves against: a delete keeps the record readable, but a token's
    /// idempotency window is over once this service has accepted the delete of
    /// the group the token minted, so the retried create mints a new group
    /// rather than answering with one the caller could neither use nor change —
    /// a group whose name the slice has already freed for a new one. The token
    /// itself is dropped as soon as that delete comes due
    /// ([`Self::settle_channel_placement_groups`]), so the entry does not
    /// outlive the group either.
    pub fn channel_placement_group_for_token(
        &self,
        cluster_id: &str,
        request_id: &str,
    ) -> Option<&ChannelPlacementGroupRecord> {
        let id = self
            .channel_placement_group_tokens
            .created(&channel_placement_group_token_key(cluster_id, request_id))?;
        self.channel_placement_group(cluster_id, id)
            .filter(|group| !group.is_delete_accepted())
    }
}

/// The key the channel placement group token map uses.
///
/// A `RequestId` is idempotent for the resource collection it was presented to,
/// and a create's collection is one cluster's
/// (`POST /prod/clusters/{ClusterId}/channelplacementgroups`), so the cluster is
/// part of the key: two clusters using the same token each keep their own entry
/// instead of overwriting one another's. The cluster is re-checked on the record
/// the token resolves to, so the key's shape carries no invariant of its own.
/// (The monitor template resources key theirs by REGION for the same reason —
/// the parent module's `template_group_token_key` — which is why this is a
/// function of its own rather than that one under a misleading name.)
fn channel_placement_group_token_key(cluster_id: &str, token: &str) -> String {
    format!("{cluster_id}/{token}")
}

/// Project a [`ChannelPlacementGroupDescription`] onto one of the structurally
/// identical per-operation flat group output shapes
/// (`CreateChannelPlacementGroupOutput`,
/// `DescribeChannelPlacementGroupOutput`, `UpdateChannelPlacementGroupOutput`,
/// `DeleteChannelPlacementGroupOutput` and the listing's own
/// `DescribeChannelPlacementGroupSummary`).
///
/// One macro rather than a hand-written copy of the same seven-member move per
/// operation, so a create, an update, the listing and the read that follows any
/// of them cannot report the same stored group differently.
macro_rules! channel_placement_group_output {
    ($out:ident, $group:expr) => {{
        let group: $crate::state::ChannelPlacementGroupDescription = $group;
        $out {
            arn: group.arn,
            channels: group.channels,
            cluster_id: group.cluster_id,
            id: group.id,
            name: group.name,
            nodes: group.nodes,
            state: group.state,
        }
    }};
}

pub(crate) use channel_placement_group_output;
