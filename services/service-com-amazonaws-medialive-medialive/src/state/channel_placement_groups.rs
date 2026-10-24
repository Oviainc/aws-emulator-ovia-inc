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
    /// A group whose delete has been accepted; the clock owes [`DELETED`].
    /// Used by `DeleteChannelPlacementGroup` when its own issue lands; the
    /// read path already reports whatever state the record carries.
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
    /// A group in the same cluster already carries that name, which `Name` is
    /// documented to make unique there ("Specify a name that is unique in the
    /// Cluster").
    NameTaken,
    /// The store already holds [`MAX_CHANNEL_PLACEMENT_GROUPS`] groups.
    StoreFull,
}

/// One created channel placement group, as `CreateChannelPlacementGroup` stored
/// it.
///
/// `Channels` is NOT a member: it is derived from the channel store on every
/// read (see
/// [`ComAmazonawsMedialiveMedialiveState::channels_in_channel_placement_group`]),
/// so a channel created or deleted after this group changes what the group
/// reports. `State` is private, because the only ways into it are the create
/// below and the clock (this record's own `settle`).
#[derive(Debug, Clone)]
pub struct ChannelPlacementGroupRecord {
    pub id: String,
    pub arn: String,
    /// The cluster this group belongs to, from the creating request's
    /// `ClusterId` path label. A group is only reachable through this id.
    pub cluster_id: String,
    /// The `Name` the request supplied, which "You can't change". A group
    /// created without one has none, rather than an empty one.
    pub name: Option<String>,
    /// The node ids the request associated the group with — at most
    /// [`MAX_CHANNEL_PLACEMENT_GROUP_NODES`] of them.
    pub nodes: Vec<String>,
    /// The request's `Tags`. No response shape of this resource carries a
    /// `Tags` member, so the tag operations are what report them: they reach
    /// this map through [`ComAmazonawsMedialiveMedialiveState::resource_tags`].
    pub tags: HashMap<String, String>,
    /// The state the group is in NOW, one of
    /// [`channel_placement_group_state`]'s values.
    state: String,
    /// When the clock makes this group's `ASSIGNING` -> `ASSIGNED` step due.
    /// STORED, never slept on; `None` once nothing is owed.
    assignment_due: Option<SystemTime>,
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
            || self.state == channel_placement_group_state::DELETED
    }

    /// Materialise the transition the clock has already made due as of `now`.
    ///
    /// A group created with a node is `ASSIGNING` and becomes `ASSIGNED` once
    /// [`TRANSIENT_STATE_SPAN`] has passed, whether or not anybody called, so
    /// running this on a read never changes what a later caller would otherwise
    /// have seen.
    fn settle(&mut self, now: SystemTime) {
        let Some(due) = self.assignment_due else {
            return;
        };
        if now < due {
            return;
        }
        self.state = channel_placement_group_state::ASSIGNED.to_string();
        self.assignment_due = None;
    }
}

/// One channel placement group as the response shapes carry it: the stored
/// record plus the two members derived from the rest of the state.
///
/// Every operation that reports a group (`CreateChannelPlacementGroup`,
/// `DescribeChannelPlacementGroup`, and the update and delete when their own
/// issues land) is modelled with its OWN flat output shape carrying the same
/// seven members, so the projection is built once here and the
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
        for group in self.channel_placement_groups.values_mut() {
            group.settle(now);
        }
    }

    /// The next channel placement group id: seven decimal digits, for the
    /// reasons on [`FIRST_CHANNEL_PLACEMENT_GROUP_ID`].
    fn mint_channel_placement_group_id(&mut self) -> String {
        let id = FIRST_CHANNEL_PLACEMENT_GROUP_ID + self.next_channel_placement_group_id;
        self.next_channel_placement_group_id += 1;
        id.to_string()
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
        if !self.clusters.contains_key(cluster_id) {
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
        let (state, assignment_due) = if nodes.is_empty() {
            (channel_placement_group_state::UNASSIGNED, None)
        } else {
            (
                channel_placement_group_state::ASSIGNING,
                Some(now + TRANSIENT_STATE_SPAN),
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
                assignment_due,
            },
        );
        Ok(id)
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

    /// The group called `name` in `cluster_id`, if one exists. `Name` is
    /// documented "Specify a name that is unique in the Cluster. You can't
    /// change the name. Names are case-sensitive", so at most one group answers
    /// and the comparison is case-sensitive.
    fn channel_placement_group_named(
        &self,
        cluster_id: &str,
        name: &str,
    ) -> Option<&ChannelPlacementGroupRecord> {
        self.channel_placement_groups.values().find(|group| {
            group.cluster_id == cluster_id && group.name.as_deref() == Some(name)
        })
    }

    /// The ids of the channels currently placed in the group `group_id` names in
    /// `cluster_id` — a channel names both the cluster it runs on and the group
    /// it is placed in, through `AnywhereSettings$ClusterId` and
    /// `AnywhereSettings$ChannelPlacementGroupId`. Derived from the channel
    /// store, so it tracks every later create and delete rather than being
    /// recorded on the group.
    ///
    /// A channel that has reached the terminal `DELETED` state is no longer
    /// placed anywhere, so it is not reported — the same rule
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
            .filter(|channel| !channel.is_deleted())
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
    /// still exists in that cluster.
    ///
    /// The cluster is part of the key AND re-checked on the record, so a token
    /// can only ever surface a group of the cluster the request named.
    pub fn channel_placement_group_for_token(
        &self,
        cluster_id: &str,
        request_id: &str,
    ) -> Option<&ChannelPlacementGroupRecord> {
        let id = self
            .channel_placement_group_tokens
            .created(&channel_placement_group_token_key(cluster_id, request_id))?;
        self.channel_placement_group(cluster_id, id)
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
/// `DescribeChannelPlacementGroupOutput`, and the update's and delete's when
/// their own issues land).
///
/// One macro rather than a hand-written copy of the same seven-member move per
/// operation, so a create and the read that follows it cannot report the same
/// stored group differently.
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
