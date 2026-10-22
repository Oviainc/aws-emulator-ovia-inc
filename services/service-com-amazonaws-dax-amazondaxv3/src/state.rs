// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-dax-amazondaxv3 service.
//!
//! This module owns the DAX control plane's persisted records (clusters, their nodes
//! and subnet groups) and the projections from a record to the modelled wire shape, so
//! every handler in `src/ops/` renders the SAME view of one stored resource instead of
//! building its own.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    Cluster, Endpoint, Node, NotificationConfiguration, ParameterGroup, ParameterGroupStatus,
    SSEDescription, SecurityGroupMembership, Subnet, SubnetGroup,
};

/// The region an ARN or an endpoint hostname carries when the request is unsigned and
/// therefore names no SigV4 credential scope.
pub const DEFAULT_REGION: &str = "us-east-1";

/// Statuses a DAX cluster reports. `Cluster$Status` is a free-form string in the model
/// (`smithy_query shape=Cluster`), and DAX words them in lower case.
pub const CLUSTER_CREATING: &str = "creating";
/// The one terminal, serving status: the only one from which a modification is accepted.
pub const CLUSTER_AVAILABLE: &str = "available";
/// Set by `UpdateCluster`/`IncreaseReplicationFactor` until the clock settles the change.
pub const CLUSTER_MODIFYING: &str = "modifying";
/// Set by `DeleteCluster` until the clock removes the cluster.
pub const CLUSTER_DELETING: &str = "deleting";

/// Statuses a node reports (`Node$NodeStatus`, "For example: `available`").
pub const NODE_CREATING: &str = "creating";
/// A node capable of serving requests — what `Cluster$ActiveNodes` counts.
pub const NODE_AVAILABLE: &str = "available";
/// A node of a cluster the clock is about to remove.
pub const NODE_DELETING: &str = "deleting";

/// `Node$ParameterGroupStatus` / `ParameterGroupStatus$ParameterApplyStatus`
/// ("For example, `in-sync`").
pub const PARAMETER_GROUP_IN_SYNC: &str = "in-sync";

/// `SecurityGroupMembership$Status` for a security group that is in effect on the nodes.
pub const SECURITY_GROUP_ACTIVE: &str = "active";

/// `NotificationConfiguration$TopicStatus` when a notification topic is attached and
/// the caller has not asked for another status.
pub const NOTIFICATION_ACTIVE: &str = "active";

/// `SSEDescription$Status` values, from the `SSEStatus` enum.
pub const SSE_ENABLED: &str = "ENABLED";
/// `SSEStatus` for a cluster created without `SSESpecification.Enabled`.
pub const SSE_DISABLED: &str = "DISABLED";

/// `ClusterEndpointEncryptionType` for an endpoint that speaks the plain `dax` protocol.
pub const ENCRYPTION_NONE: &str = "NONE";
/// `ClusterEndpointEncryptionType` for an endpoint that speaks `daxs`.
pub const ENCRYPTION_TLS: &str = "TLS";

/// The scheme and port the `Endpoint$URL` doc pins: "The default ports are 8111 for the
/// \"dax\" protocol and 9111 for the \"daxs\" protocol."
pub const DAX_SCHEME: &str = "dax";
/// Port for [`DAX_SCHEME`].
pub const DAX_PORT: i32 = 8111;
/// Scheme used when `ClusterEndpointEncryptionType` is `TLS`.
pub const DAXS_SCHEME: &str = "daxs";
/// Port for [`DAXS_SCHEME`].
pub const DAXS_PORT: i32 = 9111;

/// The network type the emulator's subnets support. Nothing in this emulator models an
/// IPv6 CIDR on a subnet, so a subnet group reports IPv4 only and a cluster created
/// without an explicit `NetworkType` derives `ipv4` from it — which is what the
/// `CreateCluster$NetworkType` doc prescribes ("If no explicit NetworkType is provided,
/// the network type is derived based on the subnet group's configuration").
pub const NETWORK_TYPE_IPV4: &str = "ipv4";

