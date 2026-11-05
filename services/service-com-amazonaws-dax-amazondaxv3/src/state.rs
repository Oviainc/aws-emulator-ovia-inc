// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-dax-amazondaxv3 service.
//!
//! This module owns the DAX control plane's persisted records (clusters, their nodes
//! and subnet groups), the event log those records write to, and the projections from
//! a record to the modelled wire shape, so every handler in `src/ops/` renders the SAME
//! view of one stored resource instead of building its own. It also owns the message
//! each fault answers with, so one fault reads the same whatever raises it.
//!
//! It carries one thing DAX does not own: the Region's NETWORK, against which a
//! subnet group's identifiers are resolved ([`region_subnets`]). The emulator's EC2
//! service models no subnets yet; when it does, `resolve_subnet` is the one place that
//! should read them from it instead. FSx's `state.rs` applies the SAME derivation to
//! `CreateFileSystem$SubnetIds`, both written against the contract
//! `services/common/src/network.rs` states, so the two report one account network; the
//! derivation moves into that shared module once one change may carry both service
//! crates' manifests.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    Cluster, ClusterNotFoundFault, Endpoint, Event, InvalidClusterStateFault,
    InvalidParameterCombinationException, InvalidParameterValueException, Node,
    NotificationConfiguration, Parameter, ParameterGroup, ParameterGroupAlreadyExistsFault,
    ParameterGroupNotFoundFault, ParameterGroupStatus, SSEDescription, SecurityGroupMembership,
    Subnet, SubnetGroup, SubnetGroupNotFoundFault,
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

/// The parameter group family a service-provided parameter group belongs to: the caching
/// software and the version of it whose defaults the group carries. AWS names a group it
/// provides itself after the family rather than after the service alone, so the name a
/// client reads back and passes to `CreateCluster$ParameterGroupName` is version-qualified.
pub const DEFAULT_PARAMETER_GROUP_FAMILY: &str = "dax1.0";

/// The parameter group every DAX account already has: `DescribeParameterGroups` lists it
/// before anything has been created, it is what a cluster created without an explicit
/// `ParameterGroupName` runs with, and `DescribeDefaultParameters` describes its
/// parameters. It is a RESERVED name — a create cannot take it and a delete cannot remove
/// it.
///
/// The name is `default.` followed by [`DEFAULT_PARAMETER_GROUP_FAMILY`] (a `const` cannot
/// be assembled from another `const` string, so the two are kept in step by
/// `tests/all/dax_parameter_groups.rs`).
pub const DEFAULT_PARAMETER_GROUP: &str = "default.dax1.0";

/// The description the default parameter group carries: `Default parameter group for `
/// followed by [`DEFAULT_PARAMETER_GROUP_FAMILY`], the family whose defaults it holds.
pub const DEFAULT_PARAMETER_GROUP_DESCRIPTION: &str = "Default parameter group for dax1.0";

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

/// `Event$SourceType` for an event about a cluster. The `SourceType` enum lists
/// `CLUSTER | PARAMETER_GROUP | SUBNET_GROUP`
/// (`smithy_query(query="shape",shape="SourceType")`).
pub const SOURCE_TYPE_CLUSTER: &str = "CLUSTER";
/// `Event$SourceType` for an event about a parameter group.
pub const SOURCE_TYPE_PARAMETER_GROUP: &str = "PARAMETER_GROUP";
/// `Event$SourceType` for an event about a subnet group.
pub const SOURCE_TYPE_SUBNET_GROUP: &str = "SUBNET_GROUP";

/// Every value the `SourceType` enum lists, so a `DescribeEvents` filter outside the
/// enum is refused rather than quietly selecting nothing.
pub const SOURCE_TYPES: [&str; 3] = [
    SOURCE_TYPE_CLUSTER,
    SOURCE_TYPE_PARAMETER_GROUP,
    SOURCE_TYPE_SUBNET_GROUP,
];

