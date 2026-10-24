// SPDX-License-Identifier: CC0-1.0

//! The node slice of this service's state — the MediaLive Anywhere Node.
//!
//! One file for the whole slice — the record, its bounds, its refusals, the
//! store's methods and the output projection — so the slice can be read, and
//! merged, on its own, the same way the multiplex slice is laid out. The store
//! itself is the `nodes` member of [`ComAmazonawsMedialiveMedialiveState`] in
//! the parent module, beside the channels and inputs, so every operation that
//! reaches a node reads the one copy.
//!
//! A node is addressed by the CLUSTER it belongs to and its own id (every node
//! operation is bound under `/prod/clusters/{ClusterId}/nodes/...` and `Id` is
//! documented as "Unique in the Cluster"), so every lookup here takes both and a
//! node id presented under another cluster resolves to nothing. The cluster has
//! to be one this store HOLDS: `CreateNode` "Create a Node in the specified
//! Cluster", so [`ComAmazonawsMedialiveMedialiveState::insert_node`] resolves
//! the id against the cluster slice beside this one and refuses a cluster the
//! account does not have.
//!
//! Every value a node-shaped response carries is either stored verbatim from the
//! request that created the node or derived from the stored state
//! ([`ComAmazonawsMedialiveMedialiveState::node_summary`]) — no operation builds
//! a node out of its own request.
//!
//! A node's DELETION is a DEREGISTRATION: the `NodeState` enum models no
//! `DELETING`/`DELETED` pair, it models `DEREGISTERING` and the terminal
//! `DEREGISTERED`, which is what `DeleteNode`'s `202 Accepted` accepts and what
//! the clock then settles the node into ([`NodeRecord::settle`]). As everywhere
//! else in this service a delete never drops the record: the node stays readable
//! in its terminal state instead of vanishing. The instant is STORED and never
//! slept on — [`ComAmazonawsMedialiveMedialiveState::settle`], which every
//! operation runs first, is what materialises the transitions the clock has
//! already made due.

use std::collections::HashMap;
use std::time::SystemTime;

use crate::ops::types::{
    DescribeNodeSummary, NodeInterfaceMapping, NodeInterfaceMappingCreateRequest, SdiSourceMapping,
};

use super::{ComAmazonawsMedialiveMedialiveState, TRANSIENT_STATE_SPAN};

/// The `NodeState` values this slice uses, spelled exactly as the model declares
/// them (`smithy_query(query="shape", shape="NodeState")`: CREATED | REGISTERING
/// | READY_TO_ACTIVATE | REGISTRATION_FAILED | ACTIVATION_FAILED | ACTIVE |
/// READY | IN_USE | DEREGISTERING | DRAINING | DEREGISTRATION_FAILED |
/// DEREGISTERED).
///
/// The nine the model declares but this slice does not name are the ones nothing
/// here can reach. A node's registration, activation and encoding all happen on
/// node HARDWARE running the Elemental Anywhere software, which this emulator
/// does not run: nothing registers (`REGISTERING`, `READY_TO_ACTIVATE`,
/// `REGISTRATION_FAILED`), nothing activates (`ACTIVE`, `ACTIVATION_FAILED`,
/// `READY`), and no node ever encodes (`IN_USE`, `DRAINING`) — a channel placed
/// in a ChannelPlacementGroup that names a node is an ASSOCIATION, which the
/// node reports through its `ChannelPlacementGroups` and the group through its
/// own `State`, not work running on the node. `DEREGISTRATION_FAILED` reports a
/// failure of that same software, which this emulator never fabricates either.
pub mod node_state {
    /// Where every created node starts: `CreateNode` "Create a Node in the
    /// specified Cluster", and the node then has to run its registration script
    /// on the hardware that will host it before it is anything else.
    pub const CREATED: &str = "CREATED";
    /// What `DeleteNode` accepts a node into, and what the clock settles.
    pub const DEREGISTERING: &str = "DEREGISTERING";
    /// The terminal state a deregistered node keeps.
    pub const DEREGISTERED: &str = "DEREGISTERED";
}

/// The `NodeConnectionState` a node in this emulator reports
/// (`smithy_query(query="shape", shape="NodeConnectionState")`: CONNECTED |
/// DISCONNECTED).
///
/// A node is `CONNECTED` once the Elemental Anywhere software on its hardware has
/// connected to the service. This emulator runs no node hardware and accepts no
/// connection from one, so a node here is never connected, and reporting
/// `CONNECTED` would claim a running agent that does not exist.
pub const NODE_CONNECTION_STATE: &str = "DISCONNECTED";