/// The maximum number of nodes a DAX cluster may hold, from the
/// `CreateCluster$ReplicationFactor` doc: "set ReplicationFactor to a number between 3
/// (one primary and two read replicas) and 10 (one primary and nine read replicas)".
pub const MAX_NODES_PER_CLUSTER: i32 = 10;

/// The `MaxResults` window both paginated DAX describes document: "The value for
/// `MaxResults` must be between 20 and 100."
pub const MIN_MAX_RESULTS: i32 = 20;
/// Upper bound of the documented `MaxResults` window, and the page size used when the
/// caller names none.
pub const MAX_MAX_RESULTS: i32 = 100;

/// The longest a `ClusterName` may be: "A name must contain from 1 to 20 alphanumeric
/// characters or hyphens" (`CreateCluster$ClusterName`).
pub const MAX_CLUSTER_NAME_LEN: usize = 20;

/// The number of clusters this service's store may hold, and with it the bound on how
/// far that store grows per request: a create past it is refused with the modelled
/// `ClusterQuotaForCustomerExceededFault` ("You have attempted to exceed the maximum
/// number of DAX clusters for your Amazon Web Services account"). Together with
/// [`MAX_NODES_PER_CLUSTER`] it caps the whole cluster store at a fixed number of
/// records, so no sequence of requests grows it without limit.
pub const MAX_CLUSTERS: usize = 50;

/// The number of subnet groups the store may hold; a create past it is refused with
/// `SubnetGroupQuotaExceededFault`.
pub const MAX_SUBNET_GROUPS: usize = 50;

/// The number of parameter groups the store may hold, counting the default group DAX
/// provides. Past it a create is refused with the modelled
/// `ParameterGroupQuotaExceededFault` ("You have attempted to exceed the maximum number
/// of parameter groups"), so no sequence of requests grows the store without limit. The
/// emulator picks the bound, as it does for [`MAX_CLUSTERS`] and [`MAX_SUBNET_GROUPS`].
pub const MAX_PARAMETER_GROUPS: usize = 50;

/// The parameter group every DAX account already has: `DescribeParameterGroups` lists it
/// before anything has been created, it is what a cluster created without an explicit
/// `ParameterGroupName` runs with, and `DescribeDefaultParameters` describes its
/// parameters. It is a RESERVED name — a create cannot take it and a delete cannot remove
/// it.
pub const DEFAULT_PARAMETER_GROUP: &str = "default.dax";

/// The description the default parameter group carries.
pub const DEFAULT_PARAMETER_GROUP_DESCRIPTION: &str = "Default parameter group.";

/// The number of subnets one group may name, so one request cannot store an
/// arbitrarily long identifier list that every page then re-renders. Past it the create
/// is refused with `SubnetQuotaExceededFault` ("The request cannot be processed because
/// it would exceed the allowed number of subnets in a subnet group").
pub const MAX_SUBNETS_PER_GROUP: usize = 20;

/// The number of tags one cluster may carry; past it the create is refused with
/// `TagQuotaPerResourceExceeded` ("You have exceeded the maximum number of tags for
/// this DAX cluster").
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// How long after the starting call a clock-owed cluster status move becomes due
/// (`creating`/`modifying` -> `available`, `deleting` -> gone). The emulator picks this
/// span; AWS takes minutes. It is deliberately non-zero so a caller that acts straight
/// after the call still observes the transient status and the state errors it causes.
pub const STATUS_SETTLE_DELAY: Duration = Duration::from_millis(500);

/// The Availability Zone suffixes nodes are spread over when neither the request nor the
/// cluster's subnet group names any. DAX "will spread the nodes across Availability
/// Zones for the highest availability" (`CreateCluster$AvailabilityZones`).
const AZ_SUFFIXES: [&str; 3] = ["a", "b", "c"];