/// The event a `CreateCluster` records against the cluster it minted.
///
/// AWS publishes no message template for a DAX event, so the wording of each of these
/// is the emulator's own; what a client can rely on is the event's source, its type
/// and the instant it was recorded at, which all come from the stored resource.
pub const EVENT_CLUSTER_CREATED: &str = "Cluster created.";
/// The event recorded when the clock settles a cluster into `available`.
pub const EVENT_CLUSTER_AVAILABLE: &str = "Cluster is available.";
/// The event an `UpdateCluster` records.
pub const EVENT_CLUSTER_MODIFIED: &str = "Cluster modified.";
/// The event an `IncreaseReplicationFactor` records.
pub const EVENT_CLUSTER_NODES_ADDED: &str = "Added nodes to the cluster.";
/// The event a `DeleteCluster` records when the deletion starts.
pub const EVENT_CLUSTER_DELETING: &str = "Deleting cluster.";
/// The event recorded when the clock completes a deletion.
pub const EVENT_CLUSTER_DELETED: &str = "Cluster deleted.";
/// The event a `CreateSubnetGroup` records against the group it minted.
pub const EVENT_SUBNET_GROUP_CREATED: &str = "Subnet group created.";
/// The event a `CreateParameterGroup` records against the group it minted.
pub const EVENT_PARAMETER_GROUP_CREATED: &str = "Parameter group created.";
/// The event a `DeleteParameterGroup` records.
pub const EVENT_PARAMETER_GROUP_DELETED: &str = "Parameter group deleted.";

/// The furthest back a `DescribeEvents` window may reach, in minutes: "you can
/// retrieve up to 14 days' worth of events if necessary" (`DescribeEvents`).
pub const MAX_EVENT_DURATION: i32 = 14 * 24 * 60;

/// [`MAX_EVENT_DURATION`] as a duration: the age at which a recorded event is no
/// longer available and is dropped from the store.
pub const EVENT_RETENTION: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// The window a `DescribeEvents` that names neither `Duration` nor `StartTime` reads:
/// "By default, only the events occurring within the last 24 hours are returned"
/// (`DescribeEvents`).
pub const DEFAULT_EVENT_DURATION: i32 = 24 * 60;

/// The number of events the store keeps (oldest dropped first), so no sequence of
/// requests grows the event log without limit.
pub const MAX_EVENTS: usize = 500;

/// How many hex characters an EC2 subnet identifier carries in its long form
/// (`subnet-0bb1c79de3example`), which is the form the network model below mints.
const SUBNET_ID_HEX_LEN: usize = 17;

/// A subnet of the account's network in one Region: the identifier a request names,
/// the VPC it belongs to and the Availability Zone it sits in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubnetRecord {
    /// The EC2 subnet identifier, as a request names it.
    pub subnet_id: String,
    /// The VPC this subnet belongs to.
    pub vpc_id: String,
    /// The Availability Zone this subnet sits in.
    pub availability_zone: String,
}

/// The id of the account's default VPC in `region`.
///
/// AWS gives an account a default VPC in every Region and this emulator models no
/// other, so the id is DERIVED from the Region name — stable, so every read reports
/// the same VPC — in EC2's short `vpc-<8 hex>` form.
pub fn default_vpc_id(region: &str) -> String {
    format!("vpc-{}", hex::encode(&Sha256::digest(region.as_bytes())[..4]))
}

/// The subnets that exist in `region`: the default VPC's default subnet in each
/// Availability Zone of that Region.
///
/// AWS creates the default VPC with "a size /16 IPv4 CIDR block and a default subnet
/// in each Availability Zone" (EC2's `CreateDefaultVpc`), so these subnets are facts
/// of the account's Region rather than resources a DAX request creates — the same
/// reasoning EC2's own state module gives for deriving a Region's Availability Zones
/// instead of storing them. Each identifier is derived from its zone name in EC2's
/// long `subnet-<17 hex>` form, so a Region always reports the same subnets and
/// [`resolve_subnet`] can tell an identifier that names one from an identifier that
/// names nothing at all.
pub fn region_subnets(region: &str) -> Vec<SubnetRecord> {
    let vpc_id = default_vpc_id(region);
    AZ_SUFFIXES
        .iter()
        .map(|suffix| {
            let availability_zone = format!("{region}{suffix}");
            let digest = hex::encode(Sha256::digest(availability_zone.as_bytes()));
            SubnetRecord {
                subnet_id: format!("subnet-{}", &digest[..SUBNET_ID_HEX_LEN]),
                vpc_id: vpc_id.clone(),
                availability_zone,
            }
        })
        .collect()
}