/// The id the first node of a process gets.
///
/// MediaLive resource ids are numeric strings, so they are minted from a counter
/// at a fixed seven-digit width, which keeps the `BTreeMap` key order equal to
/// the mint order and makes a later `ListNodes` pagination deterministic. A range
/// of its own keeps the service's id spaces apart — the cluster slice's own
/// range starts one million below this one — so a caller (or a test) that passes
/// a cluster's or a channel's id where a node id belongs gets a
/// `NotFoundException` rather than a coincidental hit.
const FIRST_NODE_ID: u64 = 9_000_000;

/// The most nodes this emulator stores across every cluster.
///
/// A ceiling, not a guess at AWS's published quota: it exists so a create loop
/// cannot grow the emulator's heap without bound. `CreateNode` models
/// `TooManyRequestsException`, which is the refusal once the store is full.
pub const MAX_NODES: usize = 100;

/// The most `RequestId` entries kept for `CreateNode` idempotency — the same
/// bound, for the same reason, as [`super::MAX_IDEMPOTENCY_TOKENS`] on channels.
pub const MAX_NODE_TOKENS: usize = MAX_NODES;

/// The most `NodeInterfaceMappings` entries this emulator accepts on one node.
///
/// A ceiling, not a guess at AWS's published quota: the list is caller-chosen and
/// the model targets it at `__listOfNodeInterfaceMappingCreateRequest`, which
/// carries no `@length`, so the request boundary refuses no length and this is
/// where the bound lives — for the same reason
/// [`super::MAX_INPUT_ATTACHMENTS`] bounds a channel's attachments. `CreateNode`
/// models `BadRequestException`, which is the refusal for a request that is out
/// of bounds.
pub const MAX_NODE_INTERFACE_MAPPINGS: usize = 32;

/// Why [`ComAmazonawsMedialiveMedialiveState::insert_node`] stored nothing. Each
/// variant is one refusal `CreateNode` models, and each leaves the store exactly
/// as it was.
#[derive(Debug)]
pub enum NodeRefusal {
    /// The account holds no cluster with the id the create was addressed to, so
    /// there is no cluster to create the node in.
    ClusterNotFound,
    /// The request listed more interface mappings than
    /// [`MAX_NODE_INTERFACE_MAPPINGS`]. Carries the count it asked for, so the
    /// refusal can name it.
    TooManyInterfaceMappings(usize),
    /// The store already holds [`MAX_NODES`] nodes.
    StoreFull,
}

/// One created node, as `CreateNode` stored it plus its lifecycle state.
///
/// The members mirror the model's `DescribeNodeSummary` shape minus the ones it
/// derives or does not claim: `State` and `ConnectionState` are the lifecycle
/// below, and `InstanceArn` ("The ARN of the EC2 instance hosting the Node"),
/// `ManagedInstanceId` and a mapping's `PhysicalInterfaceIpAddresses` ("The IP
/// addresses associated with the physical interface on the node hardware") all
/// report hardware the real service registers and this emulator does not run, so
/// none of them is invented here.
#[derive(Debug, Clone)]
pub struct NodeRecord {
    pub id: String,
    pub arn: String,
    /// "The ID of the Cluster that the Node belongs to" — the cluster the create
    /// was addressed to. A node cannot move cluster ("Note that you can't move a
    /// Node to another Cluster"), so this is written once.
    pub cluster_id: String,
    pub name: Option<String>,
    /// The `NodeRole` the request named, stored verbatim: the model documents no
    /// default for it, so a create that names none leaves it unreported rather
    /// than having one chosen here.
    pub role: Option<String>,
    /// The interface mappings the create listed, in the order it listed them,
    /// projected onto the response shape once at create time.
    pub node_interface_mappings: Vec<NodeInterfaceMapping>,
    /// The node's SDI source mappings. Empty at create time — `CreateNode`
    /// models no member for them; `UpdateNode` is what sets them, and is its own
    /// issue.
    pub sdi_source_mappings: Vec<SdiSourceMapping>,
    /// The request's `Tags`. `CreateNode` accepts them but no node-shaped
    /// response carries a `Tags` member (`smithy_query(query="shape",
    /// shape="DescribeNodeSummary")`), so the tag operations are what report
    /// them: `ListTagsForResource` reads this map through
    /// [`ComAmazonawsMedialiveMedialiveState::resource_tags`].
    pub tags: HashMap<String, String>,
    /// The node's current `NodeState`.
    state: String,
    /// The transition the clock owes this node, while it is in a transient
    /// state; `None` once it has settled. One member rather than one instant per
    /// state, because a node is in at most one transient state at a time.
    transition: Option<NodeTransition>,
}