/// A stored `Endpoint`: a DNS name, a port and the URL a DAX client dials.
#[derive(Debug, Clone)]
pub struct EndpointRecord {
    pub address: String,
    pub port: i32,
    pub url: String,
}

impl EndpointRecord {
    /// The endpoint for `address` on the protocol `encryption_type` selects.
    pub fn new(address: String, encryption_type: &str) -> Self {
        let (scheme, port) = if encryption_type == ENCRYPTION_TLS {
            (DAXS_SCHEME, DAXS_PORT)
        } else {
            (DAX_SCHEME, DAX_PORT)
        };
        // The URL carries no explicit port: the port is the scheme's documented default,
        // and `Endpoint$Port` reports it on its own.
        let url = format!("{scheme}://{address}");
        Self { address, port, url }
    }

    fn view(&self) -> Endpoint {
        Endpoint {
            address: Some(self.address.clone()),
            port: self.port,
            url: Some(self.url.clone()),
        }
    }
}

/// One node of a stored cluster.
#[derive(Debug, Clone)]
pub struct NodeRecord {
    pub node_id: String,
    pub endpoint: EndpointRecord,
    pub create_time: SystemTime,
    pub availability_zone: String,
    pub node_status: String,
    pub parameter_group_status: Option<String>,
}

impl NodeRecord {
    fn view(&self) -> Node {
        Node {
            node_id: Some(self.node_id.clone()),
            endpoint: Some(self.endpoint.view()),
            node_create_time: Some(to_date_time(self.create_time)),
            availability_zone: Some(self.availability_zone.clone()),
            node_status: Some(self.node_status.clone()),
            parameter_group_status: self.parameter_group_status.clone(),
        }
    }
}

/// A stored DAX cluster: everything `Cluster` reports, plus the bookkeeping the
/// lifecycle needs (the region its hostnames and ARN were minted in, the AZ pool new
/// nodes land in, and the instant the clock owes it its next status move).
#[derive(Debug, Clone)]
pub struct ClusterRecord {
    pub cluster_name: String,
    pub cluster_arn: String,
    pub description: Option<String>,
    pub node_type: String,
    pub status: String,
    pub nodes: Vec<NodeRecord>,
    pub discovery_endpoint: EndpointRecord,
    pub subnet_group: Option<String>,
    pub security_group_ids: Vec<String>,
    pub iam_role_arn: String,
    pub availability_zones: Vec<String>,
    pub preferred_maintenance_window: Option<String>,
    pub notification_topic_arn: Option<String>,
    pub notification_topic_status: Option<String>,
    pub parameter_group_name: Option<String>,
    pub sse_enabled: bool,
    pub cluster_endpoint_encryption_type: String,
    pub network_type: String,
    pub tags: Vec<(String, String)>,
    pub create_time: SystemTime,
    pub region: String,
    /// When set, the instant from which [`ComAmazonawsDaxAmazondaxv3State::settle_due`]
    /// owes this cluster its next status move.
    pub settles_at: Option<SystemTime>,
}