/// The subnet `subnet_id` names in `region`, or `None` when no subnet of that Region
/// carries that identifier — which is what lets a request naming it be refused
/// instead of stored.
pub fn resolve_subnet(region: &str, subnet_id: &str) -> Option<SubnetRecord> {
    region_subnets(region)
        .into_iter()
        .find(|subnet| subnet.subnet_id == subnet_id)
}

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
/// whether DAX provides it (a default group, which cannot be deleted) and the value this
/// group carries for each parameter of the caching software.
#[derive(Debug, Clone)]
pub struct ParameterGroupRecord {
    pub name: String,
    pub description: Option<String>,
    /// True for a group AWS provides rather than one a caller created.
    pub default_group: bool,
    /// This group's value for every parameter of the caching software, keyed by
    /// `Parameter$ParameterName`. A group is created holding the system-defined value of
    /// each parameter in [`DEFAULT_PARAMETERS`] (see [`default_parameter_values`]), so a
    /// read of a group reports the group's OWN stored values rather than the catalog's;
    /// a `BTreeMap` so those values page in one stable order.
    pub parameter_values: BTreeMap<String, String>,
}

impl ParameterGroupRecord {
    /// A group a caller created: it starts out carrying the system-defined value of
    /// every parameter the caching software defines, which is what a group that has
    /// never been updated reports.
    pub fn new(name: String, description: Option<String>) -> Self {
        Self {
            name,
            description,
            default_group: false,
            parameter_values: default_parameter_values(),
        }
    }

    /// The default group DAX provides in every account.
    pub fn default_group() -> Self {
        Self {
            default_group: true,
            ..Self::new(
                DEFAULT_PARAMETER_GROUP.to_string(),
                Some(DEFAULT_PARAMETER_GROUP_DESCRIPTION.to_string()),
            )
        }
    }

    /// The modelled `ParameterGroup` for this record.
    pub fn view(&self) -> ParameterGroup {
        ParameterGroup {
            parameter_group_name: Some(self.name.clone()),
            description: self.description.clone(),
        }
    }

    /// The names of the parameters this group holds a value for, in the one stable order
    /// they page in (by name).
    pub fn parameter_names(&self) -> Vec<&str> {
        self.parameter_values.keys().map(String::as_str).collect()
    }

    /// The modelled `Parameter` this group reports for `name`: the catalog entry's
    /// definition — its description, `Source`, data type, allowed values, modifiability
    /// and change type — carrying the value THIS group stores for it. `None` when the
    /// group holds no such parameter.
    pub fn parameter_view(&self, name: &str) -> Option<Parameter> {
        let definition = default_parameter(name)?;
        let value = self
            .parameter_values
            .get(name)
            .map(String::as_str)
            .unwrap_or(definition.default_value);
        Some(definition.view(value, definition.source))
    }
}

/// A stored DAX subnet group.
#[derive(Debug, Clone)]
pub struct SubnetGroupRecord {
    pub name: String,
    pub description: Option<String>,
    pub vpc_id: String,
    /// The subnets this group names, as [`resolve_subnet`] resolved them when the
    /// group was created, in the order the request named them.
    pub subnets: Vec<SubnetRecord>,
    /// The Region whose network the group's subnets were resolved in.
    pub region: String,
}