impl NodeRecord {
    /// Whether a delete has already been accepted for this node: it is
    /// deregistering, or has reached the terminal `DEREGISTERED`.
    ///
    /// This is what keeps a node whose delete was accepted READABLE but not
    /// writable: the tag writers resolve a node's ARN through
    /// `ComAmazonawsMedialiveMedialiveState::resource_tags_mut`, which does not
    /// hand out a node this answers `true` for, while the READ path goes on
    /// reporting the record in its terminal state. A mutation the node slice
    /// gains later (`UpdateNode`, `UpdateNodeState` \u2014 each its own issue)
    /// resolves its node the same way.
    pub fn is_delete_accepted(&self) -> bool {
        self.state == node_state::DEREGISTERING || self.state == node_state::DEREGISTERED
    }

    /// Accept a delete for this node: move it into `DEREGISTERING` and owe the
    /// terminal `DEREGISTERED` once [`TRANSIENT_STATE_SPAN`] has passed.
    ///
    /// A delete already accepted is left exactly as it is, so a repeated call can
    /// neither restart the clock nor revive a node the clock has already
    /// deregistered.
    fn begin_deregister(&mut self, now: SystemTime) {
        if self.is_delete_accepted() {
            return;
        }
        self.state = node_state::DEREGISTERING.to_string();
        self.transition = Some(NodeTransition {
            due: now + TRANSIENT_STATE_SPAN,
            to: node_state::DEREGISTERED,
        });
    }

    /// Materialise the transition the clock has already made due: a node
    /// `DeleteNode` accepted into `DEREGISTERING` becomes the terminal
    /// `DEREGISTERED` once the span has passed.
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

/// A transition the clock owes a node: the instant it becomes due and the
/// `NodeState` it settles into. Stored, never slept on — the same shape as the
/// channel and multiplex slices' own transitions, for the same reason.
#[derive(Debug, Clone, Copy)]
struct NodeTransition {
    due: SystemTime,
    to: &'static str,
}

impl ComAmazonawsMedialiveMedialiveState {
    /// Materialise every node transition the clock had already made due as of
    /// `now`. Called from [`ComAmazonawsMedialiveMedialiveState::settle`], which
    /// every operation runs first.
    pub(super) fn settle_nodes(&mut self, now: SystemTime) {
        for node in self.nodes.values_mut() {
            node.settle(now);
        }
    }

    /// The next node id, as AWS shapes MediaLive resource ids: a numeric string.
    fn mint_node_id(&mut self) -> String {
        let id = FIRST_NODE_ID + self.next_node_id;
        self.next_node_id += 1;
        id.to_string()
    }

    /// Store a node created in `region`, in the cluster `cluster_id`, and answer
    /// its minted id — or refuse.
    ///
    /// The cluster the node goes in, the bound on the caller-chosen mapping
    /// list, the store's cap, the mint and the insert live together here, so a
    /// handler cannot check one of them and write past another; the handler's job
    /// is only to turn a [`NodeRefusal`] into the operation's own modelled error.
    ///
    /// The cluster has to EXIST, because a node is created "in the specified
    /// Cluster". Which state that cluster is in is not a precondition: the real
    /// service accepts a node in a cluster that has not finished coming up — the
    /// node is what has to register before either of them encodes anything — so a
    /// `CREATING` cluster takes nodes exactly as an `ACTIVE` one does.
    ///
    /// A created node starts `CREATED` and owes the clock NOTHING: the real
    /// service answers `CreateNode` with a node in that state and it is the
    /// REGISTRATION SCRIPT run on the node's own hardware that moves it on from
    /// there. This emulator runs no node hardware, so a created node stays
    /// `CREATED` rather than being walked to a `READY` it never reached.
    pub fn insert_node(
        &mut self,
        region: &str,
        cluster_id: &str,
        name: Option<String>,
        role: Option<String>,
        interface_mappings: Vec<NodeInterfaceMappingCreateRequest>,
        tags: HashMap<String, String>,
    ) -> Result<String, NodeRefusal> {
        if !self.clusters.contains_key(cluster_id) {
            return Err(NodeRefusal::ClusterNotFound);
        }
        if interface_mappings.len() > MAX_NODE_INTERFACE_MAPPINGS {
            return Err(NodeRefusal::TooManyInterfaceMappings(
                interface_mappings.len(),
            ));
        }
        if self.nodes.len() >= MAX_NODES {
            return Err(NodeRefusal::StoreFull);
        }

        let id = self.mint_node_id();
        let arn = ::engine::emulator_arn("medialive", region, &format!("node:{id}"));
        self.nodes.insert(
            id.clone(),
            NodeRecord {
                id: id.clone(),
                arn,
                cluster_id: cluster_id.to_string(),
                name,
                role,
                node_interface_mappings: interface_mappings
                    .into_iter()
                    .map(described_interface_mapping)
                    .collect(),
                sdi_source_mappings: Vec::new(),
                tags,
                state: node_state::CREATED.to_string(),
                transition: None,
            },
        );
        Ok(id)
    }