impl ClusterRecord {
    /// The modelled `Cluster` for this record. `TotalNodes` and `ActiveNodes` are
    /// DERIVED from the stored node list — `ActiveNodes` counts the nodes that are
    /// "capable of serving requests" (`Cluster$ActiveNodes`), so a cluster still coming
    /// up reports fewer active than total.
    pub fn view(&self) -> Cluster {
        let total = i32::try_from(self.nodes.len()).unwrap_or(i32::MAX);
        let active = i32::try_from(
            self.nodes
                .iter()
                .filter(|node| node.node_status == NODE_AVAILABLE)
                .count(),
        )
        .unwrap_or(i32::MAX);
        Cluster {
            cluster_name: Some(self.cluster_name.clone()),
            description: self.description.clone(),
            cluster_arn: Some(self.cluster_arn.clone()),
            total_nodes: Some(total),
            active_nodes: Some(active),
            node_type: Some(self.node_type.clone()),
            status: Some(self.status.clone()),
            cluster_discovery_endpoint: Some(self.discovery_endpoint.view()),
            // Only `DecreaseReplicationFactor` schedules node removals, and it is not
            // implemented, so no stored cluster ever has one pending.
            node_ids_to_remove: None,
            nodes: Some(self.nodes.iter().map(NodeRecord::view).collect()),
            preferred_maintenance_window: self.preferred_maintenance_window.clone(),
            notification_configuration: self.notification_topic_arn.as_ref().map(|arn| {
                NotificationConfiguration {
                    topic_arn: Some(arn.clone()),
                    topic_status: Some(
                        self.notification_topic_status
                            .clone()
                            .unwrap_or_else(|| NOTIFICATION_ACTIVE.to_string()),
                    ),
                }
            }),
            subnet_group: self.subnet_group.clone(),
            security_groups: Some(
                self.security_group_ids
                    .iter()
                    .map(|id| SecurityGroupMembership {
                        security_group_identifier: Some(id.clone()),
                        status: Some(SECURITY_GROUP_ACTIVE.to_string()),
                    })
                    .collect(),
            ),
            iam_role_arn: Some(self.iam_role_arn.clone()),
            parameter_group: self
                .parameter_group_name
                .as_ref()
                .map(|name| ParameterGroupStatus {
                    parameter_group_name: Some(name.clone()),
                    parameter_apply_status: Some(PARAMETER_GROUP_IN_SYNC.to_string()),
                    node_ids_to_reboot: Some(Vec::new()),
                }),
            sse_description: Some(SSEDescription {
                status: Some(
                    if self.sse_enabled {
                        SSE_ENABLED
                    } else {
                        SSE_DISABLED
                    }
                    .to_string(),
                ),
            }),
            cluster_endpoint_encryption_type: Some(self.cluster_endpoint_encryption_type.clone()),
            network_type: Some(self.network_type.clone()),
        }
    }

    /// The `Cluster` as `DescribeClusters` reports it. Its doc withholds node-level
    /// detail while the cluster or a node is not yet provisioned: "If the cluster is in
    /// the CREATING state, only cluster level information will be displayed until all of
    /// the nodes are successfully provisioned. If the cluster is in the DELETING state,
    /// only cluster level information will be displayed. If nodes are currently being
    /// added to the DAX cluster, node endpoint information and creation time for the
    /// additional nodes will not be displayed until they are completely provisioned."
    ///
    /// `TotalNodes`/`ActiveNodes` are cluster-level and stay reported throughout.
    pub fn describe_view(&self) -> Cluster {
        let mut view = self.view();
        if self.status == CLUSTER_CREATING || self.status == CLUSTER_DELETING {
            view.nodes = Some(Vec::new());
            return view;
        }
        if let Some(nodes) = view.nodes.as_mut() {
            for node in nodes
                .iter_mut()
                .filter(|node| node.node_status.as_deref() == Some(NODE_CREATING))
            {
                node.endpoint = None;
                node.node_create_time = None;
            }
        }
        view
    }

    /// The AZ a node at `index` of this cluster lands in: the cluster's AZ pool, cycled.
    pub fn availability_zone_for(&self, index: usize) -> String {
        if self.availability_zones.is_empty() {
            default_availability_zone(&self.region, index)
        } else {
            self.availability_zones[index % self.availability_zones.len()].clone()
        }
    }

    /// Mint one more node for this cluster, numbered `seq`, and append it.
    pub fn push_node(&mut self, seq: u64, now: SystemTime) {
        let index = self.nodes.len();
        let node_id = format!("{}-{seq}", self.cluster_name);
        let address = node_endpoint_address(&node_id, &self.cluster_name, &self.region);
        self.nodes.push(NodeRecord {
            node_id,
            endpoint: EndpointRecord::new(address, &self.cluster_endpoint_encryption_type),
            create_time: now,
            availability_zone: self.availability_zone_for(index),
            node_status: NODE_CREATING.to_string(),
            parameter_group_status: self
                .parameter_group_name
                .as_ref()
                .map(|_| PARAMETER_GROUP_IN_SYNC.to_string()),
        });
    }
}