impl SubnetGroupRecord {
    /// The modelled `SubnetGroup` for this record. Each `Subnet` reports the zone the
    /// network model holds for it, and its supported network types are IPv4 (see
    /// [`NETWORK_TYPE_IPV4`]).
    pub fn view(&self) -> SubnetGroup {
        SubnetGroup {
            subnet_group_name: Some(self.name.clone()),
            description: self.description.clone(),
            vpc_id: Some(self.vpc_id.clone()),
            subnets: Some(
                self.subnets
                    .iter()
                    .map(|subnet| Subnet {
                        subnet_identifier: Some(subnet.subnet_id.clone()),
                        subnet_availability_zone: Some(subnet.availability_zone.clone()),
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
            .subnets
            .iter()
            .map(|subnet| subnet.availability_zone.clone())
            .collect();
        zones.sort_unstable();
        zones.dedup();
        zones
    }
}

// ---------------------------------------------------------------------------
// The system-defined parameter catalog.
// ---------------------------------------------------------------------------

/// `Parameter$Source` for a parameter the caching software's own parameter group defines:
/// "How the parameter is defined. For example, `system` denotes a system-defined
/// parameter." DAX publishes its two cluster parameters as `user`, which is the value each
/// catalog entry below carries and what every read of the catalog reports.
pub const PARAMETER_SOURCE_USER: &str = "user";

/// `Parameter$Source` for a parameter defined by the service itself rather than by a
/// parameter group — the other value the member's documentation names.
pub const PARAMETER_SOURCE_SYSTEM: &str = "system";

/// The `Source` values this service reports, so a reported `Source` can be checked
/// against the vocabulary the member documents.
pub const PARAMETER_SOURCES: [&str; 2] = [PARAMETER_SOURCE_SYSTEM, PARAMETER_SOURCE_USER];

/// `Parameter$ParameterType` (`ParameterType` enum) for a parameter that applies to
/// "any nodes" rather than only nodes of a particular type.
pub const PARAMETER_TYPE_DEFAULT: &str = "DEFAULT";

/// `Parameter$ParameterType` for a parameter that carries per-node-type values.
pub const PARAMETER_TYPE_NODE_TYPE_SPECIFIC: &str = "NODE_TYPE_SPECIFIC";

/// `Parameter$IsModifiable` (`IsModifiable` enum) for a parameter "the customer is
/// allowed to modify".
pub const IS_MODIFIABLE_TRUE: &str = "TRUE";

/// `Parameter$ChangeType` (`ChangeType` enum) for a parameter whose new value takes
/// effect without rebooting a node.
pub const CHANGE_TYPE_IMMEDIATE: &str = "IMMEDIATE";

/// `Parameter$DataType` of a parameter whose value is a whole number ("The data type
/// of the parameter. For example, `integer`").
pub const DATA_TYPE_INTEGER: &str = "integer";

/// One system-defined DAX parameter: the definition behind every `Parameter` this
/// service reports, independent of which group is being read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParameterDefinition {
    /// `Parameter$ParameterName`.
    pub name: &'static str,
    /// The value the system defines for it — what a group that has never been updated
    /// reports.
    pub default_value: &'static str,
    /// `Parameter$Description`, as the catalog publishes it.
    pub description: &'static str,
    /// `Parameter$Source`: how this parameter is defined, one of [`PARAMETER_SOURCES`].
    /// It is a property of the catalog entry, so every read of this parameter reports the
    /// same `Source` rather than one chosen where the parameter is projected.
    pub source: &'static str,
    /// `Parameter$DataType`.
    pub data_type: &'static str,
    /// `Parameter$AllowedValues`: "A range of values within which the parameter can be
    /// set", in the AWS range notation (`<min>-` is "`min` and above").
    pub allowed_values: &'static str,
    /// `Parameter$IsModifiable`, one of [`crate::ops::types::IS_MODIFIABLE_VALUES`].
    pub is_modifiable: &'static str,
    /// `Parameter$ChangeType`, one of [`crate::ops::types::CHANGE_TYPE_VALUES`].
    pub change_type: &'static str,
}

impl ParameterDefinition {
    /// The modelled `Parameter` for this definition carrying `value`, reported as
    /// coming from `source`. `ParameterType` is DERIVED: this emulator models no
    /// per-node-type override, so every parameter "can be applied to any nodes" and
    /// reports [`PARAMETER_TYPE_DEFAULT`] with an empty `NodeTypeSpecificValues`.
    pub fn view(&self, value: &str, source: &str) -> Parameter {
        Parameter {
            parameter_name: Some(self.name.to_string()),
            // Nothing in this emulator overrides a parameter for one node type, so
            // every parameter applies to any node: `DEFAULT`, with an empty
            // `NodeTypeSpecificValues`. One that carried such values would report
            // [`PARAMETER_TYPE_NODE_TYPE_SPECIFIC`] instead.
            parameter_type: Some(PARAMETER_TYPE_DEFAULT.to_string()),
            parameter_value: Some(value.to_string()),
            node_type_specific_values: Some(Vec::new()),
            description: Some(self.description.to_string()),
            source: Some(source.to_string()),
            data_type: Some(self.data_type.to_string()),
            allowed_values: Some(self.allowed_values.to_string()),
            is_modifiable: Some(self.is_modifiable.to_string()),
            change_type: Some(self.change_type.to_string()),
        }
    }

    /// This parameter as the catalog defines it: its default value, carrying the entry's
    /// own [`ParameterDefinition::source`]. This is the view `DescribeDefaultParameters`
    /// reports, and the one a group that has never been updated carries.
    pub fn default_view(&self) -> Parameter {
        self.view(self.default_value, self.source)
    }
}

/// The parameters the DAX caching software defines, sorted by name (so [`page`] can
/// page them) and unique — the catalog `DescribeDefaultParameters` reports and the
/// values any parameter group starts from.
///
/// Source: the Amazon DAX Developer Guide's parameter-group / TTL settings, which
/// document exactly two DAX cluster parameters — `query-ttl-millis`, the time to live
/// for the query cache, and `record-ttl-millis`, the time to live for the item cache —
/// each a duration in milliseconds, each defaulting to 300000 (five minutes), each
/// modifiable by the customer and applied to the cluster without a node reboot; the
/// description and `Source` of each are the ones DAX publishes for that entry.
pub const DEFAULT_PARAMETERS: &[ParameterDefinition] = &[
    ParameterDefinition {
        name: "query-ttl-millis",
        default_value: "300000",
        description: "Duration in milliseconds for queries to remain cached",
        source: PARAMETER_SOURCE_USER,
        data_type: DATA_TYPE_INTEGER,
        allowed_values: "0-",
        is_modifiable: IS_MODIFIABLE_TRUE,
        change_type: CHANGE_TYPE_IMMEDIATE,
    },
    ParameterDefinition {
        name: "record-ttl-millis",
        default_value: "300000",
        description: "Duration in milliseconds for records to remain valid in cache \
                      (Default: 0 = infinite)",
        source: PARAMETER_SOURCE_USER,
        data_type: DATA_TYPE_INTEGER,
        allowed_values: "0-",
        is_modifiable: IS_MODIFIABLE_TRUE,
        change_type: CHANGE_TYPE_IMMEDIATE,
    },
];

/// The names of [`DEFAULT_PARAMETERS`], in the order they page in.
pub fn default_parameter_names() -> Vec<&'static str> {
    DEFAULT_PARAMETERS
        .iter()
        .map(|parameter| parameter.name)
        .collect()
}

/// The system definition of the parameter called `name`, or `None` when DAX defines no
/// such parameter.
pub fn default_parameter(name: &str) -> Option<&'static ParameterDefinition> {
    DEFAULT_PARAMETERS
        .iter()
        .find(|parameter| parameter.name == name)
}

/// The values a newly created parameter group carries: the system-defined value of every
/// parameter in [`DEFAULT_PARAMETERS`], keyed by name.
pub fn default_parameter_values() -> BTreeMap<String, String> {
    DEFAULT_PARAMETERS
        .iter()
        .map(|parameter| {
            (
                parameter.name.to_string(),
                parameter.default_value.to_string(),
            )
        })
        .collect()
}

/// One recorded DAX event: what happened to which resource, and when.
#[derive(Debug, Clone)]
pub struct EventRecord {
    /// Monotonic sequence number, which a `DescribeEvents` page token names.
    pub seq: u64,
    /// The resource the event is about (`Event$SourceName`).
    pub source_name: String,
    /// One of [`SOURCE_TYPES`] (`Event$SourceType`).
    pub source_type: String,
    /// What happened (`Event$Message`).
    pub message: String,
    /// When it happened (`Event$Date`).
    pub date: SystemTime,
}

impl EventRecord {
    /// The modelled `Event` for this record.
    pub fn view(&self) -> Event {
        Event {
            source_name: Some(self.source_name.clone()),
            source_type: Some(self.source_type.clone()),
            message: Some(self.message.clone()),
            date: Some(to_date_time(self.date)),
        }
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
    /// Events recorded for this service's resources, oldest first.
    pub events: Vec<EventRecord>,
    /// Monotonic event counter, so one page token names exactly one event.
    pub next_event_seq: u64,
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
            events: Vec::new(),
            next_event_seq: 0,
        }
    }
}