    /// Record `token` as having created the node `node_id` in `cluster_id`,
    /// evicting the oldest token first if that would push the map past
    /// [`MAX_NODE_TOKENS`].
    pub fn record_node_token(&mut self, cluster_id: &str, token: String, node_id: &str) {
        self.node_tokens
            .record(node_token_key(cluster_id, &token), node_id, MAX_NODE_TOKENS);
    }

    /// The node a stored `RequestId` created in `cluster_id`, if that node still
    /// exists in it.
    ///
    /// The cluster is part of the key AND re-checked on the record: a token
    /// presented to one cluster's endpoint says nothing about another's.
    pub fn node_for_token(&self, cluster_id: &str, request_id: &str) -> Option<&str> {
        let id = self.node_tokens.created(&node_token_key(cluster_id, request_id))?;
        self.node(cluster_id, id).map(|node| node.id.as_str())
    }

    /// The node `node_id` of the cluster `cluster_id`, whatever state it is in.
    ///
    /// Both ids have to match: `Id` is "Unique in the Cluster", so a node id
    /// addressed under a cluster it does not belong to names no node at all.
    pub fn node(&self, cluster_id: &str, node_id: &str) -> Option<&NodeRecord> {
        self.nodes
            .get(node_id)
            .filter(|node| node.cluster_id == cluster_id)
    }

    /// The same node, to write through — the one place a node is resolved
    /// mutably, so the "both ids have to match" rule of [`Self::node`] is stated
    /// once however many mutations the slice grows.
    fn node_mut(&mut self, cluster_id: &str, node_id: &str) -> Option<&mut NodeRecord> {
        self.nodes
            .get_mut(node_id)
            .filter(|node| node.cluster_id == cluster_id)
    }

    /// The ids of the channel placement groups this node is associated with —
    /// "An array of IDs. Each ID is one ChannelPlacementGroup that is associated
    /// with this Node. Empty if the Node is not yet associated with any groups".
    ///
    /// DERIVED on every read from the channel-placement-group slice beside this
    /// one, never recorded on the node: `CreateChannelPlacementGroup` is what
    /// associates a group with a node (its `Nodes`), so a group created or
    /// deleted after this node changes what the node reports. A group whose
    /// delete has been accepted is left out: it is on its way out of the
    /// account, so it associates nothing. Only the node's OWN cluster is walked
    /// — "The Node and the ChannelPlacementGroup must be in the same Cluster".
    ///
    /// The walk is in group-id order, because the store is a `BTreeMap`, so two
    /// reads report the list the same way round.
    fn channel_placement_groups_with_node(&self, cluster_id: &str, node_id: &str) -> Vec<String> {
        self.channel_placement_groups
            .values()
            .filter(|group| {
                group.cluster_id == cluster_id
                    && !group.is_delete_accepted()
                    && group.nodes.iter().any(|node| node == node_id)
            })
            .map(|group| group.id.clone())
            .collect()
    }

    /// The `DescribeNodeSummary` projection of one stored node — the model's one
    /// node shape, which `CreateNode`, `DescribeNode`, `DeleteNode` and
    /// `ListNodes` all report the members of.
    ///
    /// Every member is read off the stored record or derived from it, so the
    /// create, the read that follows it and the delete cannot report the same
    /// stored node differently.
    pub fn node_summary(&self, node: &NodeRecord) -> DescribeNodeSummary {
        DescribeNodeSummary {
            arn: Some(node.arn.clone()),
            // "An array of IDs. Each ID is one ChannelPlacementGroup that is
            // associated with this Node. Empty if the Node is not yet associated
            // with any groups": derived from the groups this store holds
            // ([`Self::channel_placement_groups_with_node`]), so it is empty
            // exactly while no live group of the node's cluster names it.
            channel_placement_groups: Some(
                self.channel_placement_groups_with_node(&node.cluster_id, &node.id),
            ),
            cluster_id: Some(node.cluster_id.clone()),
            connection_state: Some(NODE_CONNECTION_STATE.to_string()),
            id: Some(node.id.clone()),
            // ABSENT, not empty: `InstanceArn` and `ManagedInstanceId` name the
            // EC2 instance and the managed instance the real service registers
            // for a node's hardware, which this emulator does not provision. A
            // value here would claim an instance that does not exist.
            instance_arn: None,
            managed_instance_id: None,
            name: node.name.clone(),
            node_interface_mappings: Some(node.node_interface_mappings.clone()),
            role: node.role.clone(),
            state: Some(node.state.clone()),
            sdi_source_mappings: Some(node.sdi_source_mappings.clone()),
        }
    }