/// A stored DAX parameter group: everything the modelled `ParameterGroup` reports, plus
/// whether DAX provides it (a default group, which cannot be deleted).
#[derive(Debug, Clone)]
pub struct ParameterGroupRecord {
    pub name: String,
    pub description: Option<String>,
    /// True for a group AWS provides rather than one a caller created.
    pub default_group: bool,
}

impl ParameterGroupRecord {
    /// The default group DAX provides in every account.
    pub fn default_group() -> Self {
        Self {
            name: DEFAULT_PARAMETER_GROUP.to_string(),
            description: Some(DEFAULT_PARAMETER_GROUP_DESCRIPTION.to_string()),
            default_group: true,
        }
    }

    /// The modelled `ParameterGroup` for this record.
    pub fn view(&self) -> ParameterGroup {
        ParameterGroup {
            parameter_group_name: Some(self.name.clone()),
            description: self.description.clone(),
        }
    }
}

/// A stored DAX subnet group.
#[derive(Debug, Clone)]
pub struct SubnetGroupRecord {
    pub name: String,
    pub description: Option<String>,
    pub vpc_id: String,
    pub subnet_ids: Vec<String>,
    pub region: String,
}

impl SubnetGroupRecord {
    /// The modelled `SubnetGroup` for this record. Each `Subnet` is derived from the
    /// stored identifier: its AZ is this emulator's placement for that identifier, and
    /// its supported network types are IPv4 (see [`NETWORK_TYPE_IPV4`]).
    pub fn view(&self) -> SubnetGroup {
        SubnetGroup {
            subnet_group_name: Some(self.name.clone()),
            description: self.description.clone(),
            vpc_id: Some(self.vpc_id.clone()),
            subnets: Some(
                self.subnet_ids
                    .iter()
                    .map(|id| Subnet {
                        subnet_identifier: Some(id.clone()),
                        subnet_availability_zone: Some(subnet_availability_zone(&self.region, id)),
                        supported_network_types: Some(vec![NETWORK_TYPE_IPV4.to_string()]),
                    })
                    .collect(),
            ),
            supported_network_types: Some(vec![NETWORK_TYPE_IPV4.to_string()]),
        }
    }

    /// The distinct Availability Zones this group's subnets sit in, sorted — the pool a
    /// cluster placed in this group spreads its nodes over.
    pub fn availability_zones(&self) -> Vec<String> {
        let mut zones: Vec<String> = self
            .subnet_ids
            .iter()
            .map(|id| subnet_availability_zone(&self.region, id))
            .collect();
        zones.sort_unstable();
        zones.dedup();
        zones
    }
}

/// Per-service state for com-amazonaws-dax-amazondaxv3. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsDaxAmazondaxv3State {
    /// Clusters by their (lower-cased) `ClusterName`. A `BTreeMap` so every describe
    /// pages in one stable order.
    pub clusters: BTreeMap<String, ClusterRecord>,
    /// Subnet groups by `SubnetGroupName`.
    pub subnet_groups: BTreeMap<String, SubnetGroupRecord>,
    /// Parameter groups by `ParameterGroupName`, seeded with the default group DAX
    /// provides (see [`DEFAULT_PARAMETER_GROUP`]). A `BTreeMap` so every describe pages
    /// in one stable order.
    pub parameter_groups: BTreeMap<String, ParameterGroupRecord>,
    /// Monotonic node counter, so a node id is unique and reproducible.
    pub next_node_seq: u64,
}