impl ComAmazonawsDaxAmazondaxv3State {
    /// The next node sequence number.
    pub fn next_node_seq(&mut self) -> u64 {
        self.next_node_seq += 1;
        self.next_node_seq
    }

    /// Record one event against `source_name` (a [`SOURCE_TYPES`] resource) at `at`.
    ///
    /// The log is BOUNDED: past [`MAX_EVENTS`] the oldest events are dropped, so no
    /// sequence of requests grows it without limit.
    pub fn record_event(
        &mut self,
        source_name: &str,
        source_type: &str,
        message: &str,
        at: SystemTime,
    ) {
        self.next_event_seq += 1;
        self.events.push(EventRecord {
            seq: self.next_event_seq,
            source_name: source_name.to_string(),
            source_type: source_type.to_string(),
            message: message.to_string(),
            date: at,
        });
        if self.events.len() > MAX_EVENTS {
            let overflow = self.events.len() - MAX_EVENTS;
            self.events.drain(..overflow);
        }
    }

    /// Drop every event older than [`EVENT_RETENTION`] as of `now`.
    ///
    /// This is a move the CLOCK already made due — an event 14 days old is no longer
    /// available whether or not anybody called — so a reporting operation may run it
    /// before it reads.
    pub fn expire_events(&mut self, now: SystemTime) {
        let Some(cutoff) = now.checked_sub(EVENT_RETENTION) else {
            return;
        };
        self.events.retain(|event| event.date >= cutoff);
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
                self.record_event(&name, SOURCE_TYPE_CLUSTER, EVENT_CLUSTER_DELETED, now);
                continue;
            }
            let mut settled = false;
            if let Some(cluster) = self.clusters.get_mut(&name) {
                cluster.status = CLUSTER_AVAILABLE.to_string();
                for node in &mut cluster.nodes {
                    node.node_status = NODE_AVAILABLE.to_string();
                }
                cluster.settles_at = None;
                settled = true;
            }
            if settled {
                self.record_event(&name, SOURCE_TYPE_CLUSTER, EVENT_CLUSTER_AVAILABLE, now);
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

/// The message `ParameterGroupNotFoundFault` carries. This fault has one template and
/// it names no identifier: the requested name is NOT echoed back, on any path that
/// raises it.
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

/// A modelled `TStamp` as an instant, or `None` for a timestamp this service cannot
/// place: a value that is not finite, is before the epoch, or is further from it than
/// an instant reaches. A request carries this value, so every one of those answers
/// `None` for the caller to refuse rather than panicking in the conversion.
pub fn from_date_time(at: ::aws_smithy_types::DateTime) -> Option<SystemTime> {
    let secs = at.as_secs_f64();
    if !secs.is_finite() || secs < 0.0 {
        return None;
    }
    UNIX_EPOCH.checked_add(Duration::try_from_secs_f64(secs).ok()?)
}

// ---------------------------------------------------------------------------
// The message each fault answers with
//
// A fault's wording is built here and nowhere else, so every site raising it answers
// the same text. Most DAX faults have ONE template, which makes the message a property
// of the FAULT rather than of the operation; the exception is a fault raised while
// resolving one element of a LIST of requested identifiers, where the text names the
// element that resolved to nothing — that is the only case in which the caller cannot
// tell from the request which name failed.
// ---------------------------------------------------------------------------

/// `ClusterNotFoundFault` for a request addressed to ONE cluster: the template names no
/// identifier, because the request already carries the only name it could mean.
pub fn cluster_not_found() -> ClusterNotFoundFault {
    ClusterNotFoundFault {
        message: Some("Cluster not found.".to_string()),
    }
}

/// `ClusterNotFoundFault` for one element of a requested LIST of cluster names: the
/// template NAMES that element, so a caller that asked for several clusters can tell
/// which of them is missing. `name` is the identifier as this service holds it —
/// `CreateCluster$ClusterName` documents that it "is stored as a lowercase string", so
/// the lower-cased form is the cluster's identity rather than a rewrite of the request.
pub fn cluster_not_found_named(name: &str) -> ClusterNotFoundFault {
    ClusterNotFoundFault {
        message: Some(format!("Cluster {name} not found.")),
    }
}

/// `SubnetGroupNotFoundFault` for a subnet group this store does not hold. This
/// fault's template DOES name the group it was asked for.
pub fn subnet_group_not_found(name: &str) -> SubnetGroupNotFoundFault {
    SubnetGroupNotFoundFault {
        message: Some(format!("Subnet group {name} not found.")),
    }
}

/// `InvalidClusterStateFault` for a cluster that is not [`CLUSTER_AVAILABLE`], which
/// is the only status a modification is accepted from.
pub fn invalid_cluster_state() -> InvalidClusterStateFault {
    InvalidClusterStateFault {
        message: Some(format!("Cluster is not in the {CLUSTER_AVAILABLE} state.")),
    }
}

/// `InvalidParameterValueException` for a `MaxResults` outside the window both
/// paginated describes document ("The value for `MaxResults` must be between 20 and
/// 100"), carrying [`max_results_out_of_range_message`].
pub fn invalid_max_results() -> InvalidParameterValueException {
    InvalidParameterValueException {
        message: Some(max_results_out_of_range_message()),
    }
}

/// `InvalidParameterValueException` for a `NextToken` this service did not issue,
/// naming the operation whose page it would have continued.
pub fn invalid_next_token(operation: &str) -> InvalidParameterValueException {
    InvalidParameterValueException {
        message: Some(format!(
            "NextToken is not a continuation token from a prior {operation} response"
        )),
    }
}

/// `InvalidParameterValueException` for a request naming subnets that do not exist in
/// the Region's network: the template lists the identifiers that could not be
/// resolved, which is the one part of it built from the request.
pub fn invalid_subnets(subnet_ids: &[String]) -> InvalidParameterValueException {
    InvalidParameterValueException {
        message: Some(format!(
            "Some input subnets in :[{}] are invalid.",
            subnet_ids.join(", ")
        )),
    }
}

/// `InvalidParameterCombinationException` for a `DescribeEvents` window reaching
/// further back than the events DAX retains ([`MAX_EVENT_DURATION`]).
pub fn events_beyond_retention() -> InvalidParameterCombinationException {
    InvalidParameterCombinationException {
        message: Some(format!(
            "Events occurring more than {} days in the past are not available.",
            MAX_EVENT_DURATION / (24 * 60)
        )),
    }
}

/// `ParameterGroupNotFoundFault` for a parameter group this store does not hold,
/// carrying [`parameter_group_not_found_message`].
pub fn parameter_group_not_found() -> ParameterGroupNotFoundFault {
    ParameterGroupNotFoundFault {
        message: Some(parameter_group_not_found_message()),
    }
}

/// `ParameterGroupAlreadyExistsFault` for a name this store already holds. Like this
/// service's other parameter-group fault the template is generic: the name the request
/// carried is NOT echoed back.
pub fn parameter_group_already_exists() -> ParameterGroupAlreadyExistsFault {
    ParameterGroupAlreadyExistsFault {
        message: Some("Parameter Group already exists.".to_string()),
    }
}