    /// The `DescribeNodeSummary` of the node `node_id` in `cluster_id`, or `None`
    /// when the store holds no such node.
    pub fn node_description(&self, cluster_id: &str, node_id: &str) -> Option<DescribeNodeSummary> {
        let node = self.node(cluster_id, node_id)?;
        Some(self.node_summary(node))
    }

    /// Accept a delete for the node `node_id` of `cluster_id` and answer the node
    /// as the acceptance left it, or `None` when the store holds no such node.
    ///
    /// The state write and the projection live together here, so the response
    /// reports the state that was stored rather than one a handler assembled: its
    /// `State` is `DEREGISTERING` as of this call (or the terminal
    /// `DEREGISTERED` on a repeat whose clock has already run) and every other
    /// member is the stored record's.
    pub fn deregister_node_description(
        &mut self,
        cluster_id: &str,
        node_id: &str,
        now: SystemTime,
    ) -> Option<DescribeNodeSummary> {
        let node = self.node_mut(cluster_id, node_id)?;
        node.begin_deregister(now);
        self.node_description(cluster_id, node_id)
    }
}

/// One created node's interface mapping, projected from the CREATE REQUEST shape
/// onto the response shape.
///
/// The response shape carries one member the request does not —
/// `PhysicalInterfaceIpAddresses`, "The IP addresses associated with the physical
/// interface on the node hardware" — which the real service learns from the
/// hardware when the node registers. This emulator runs no node hardware, so the
/// member is ABSENT rather than an empty list that would claim an interface with
/// no addresses.
fn described_interface_mapping(mapping: NodeInterfaceMappingCreateRequest) -> NodeInterfaceMapping {
    NodeInterfaceMapping {
        logical_interface_name: mapping.logical_interface_name,
        network_interface_mode: mapping.network_interface_mode,
        physical_interface_name: mapping.physical_interface_name,
        physical_interface_ip_addresses: None,
    }
}

/// The key the node idempotency-token map records a token against.
///
/// A node is created against ONE cluster's endpoint (`POST
/// /prod/clusters/{ClusterId}/nodes`) and its id is unique in that cluster, so
/// the cluster is part of the key: two clusters using the same `RequestId` each
/// keep their own entry instead of overwriting one another's. The cluster is
/// re-checked on the record the token resolves to
/// ([`ComAmazonawsMedialiveMedialiveState::node_for_token`]), so the key's shape
/// carries no invariant of its own.
fn node_token_key(cluster_id: &str, token: &str) -> String {
    format!("{cluster_id}/{token}")
}

/// Project a `DescribeNodeSummary` onto one of the structurally identical
/// per-operation flat node output shapes (`CreateNodeOutput`,
/// `DescribeNodeOutput`, `DeleteNodeOutput`, and `UpdateNode`'s and
/// `UpdateNodeState`'s once those land).
///
/// One macro rather than a hand-written copy of the same member move per
/// operation, so a create and the read that follows it cannot report the same
/// stored node differently. `ManagedInstanceId` is the one member the summary
/// carries that the flat shapes do not model, so it is dropped here rather than
/// named in each of them.
macro_rules! node_description_output {
    ($out:ident, $node:expr) => {{
        let node: $crate::ops::types::DescribeNodeSummary = $node;
        $out {
            arn: node.arn,
            channel_placement_groups: node.channel_placement_groups,
            cluster_id: node.cluster_id,
            connection_state: node.connection_state,
            id: node.id,
            instance_arn: node.instance_arn,
            name: node.name,
            node_interface_mappings: node.node_interface_mappings,
            role: node.role,
            state: node.state,
            sdi_source_mappings: node.sdi_source_mappings,
        }
    }};
}

pub(crate) use node_description_output;