/// A fresh store is an account DAX has just become available in: no clusters and no
/// subnet groups, and the one parameter group AWS provides itself. Seeding it HERE rather
/// than from a handler is what keeps `DescribeParameterGroups` a pure read.
impl Default for ComAmazonawsDaxAmazondaxv3State {
    fn default() -> Self {
        let default_group = ParameterGroupRecord::default_group();
        Self {
            clusters: BTreeMap::new(),
            subnet_groups: BTreeMap::new(),
            parameter_groups: BTreeMap::from([(default_group.name.clone(), default_group)]),
            next_node_seq: 0,
        }
    }
}

impl ComAmazonawsDaxAmazondaxv3State {
    /// The next node sequence number.
    pub fn next_node_seq(&mut self) -> u64 {
        self.next_node_seq += 1;
        self.next_node_seq
    }

    /// Apply every cluster status move the clock has already made due as of `now`:
    /// `creating`/`modifying` becomes `available` (with its nodes), and a `deleting`
    /// cluster is removed. Nothing here depends on a call having been made — each move
    /// was owed at the same instant even if nobody had asked — so a reporting operation
    /// may run it before it reads.
    pub fn settle_due(&mut self, now: SystemTime) {
        let due: Vec<String> = self
            .clusters
            .iter()
            .filter(|(_, cluster)| cluster.settles_at.is_some_and(|at| at <= now))
            .map(|(name, _)| name.clone())
            .collect();
        for name in due {
            let deleting = self
                .clusters
                .get(&name)
                .is_some_and(|cluster| cluster.status == CLUSTER_DELETING);
            if deleting {
                self.clusters.remove(&name);
                continue;
            }
            if let Some(cluster) = self.clusters.get_mut(&name) {
                cluster.status = CLUSTER_AVAILABLE.to_string();
                for node in &mut cluster.nodes {
                    node.node_status = NODE_AVAILABLE.to_string();
                }
                cluster.settles_at = None;
            }
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDaxAmazondaxv3State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDaxAmazondaxv3State>)
}

/// [`service_state`] or the 501 this service answers when it is mounted on an engine
/// that holds no store at all.
///
/// Unreachable in this emulator — the router builds one `engine::InMemoryEngine` and
/// hands it to every operation — but a DAX model declares no server-side fault, so a
/// deployment with no store has genuinely implemented nothing to answer with.
pub fn store<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    operation: &str,
) -> Result<::engine::ServiceState<ComAmazonawsDaxAmazondaxv3State>, crate::scaffolded::Scaffolded<E>>
{
    service_state(engine).ok_or_else(|| crate::scaffolded::Scaffolded::not_implemented(operation))
}

/// The region this request was signed for, or [`DEFAULT_REGION`] when it carries no
/// SigV4 credential scope. Every ARN and endpoint hostname a cluster stores is minted
/// in it.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of a DAX cluster: `arn:aws:dax:<region>:<account>:cache/<ClusterName>`.
pub fn cluster_arn(region: &str, cluster_name: &str) -> String {
    ::engine::emulator_arn("dax", region, &format!("cache/{cluster_name}"))
}

/// The DNS name of a cluster's discovery endpoint.
pub fn discovery_endpoint_address(cluster_name: &str, region: &str) -> String {
    format!("{cluster_name}.dax-clusters.{region}.amazonaws.com")
}

/// The DNS name of one node's endpoint.
pub fn node_endpoint_address(node_id: &str, cluster_name: &str, region: &str) -> String {
    format!("{node_id}.{cluster_name}.dax-clusters.{region}.amazonaws.com")
}

/// The AZ a node at `index` lands in when nothing named a pool: DAX spreads nodes over
/// the region's zones.
pub fn default_availability_zone(region: &str, index: usize) -> String {
    format!("{region}{}", AZ_SUFFIXES[index % AZ_SUFFIXES.len()])
}

/// This emulator's placement of a subnet: a zone of the subnet's own region, chosen by
/// a digest of the subnet identifier, so the same identifier always reports the same AZ
/// however it is ordered in a request.
pub fn subnet_availability_zone(region: &str, subnet_id: &str) -> String {
    let digest = Sha256::digest(subnet_id.as_bytes());
    format!(
        "{region}{}",
        AZ_SUFFIXES[usize::from(digest[0]) % AZ_SUFFIXES.len()]
    )
}

/// The VPC a subnet group's subnets belong to. The emulator models no EC2 subnets, so
/// the identifier is DERIVED from the set of subnet identifiers (order-independent):
/// one set of subnets is always one VPC, and two different sets are different VPCs.
pub fn derived_vpc_id(subnet_ids: &[String]) -> String {
    let mut sorted: Vec<&str> = subnet_ids.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    sorted.dedup();
    let digest = Sha256::digest(sorted.join(",").as_bytes());
    format!("vpc-{}", hex::encode(&digest[..4]))
}

/// A `ClusterName` the `CreateCluster$ClusterName` doc accepts: "A name must contain
/// from 1 to 20 alphanumeric characters or hyphens. The first character must be a
/// letter. A name cannot end with a hyphen or contain two consecutive hyphens."
///
/// Returns the reason it is refused, worded for the caller, or `None` when it is valid.
pub fn cluster_name_violation(name: &str) -> Option<String> {
    if name.is_empty() || name.chars().count() > MAX_CLUSTER_NAME_LEN {
        return Some(format!(
            "ClusterName must contain from 1 to {MAX_CLUSTER_NAME_LEN} alphanumeric characters or hyphens"
        ));
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Some(
            "ClusterName must contain only alphanumeric characters or hyphens".to_string(),
        );
    }
    if !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return Some("ClusterName must begin with a letter".to_string());
    }
    if name.ends_with('-') {
        return Some("ClusterName cannot end with a hyphen".to_string());
    }
    if name.contains("--") {
        return Some("ClusterName cannot contain two consecutive hyphens".to_string());
    }
    None
}

/// The message DAX answers when `MaxResults` falls outside the documented window. DAX
/// words this rejection from a FIXED template that echoes nothing of the request, so it
/// is built from the documented bounds alone rather than from the value received.
pub fn max_results_out_of_range_message() -> String {
    format!(
        "Invalid value for max records. Must be between {MIN_MAX_RESULTS} and {MAX_MAX_RESULTS}"
    )
}

/// The message `ParameterGroupNotFoundFault` carries. Like DAX's other not-found faults
/// it is generic: the requested name is NOT echoed back.
pub fn parameter_group_not_found_message() -> String {
    "Parameter Group not found.".to_string()
}

/// The page of `names` to answer, and the token that continues it.
///
/// `start_from` is the name a continuation token resolved to (see [`decode_token`]); the
/// page begins at the first name at or after it. `max_results` is the caller's page
/// size, clamped into `1..=`[`MAX_MAX_RESULTS`] here as well as validated by the
/// handler, so this helper can never answer an unbounded page nor an empty one that
/// hands back the same token forever.
pub fn page<'a>(
    names: &'a [&'a str],
    max_results: Option<i32>,
    start_from: Option<&str>,
) -> (&'a [&'a str], Option<String>) {
    let start = match start_from {
        Some(from) => names.partition_point(|name| *name < from),
        None => 0,
    };
    let size = usize::try_from(max_results.unwrap_or(MAX_MAX_RESULTS).clamp(1, MAX_MAX_RESULTS))
        .unwrap_or(1);
    let rest = &names[start.min(names.len())..];
    let end = size.min(rest.len());
    let token = rest.get(end).map(::aws_smithy_types::base64::encode);
    (&rest[..end], token)
}

/// The name a continuation token produced by [`page`] resolves to; `None` when the
/// token is not one this service issued, which the handler refuses rather than
/// answering some arbitrary page with a 200.
pub fn decode_token(token: &str) -> Option<String> {
    ::aws_smithy_types::base64::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .filter(|name| !name.is_empty())
}

/// A stored instant as the modelled `TStamp`.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let secs = at
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or(0.0);
    ::aws_smithy_types::DateTime::from_secs_f64(secs)
}
