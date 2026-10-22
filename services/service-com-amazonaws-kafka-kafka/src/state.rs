// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-kafka-kafka service.
//!
//! The provisioned-cluster registry lives here, together with the renderings the
//! reporting operations derive from it: `DescribeCluster` / `ListClusters` build a
//! [`ClusterInfo`] with [`Cluster::to_cluster_info`], and `GetBootstrapBrokers`
//! builds its endpoint strings with [`Cluster::broker_endpoints`] plus the access
//! predicates beside it. Keeping them next to the record is what makes the two
//! readers agree: they render the same stored cluster the same way.
//!
//! A cluster's configuration also changes after creation: an update operation
//! (`UpdateConnectivity`) records a [`PendingUpdate`] on the cluster, and
//! [`Cluster::settle`] folds it into the record once the clock owes its
//! completion — so one place decides what every reader then reports.
//!
//! The channel registry lives beside it, keyed by channel ARN and indexed by
//! (cluster ARN, channel name) — a channel is a sub-resource of the cluster it
//! streams from, so it is only ever resolved through that cluster and disappears
//! with it.
//!
//! The replicator registry sits beside them on the same principle: a [`Replicator`]
//! holds the whole replication setup, and [`Replicator::to_replicator_summary`] is
//! the one rendering `ListReplicators` reports it through.
//!
//! The service's Apache Kafka version catalog lives here too
//! ([`KAFKA_VERSION_CATALOG`], with [`compatible_target_versions`] and
//! [`kafka_version_order`]), for the same reason: it is the single list every
//! version-aware operation answers from.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Bound;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    BrokerNodeGroupInfo, BrokerSoftwareInfo, ChannelLoggingInfo, ClientAuthentication, ClusterInfo,
    ConfigurationInfo, ConnectivityInfo, CreateChannelInput, CreateClusterInput,
    DescribeChannelOutput, EncryptionAtRest, EncryptionConfiguration, EncryptionInTransit,
    EncryptionInfo, IcebergDestinationConfiguration, JmxExporter, JmxExporterInfo, KafkaCluster,
    KafkaClusterSummary, LoggingInfo, NodeExporter, NodeExporterInfo, OpenMonitoring,
    OpenMonitoringInfo, Prometheus, PrometheusInfo, Provisioned, Rebalancing, ReplicationInfo,
    ReplicationInfoSummary, ReplicatorSummary, S3DestinationConfiguration, TopicConfiguration,
    ZookeeperAccess,
};

/// How long after `CreateCluster` a new cluster becomes `ACTIVE`.
///
/// CHOSEN, not documented: the real service takes many minutes to provision
/// brokers. The project fixes an emulated clock-owed transition at 250–750 ms, so
/// a caller that describes the cluster immediately still observes `CREATING` (and
/// the state errors that go with it) while a caller that waits sees `ACTIVE`.
pub const CLUSTER_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// How long a cluster update (`UpdateConnectivity` and the sibling update
/// operations) stays in flight before the cluster leaves `UPDATING`.
///
/// CHOSEN, not documented: the real service reconfigures brokers over minutes.
/// The project fixes an emulated clock-owed transition at 250–750 ms, so a caller
/// that describes the cluster immediately still observes `UPDATING`, its active
/// operation ARN and the pre-update configuration, while a caller that waits sees
/// the new configuration and the new cluster version.
pub const CLUSTER_UPDATE_DELAY: Duration = Duration::from_millis(500);

/// The largest number of broker nodes one provisioned cluster may hold, from the
/// `range` on `CreateClusterRequest$NumberOfBrokerNodes` (`__integerMin1Max15`,
/// 1..15) in the model. Enforced by the handler AND clamped in
/// [`Cluster::provisioned`], so no caller of this module can make it allocate a
/// broker list sized by an unchecked request value.
pub const MAX_BROKER_NODES_PER_CLUSTER: i32 = 15;

/// Clusters one account may hold in one region, from the Amazon MSK service
/// quota "Number of clusters per account" (90, per Region). A create past it is
/// refused with the modelled `TooManyRequestsException`, so the registry's
/// cardinality is bounded rather than growing with the number of calls.
pub const MAX_CLUSTERS_PER_REGION: usize = 90;

/// Absolute ceiling on the registry across every region a caller signs for.
///
/// CHOSEN as a process-safety backstop, not an AWS quota: the signing region
/// comes from the request, so the per-region quota alone would still let a caller
/// grow the store by naming new regions. Sized at the per-region quota times more
/// regions than AWS publishes, so it can only ever be reached by a caller trying
/// to exhaust the emulator, never by a realistic client script.
pub const MAX_CLUSTERS_IN_REGISTRY: usize = MAX_CLUSTERS_PER_REGION * 64;

/// How long after `CreateChannel` a new channel becomes `ACTIVE`.
///
/// CHOSEN, not documented: the real service takes a while to stand a channel's
/// delivery pipeline up. The project fixes an emulated clock-owed transition at
/// 250–750 ms, so a caller that describes the channel immediately still observes
/// `CREATING` while a caller that waits sees `ACTIVE`.
pub const CHANNEL_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// Topic configurations one channel carries: `CreateChannelRequest$TopicConfigurationList`
/// documents "Currently exactly one topic must be specified", so a list of any
/// other length is refused with the modelled `BadRequestException`.
pub const TOPIC_CONFIGURATIONS_PER_CHANNEL: usize = 1;

/// Channels one cluster may hold.
///
/// CHOSEN as a process-safety backstop, not a published AWS quota: a channel
/// record is created by a request, so without a ceiling the registry would grow
/// with the number of calls. Sized well above what any realistic client script
/// creates against one cluster.
pub const MAX_CHANNELS_PER_CLUSTER: usize = 100;

/// Amazon MSK provisions a three-node Apache ZooKeeper ensemble alongside a
/// provisioned cluster, which is what `ZookeeperConnectString` enumerates.
const ZOOKEEPER_NODE_COUNT: i32 = 3;

// Port numbers below are the ones Amazon MSK documents in "Port information for
// Amazon MSK" — the access path a client uses picks the port, which is why
// `GetBootstrapBrokers` returns one string per enabled path.
/// Plaintext client-broker access.
pub const PORT_PLAINTEXT: u16 = 9092;
/// TLS client-broker access.
pub const PORT_TLS: u16 = 9094;
/// SASL/SCRAM client-broker access.
pub const PORT_SASL_SCRAM: u16 = 9096;
/// SASL/IAM client-broker access.
pub const PORT_SASL_IAM: u16 = 9098;
/// TLS access over public (service-provided EIP) connectivity.
pub const PORT_PUBLIC_TLS: u16 = 9194;
/// SASL/SCRAM access over public connectivity.
pub const PORT_PUBLIC_SASL_SCRAM: u16 = 9196;
/// SASL/IAM access over public connectivity.
pub const PORT_PUBLIC_SASL_IAM: u16 = 9198;
/// TLS access over multi-VPC (PrivateLink) connectivity.
pub const PORT_VPC_TLS: u16 = 14094;
/// SASL/SCRAM access over multi-VPC connectivity.
pub const PORT_VPC_SASL_SCRAM: u16 = 14096;
/// SASL/IAM access over multi-VPC connectivity.
pub const PORT_VPC_SASL_IAM: u16 = 14098;
/// Plaintext Apache ZooKeeper access.
const PORT_ZOOKEEPER: u16 = 2181;
/// TLS Apache ZooKeeper access.
const PORT_ZOOKEEPER_TLS: u16 = 2182;

/// The default for `EncryptionInTransit$ClientBroker`, per that member's model
/// documentation ("The default value is TLS_PLAINTEXT").
pub const DEFAULT_CLIENT_BROKER: &str = "TLS_PLAINTEXT";
/// The default for `EnhancedMonitoring`: the baseline level the enum names.
pub const DEFAULT_ENHANCED_MONITORING: &str = "DEFAULT";
/// The default for `StorageMode`: local broker storage, i.e. no tiered storage.
pub const DEFAULT_STORAGE_MODE: &str = "LOCAL";
/// The default for `ConnectivityInfo$NetworkType`, per the `NetworkType` model
/// documentation ("By default, a cluster uses the IPv4 network type").
pub const DEFAULT_NETWORK_TYPE: &str = "IPV4";

/// A `KafkaVersionStatus` value: a version Amazon MSK still offers for new
/// clusters and for upgrades.
pub const KAFKA_VERSION_ACTIVE: &str = "ACTIVE";
/// A `KafkaVersionStatus` value: a version Amazon MSK has deprecated. Existing
/// clusters keep running it, but it is not an upgrade target.
pub const KAFKA_VERSION_DEPRECATED: &str = "DEPRECATED";

/// One Apache Kafka version Amazon MSK knows about, with the lifecycle status it
/// publishes for that version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KafkaVersionEntry {
    /// The version string as MSK names it, e.g. a three-component release, a
    /// `.x` name (MSK patches the trailing component for you) or the
    /// `2.8.2.tiered` variant.
    pub version: &'static str,
    /// [`KAFKA_VERSION_ACTIVE`] or [`KAFKA_VERSION_DEPRECATED`] — one of the
    /// model's `KafkaVersionStatus` values
    /// ([`crate::ops::types::KAFKA_VERSION_STATUS_VALUES`]).
    pub status: &'static str,
}

/// Every Apache Kafka version this emulator's MSK knows, OLDEST FIRST, so the
/// catalog order is the upgrade ladder itself.
///
/// Source: the Amazon MSK Developer Guide, "Supported Apache Kafka versions"
/// (<https://docs.aws.amazon.com/msk/latest/developerguide/supported-kafka-versions.html>),
/// which lists the versions MSK offers and the ones it has deprecated. MSK
/// deprecated every version older than 2.8.1, which is the ACTIVE/DEPRECATED
/// boundary below; a deprecated version is still a valid SOURCE version (a
/// cluster already running it can be upgraded away from it) but never a target.
///
/// This is the one catalog: `GetCompatibleKafkaVersions` derives its matrix from
/// it, so nothing has to keep a second copy of the version list in step.
pub const KAFKA_VERSION_CATALOG: &[KafkaVersionEntry] = &[
    KafkaVersionEntry { version: "1.1.1", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.1.0", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.2.1", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.3.1", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.4.1", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.4.1.1", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.5.1", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.6.0", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.6.1", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.6.2", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.6.3", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.7.0", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.7.1", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.7.2", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.8.0", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.8.1", status: KAFKA_VERSION_ACTIVE },
    KafkaVersionEntry { version: "2.8.2.tiered", status: KAFKA_VERSION_ACTIVE },
    KafkaVersionEntry { version: "3.1.1", status: KAFKA_VERSION_ACTIVE },
    KafkaVersionEntry { version: "3.2.0", status: KAFKA_VERSION_ACTIVE },
    KafkaVersionEntry { version: "3.3.1", status: KAFKA_VERSION_ACTIVE },
    KafkaVersionEntry { version: "3.3.2", status: KAFKA_VERSION_ACTIVE },
    KafkaVersionEntry { version: "3.4.0", status: KAFKA_VERSION_ACTIVE },
    KafkaVersionEntry { version: "3.5.1", status: KAFKA_VERSION_ACTIVE },
    KafkaVersionEntry { version: "3.6.0", status: KAFKA_VERSION_ACTIVE },
    KafkaVersionEntry { version: "3.7.x", status: KAFKA_VERSION_ACTIVE },
    KafkaVersionEntry { version: "3.8.x", status: KAFKA_VERSION_ACTIVE },
    KafkaVersionEntry { version: "3.9.x", status: KAFKA_VERSION_ACTIVE },
];

/// The versions a cluster running `source` can be upgraded to: every catalogued
/// version that is still [`KAFKA_VERSION_ACTIVE`] and strictly NEWER than
/// `source`, oldest first.
///
/// Amazon MSK upgrades a cluster's Apache Kafka version in place and does not
/// support downgrading, and a deprecated version cannot be upgraded to — so a
/// version at or below the source, and every deprecated version, is excluded.
/// `source` need not be catalogued: a cluster running a version this catalog
/// does not list is still ordered against it by [`kafka_version_order`].
pub fn compatible_target_versions(source: &str) -> Vec<String> {
    KAFKA_VERSION_CATALOG
        .iter()
        .filter(|entry| {
            entry.status == KAFKA_VERSION_ACTIVE
                && kafka_version_order(entry.version, source) == std::cmp::Ordering::Greater
        })
        .map(|entry| entry.version.to_string())
        .collect()
}

/// Order two Apache Kafka version strings the way the upgrade ladder runs:
/// component by component numerically, so `2.8.1` precedes `2.8.2.tiered` and
/// `3.6.0` precedes `3.7.x`.
///
/// A `.x` component is MSK's name for "the newest patch of this minor", so it
/// orders ABOVE every numbered patch of the same minor. Any other
/// non-numeric component (the `tiered` of `2.8.2.tiered`) is a qualifier, which
/// orders after the bare numeric version it qualifies and otherwise
/// lexicographically — enough to give the catalog one total order without
/// inventing semantics for it.
pub fn kafka_version_order(left: &str, right: &str) -> std::cmp::Ordering {
    kafka_version_key(left).cmp(&kafka_version_key(right))
}

/// `(numeric components, qualifier)` for a version string — the sort key
/// [`kafka_version_order`] compares. `Vec<u32>` compares lexicographically, so a
/// version that is a prefix of another (`2.4.1` of `2.4.1.1`) orders below it.
fn kafka_version_key(version: &str) -> (Vec<u32>, String) {
    let mut numbers: Vec<u32> = Vec::new();
    let mut qualifier = String::new();
    for component in version.split('.') {
        if !qualifier.is_empty() {
            qualifier.push('.');
            qualifier.push_str(component);
        } else if let Ok(number) = component.parse::<u32>() {
            numbers.push(number);
        } else if component == "x" {
            // "the latest patch of this minor": above every numbered patch.
            numbers.push(u32::MAX);
        } else {
            qualifier.push_str(component);
        }
    }
    (numbers, qualifier)
}

/// The message Amazon MSK answers a request naming a resource it does not hold.
///
/// A CONSTANT of the service, not built from the request: every operation that
/// models `NotFoundException` reports this one wording, which is what the
/// behavioural corpus records the real service answering (the apostrophe is
/// U+2019, as the service writes it). The parameter at fault is named by
/// `NotFoundException$InvalidParameter`, not by the message, so a reader that
/// knows which member it was still says the same thing about the resource.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "The requested resource doesn\u{2019}t exist.";

/// The `ClusterType` of a cluster whose brokers this emulator provisions — one of
/// the two values the model's `ClusterType` enum names. It is STORED on the record
/// (see [`Cluster::cluster_type`]) rather than assumed by a reader, so a listing's
/// `ClusterTypeFilter` tests what each cluster IS; the serverless create path
/// (`CreateClusterV2` with a `Serverless` body) is still a scaffold, so no record
/// carries `SERVERLESS` yet.
pub const CLUSTER_TYPE_PROVISIONED: &str = "PROVISIONED";

/// The largest page a cluster listing may report, from the `range` on the
/// `MaxResults` shape (1..100) that `ListClusters` and `ListClustersV2` both bind.
/// Also the page size used when a request names none.
pub const MAX_RESULTS_LIMIT: i32 = 100;

/// Marker that prefixes a cluster page token's plaintext, so a token minted for
/// some other list cannot be mistaken for one of these.
const CLUSTER_PAGE_TOKEN_PREFIX: &str = "kafka:clusters:";

/// One broker node of a provisioned cluster, and the DNS name clients reach it on.
/// `GetBootstrapBrokers` renders every bootstrap string from these hosts.
#[derive(Debug, Clone)]
pub struct BrokerNode {
    /// 1-based broker id, as it appears in the `b-<id>` DNS label.
    pub id: i32,
    /// `b-<id>.<cluster>.<suffix>.kafka.<region>.amazonaws.com` — the port is
    /// appended per access path.
    pub endpoint_host: String,
}

/// A provisioned MSK cluster, holding the whole submitted configuration so every
/// reader can be computed from it rather than echoed from a request.
#[derive(Debug, Clone)]
pub struct Cluster {
    pub arn: String,
    pub name: String,
    /// The region the cluster lives in: the signing region of the request that
    /// created it, and the region segment of its ARN and endpoints. A cluster name
    /// is unique per account and region, so this is half of its registry key.
    pub region: String,
    /// A `ClusterType` value — which of the model's two cluster kinds this record
    /// is, so a listing's `ClusterTypeFilter` and the `Cluster` shape's
    /// `clusterType` both read it off the record instead of assuming one.
    pub cluster_type: String,
    pub kafka_version: String,
    pub current_version: String,
    pub creation_time: SystemTime,
    /// The instant the clock owes this cluster its `CREATING` -> `ACTIVE`
    /// transition. Stored, never slept on.
    pub active_due: SystemTime,
    /// A `ClusterState` value.
    pub state: String,
    pub number_of_broker_nodes: i32,
    pub broker_node_group_info: BrokerNodeGroupInfo,
    pub client_authentication: Option<ClientAuthentication>,
    /// Always stored with its defaults resolved, so every reader reports the
    /// effective settings the cluster actually runs with.
    pub encryption_info: EncryptionInfo,
    pub enhanced_monitoring: String,
    pub open_monitoring: OpenMonitoringInfo,
    pub logging_info: Option<LoggingInfo>,
    pub storage_mode: String,
    pub configuration_info: Option<ConfigurationInfo>,
    pub rebalancing: Option<Rebalancing>,
    /// Ordered so `DescribeCluster` renders a stable map.
    pub tags: BTreeMap<String, String>,
    pub zookeeper_connect_string: String,
    pub zookeeper_connect_string_tls: String,
    pub brokers: Vec<BrokerNode>,
    /// Whether Apache ZooKeeper access is turned on, as
    /// `UpdateConnectivity`'s `ZookeeperAccess` leaves it. A cluster is
    /// provisioned with access on. The model reports this setting through
    /// `MutableClusterInfo$ZookeeperAccess`, i.e. through the cluster-operation
    /// readers, so it is stored here rather than rendered by `ClusterInfo`,
    /// which models no member for it.
    pub zookeeper_access_enabled: bool,
    /// Which revision of its configuration the cluster is at.
    /// [`Cluster::current_version`] is this number's token, so the version
    /// changes exactly when a completed update changes the cluster.
    pub revision: u64,
    /// The update this cluster has in flight, if any: the one cluster operation
    /// acting on it, and the change the clock owes it. `Some` is exactly what
    /// makes the cluster `UPDATING` with an active operation ARN.
    pub pending_update: Option<PendingUpdate>,
}

/// A cluster operation in flight: the change a caller asked for, the ARN of the
/// operation that will apply it, and the instant the clock owes its completion.
/// Stored, never slept on — [`Cluster::settle`] materialises it.
#[derive(Debug, Clone)]
pub struct PendingUpdate {
    /// `ClusterInfo$ActiveOperationArn` while the operation runs, and the
    /// `ClusterOperationArn` the update call answered with.
    pub operation_arn: String,
    /// When the operation is due to finish.
    pub due: SystemTime,
    /// What it changes.
    pub change: ClusterChange,
}

/// The change one cluster operation applies. Each update operation this service
/// implements contributes the variant describing what it changes, so a cluster
/// holds at most one in-flight change whatever produced it.
#[derive(Debug, Clone)]
pub enum ClusterChange {
    /// `UpdateConnectivity`: the broker access configuration, the ZooKeeper
    /// access setting, or both. `None` means the request left that part of the
    /// cluster alone.
    Connectivity {
        connectivity_info: Option<ConnectivityInfo>,
        zookeeper_access: Option<ZookeeperAccess>,
    },
}

/// A channel streaming one cluster topic to Amazon S3 or Apache Iceberg, holding
/// the whole submitted configuration so every reader is computed from the record
/// rather than echoed from a request.
#[derive(Debug, Clone)]
pub struct Channel {
    pub arn: String,
    pub name: String,
    /// The ARN of the cluster the channel reads from. A channel name is unique
    /// WITHIN a cluster, so this is half of its registry key, and a channel is
    /// only ever reported under the cluster that holds it.
    pub cluster_arn: String,
    /// The region the channel lives in: the region of the cluster that holds it,
    /// and the region segment of its own ARN.
    pub region: String,
    pub creation_time: SystemTime,
    /// The instant the clock owes this channel its `CREATING` -> `ACTIVE`
    /// transition. Stored, never slept on.
    pub active_due: SystemTime,
    /// A `ChannelStatus` value.
    pub status: String,
    /// A `ChannelDestinationType` value, DERIVED from which destination
    /// configuration the create carried rather than taken from the request.
    pub destination_type: String,
    pub encryption_configuration: Option<EncryptionConfiguration>,
    pub iceberg_destination_configuration: Option<IcebergDestinationConfiguration>,
    pub s3_destination_configuration: Option<S3DestinationConfiguration>,
    pub topic_configuration_list: Vec<TopicConfiguration>,
    pub logging_info: Option<ChannelLoggingInfo>,
    /// Ordered so `DescribeChannel` renders a stable map.
    pub tags: BTreeMap<String, String>,
    /// The cluster operation that created the channel, as `CreateChannel` and
    /// `DescribeChannel` both report it: minted once with the channel and stored,
    /// so the two readers never disagree about it.
    pub cluster_operation_arn: String,
}

/// The `ChannelDestinationType` value for a channel whose records land in an
/// Apache Iceberg table.
pub const DESTINATION_TYPE_ICEBERG: &str = "ICEBERG";
/// The `ChannelDestinationType` value for a channel whose records land in an
/// Amazon S3 bucket.
pub const DESTINATION_TYPE_S3: &str = "S3";

/// Why [`ComAmazonawsKafkaKafkaState::register_channel`] refused to register a
/// channel. Every variant leaves the registry exactly as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelRejected {
    /// The cluster already holds [`MAX_CHANNELS_PER_CLUSTER`] channels.
    ClusterQuota,
    /// A channel with this ARN is already registered. The registry's monotonic
    /// ordinal makes this unreachable; it is refused rather than overwriting an
    /// existing channel.
    DuplicateArn,
}

/// Which clusters a cluster listing keeps, on top of the endpoint's own region.
/// A `None` member filters nothing, which is how an absent request member behaves.
#[derive(Debug, Default, Clone, Copy)]
pub struct ClusterFilter<'a> {
    /// Keep the clusters whose name starts with this prefix
    /// (`ClusterNameFilter`).
    pub name_prefix: Option<&'a str>,
    /// Keep the clusters whose `ClusterType` equals this (`ClusterTypeFilter`).
    pub cluster_type: Option<&'a str>,
}

impl ClusterFilter<'_> {
    /// Whether `cluster` passes every member this filter carries.
    fn keeps(&self, cluster: &Cluster) -> bool {
        self.name_prefix
            .is_none_or(|prefix| cluster.name.starts_with(prefix))
            && self
                .cluster_type
                .is_none_or(|kind| cluster.cluster_type == kind)
    }
}

/// One page of a cluster listing: the registry keys it reports, and the token the
/// next page resumes from.
#[derive(Debug, Default)]
pub struct ClusterPage {
    /// The ARNs this page reports, in the registry's own order.
    pub keys: Vec<String>,
    /// The token a caller passes back to get the next page, absent on the last
    /// page — minted from the registry KEY the resume comparison uses, so a full
    /// page can never be mistaken for the last one.
    pub next_token: Option<String>,
}

/// How long after `CreateReplicator` a new replicator reaches `RUNNING`.
///
/// CHOSEN, not documented: the real service takes minutes to stand a replicator
/// up. The project fixes an emulated clock-owed transition at 250–750 ms, so a
/// caller that lists immediately still observes `CREATING` while one that waits
/// sees `RUNNING`.
pub const REPLICATOR_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// The `ReplicatorState` value a replicator holds until the clock owes it
/// [`REPLICATOR_STATE_RUNNING`]; both are members of the model's
/// `ReplicatorState` enum.
pub const REPLICATOR_STATE_CREATING: &str = "CREATING";
/// The `ReplicatorState` value a settled replicator reports.
pub const REPLICATOR_STATE_RUNNING: &str = "RUNNING";

/// One Kafka cluster a replicator replicates between, under the alias the
/// replicator knows it by.
///
/// The whole submitted `KafkaCluster` is kept, so every reader can be COMPUTED
/// from the record: `KafkaClusterSummary` carries only the cluster details and
/// the alias, while `DescribeReplicator`'s richer shape needs the VPC,
/// authentication and encryption settings the same record already holds.
#[derive(Debug, Clone)]
pub struct ReplicatorCluster {
    /// The alias replicated topic names are prefixed with — `KafkaClusterSummary$
    /// KafkaClusterAlias`, and what a `ReplicationInfo` is reported under.
    pub alias: String,
    /// The cluster as the replicator's creator submitted it.
    pub cluster: KafkaCluster,
}

impl ReplicatorCluster {
    /// Whether this is the cluster a `ReplicationInfo` endpoint names.
    ///
    /// A replication endpoint names its cluster by ARN (an Amazon MSK cluster) or
    /// by id (an Apache Kafka cluster), which is exactly the two ways a
    /// `KafkaCluster` can be given — so one comparison resolves either form
    /// against the stored setup.
    pub fn names(&self, arn: Option<&str>, id: Option<&str>) -> bool {
        let msk_matches = match (arn, self.cluster.amazon_msk_cluster.as_ref()) {
            (Some(arn), Some(msk)) => msk.msk_cluster_arn == arn,
            _ => false,
        };
        let apache_matches = match (id, self.cluster.apache_kafka_cluster.as_ref()) {
            (Some(id), Some(apache)) => apache.apache_kafka_cluster_id == id,
            _ => false,
        };
        msk_matches || apache_matches
    }

    /// The cluster as the `KafkaClusterSummary` a replicator reader reports: the
    /// alias plus whichever of the two cluster kinds the record holds. The
    /// summary shape models no VPC, authentication or encryption member, so those
    /// stay in the record rather than being reported here.
    pub fn to_summary(&self) -> KafkaClusterSummary {
        KafkaClusterSummary {
            amazon_msk_cluster: self.cluster.amazon_msk_cluster.clone(),
            apache_kafka_cluster: self.cluster.apache_kafka_cluster.clone(),
            kafka_cluster_alias: Some(self.alias.clone()),
        }
    }
}

/// A replicator, holding the whole replication setup so every reader is computed
/// from it rather than echoed from a request.
#[derive(Debug, Clone)]
pub struct Replicator {
    /// `ReplicatorArn` — the ARN in the region this endpoint serves it from.
    pub arn: String,
    /// `ReplicatorResourceArn` — the ARN of the replicator resource in the region
    /// the replicator was CREATED in. The same ARN for a replicator created here,
    /// and a different one for a replicator reference.
    pub resource_arn: String,
    pub name: String,
    /// The region this replicator lives in: the signing region of the request
    /// that created it, and the region segment of its ARN. `ListReplicators` is
    /// regional, so this is what scopes it to one endpoint.
    pub region: String,
    pub current_version: String,
    pub creation_time: SystemTime,
    /// The instant the clock owes this replicator its `CREATING` -> `RUNNING`
    /// transition. Stored, never slept on.
    pub running_due: SystemTime,
    /// A `ReplicatorState` value.
    pub state: String,
    /// Whether this record is a reference to a replicator created in another
    /// region (`IsReplicatorReference`).
    pub is_replicator_reference: bool,
    /// The clusters replication runs between, in submission order.
    pub kafka_clusters: Vec<ReplicatorCluster>,
    /// The replications configured between those clusters, in submission order.
    pub replication_info: Vec<ReplicationInfo>,
}

impl Replicator {
    /// Apply the `CREATING` -> `RUNNING` transition once `now` has reached
    /// [`Self::running_due`]. Idempotent, and a no-op for every other state: only
    /// a transition the clock already owed is materialised here.
    pub fn settle(&mut self, now: SystemTime) {
        if self.state == REPLICATOR_STATE_CREATING && now >= self.running_due {
            self.state = REPLICATOR_STATE_RUNNING.to_string();
        }
    }

    /// The alias of the stored cluster an endpoint of a `ReplicationInfo` names,
    /// or `None` when the setup holds no such cluster.
    pub fn alias_of(&self, arn: Option<&str>, id: Option<&str>) -> Option<String> {
        self.kafka_clusters
            .iter()
            .find(|cluster| cluster.names(arn, id))
            .map(|cluster| cluster.alias.clone())
    }

    /// The replicator as the `ReplicatorSummary` `ListReplicators` reports.
    ///
    /// Each replication is reported by the ALIASES of its endpoints, resolved
    /// against this replicator's own cluster list — the summary shape carries no
    /// ARN or id member, so the aliases are derived from the stored setup rather
    /// than echoed from it.
    pub fn to_replicator_summary(&self) -> ReplicatorSummary {
        ReplicatorSummary {
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            current_version: Some(self.current_version.clone()),
            is_replicator_reference: Some(self.is_replicator_reference),
            kafka_clusters_summary: Some(
                self.kafka_clusters
                    .iter()
                    .map(ReplicatorCluster::to_summary)
                    .collect(),
            ),
            replication_info_summary_list: Some(
                self.replication_info
                    .iter()
                    .map(|info| ReplicationInfoSummary {
                        source_kafka_cluster_alias: self.alias_of(
                            info.source_kafka_cluster_arn.as_deref(),
                            info.source_kafka_cluster_id.as_deref(),
                        ),
                        target_kafka_cluster_alias: self.alias_of(
                            info.target_kafka_cluster_arn.as_deref(),
                            info.target_kafka_cluster_id.as_deref(),
                        ),
                    })
                    .collect(),
            ),
            replicator_arn: Some(self.arn.clone()),
            replicator_name: Some(self.name.clone()),
            replicator_resource_arn: Some(self.resource_arn.clone()),
            replicator_state: Some(self.state.clone()),
        }
    }
}

/// Why [`ComAmazonawsKafkaKafkaState::register_cluster`] refused to register a
/// cluster. Every variant leaves the registry exactly as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterRejected {
    /// The region already holds [`MAX_CLUSTERS_PER_REGION`] clusters.
    RegionQuota,
    /// The registry already holds [`MAX_CLUSTERS_IN_REGISTRY`] clusters.
    RegistryCeiling,
    /// A cluster with this ARN is already registered. The registry's monotonic
    /// ordinal makes this unreachable; it is refused rather than overwriting an
    /// existing cluster.
    DuplicateArn,
}

/// Per-service state for com-amazonaws-kafka-kafka. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsKafkaKafkaState {
    /// Every live cluster, keyed by ARN. `BTreeMap` so `ListClusters` has one
    /// total order to page through.
    pub clusters: BTreeMap<String, Cluster>,
    /// (region, cluster name) -> ARN. A cluster name is unique within an account
    /// and region — the same name in two regions is two clusters — so the key
    /// carries the region as well as the name.
    pub cluster_arn_by_name: HashMap<(String, String), String>,
    /// region -> how many clusters it holds. Maintained on insert and remove by
    /// [`Self::register_cluster`] / [`Self::unregister_cluster`], so enforcing the
    /// per-region quota costs a lookup rather than a walk of the registry.
    pub clusters_per_region: HashMap<String, usize>,
    /// Monotonic counter feeding the ARN and DNS suffixes, so each cluster gets a
    /// distinct identity and a rebuilt emulator reproduces the same ones.
    pub next_cluster_suffix: u64,
    /// Every live channel, keyed by ARN. `BTreeMap` so `ListChannels` has one
    /// total order to page through.
    pub channels: BTreeMap<String, Channel>,
    /// (cluster ARN, channel name) -> channel ARN. A channel name is unique
    /// within a cluster — the same name under two clusters is two channels — so
    /// the key carries the cluster as well as the name.
    pub channel_arn_by_name: HashMap<(String, String), String>,
    /// cluster ARN -> the ARNs of the channels it holds, so the per-cluster
    /// quota costs a lookup and deleting a cluster drops exactly its own
    /// channels rather than walking the whole registry.
    pub channel_arns_by_cluster: HashMap<String, BTreeSet<String>>,
    /// Monotonic counter feeding each channel's ARN and its create operation's
    /// ARN, so every channel gets a distinct identity.
    pub next_channel_suffix: u64,
    /// Every registered replicator, keyed by ARN. `BTreeMap` so
    /// `ListReplicators` has one total order to page through, the same way the
    /// cluster registry gives `ListClusters` one.
    pub replicators: BTreeMap<String, Replicator>,
}

impl ComAmazonawsKafkaKafkaState {
    /// The ARN registered for `name` in `region`, if any. This is the lookup that
    /// makes a repeated `CreateCluster` a conflict.
    pub fn arn_for_name(&self, region: &str, name: &str) -> Option<&String> {
        self.cluster_arn_by_name
            .get(&(region.to_string(), name.to_string()))
    }

    /// How many clusters `region` currently holds — what the per-region quota is
    /// checked against. O(1): the count is maintained as clusters come and go.
    pub fn cluster_count_in(&self, region: &str) -> usize {
        self.clusters_per_region.get(region).copied().unwrap_or(0)
    }

    /// Add `cluster` to every index that must know about it. The one insertion
    /// path, so the ARN registry, the (region, name) index and the per-region
    /// count cannot drift apart — and the one place the registry's ceilings are
    /// enforced, so they hold for every caller of this module rather than only
    /// for a handler that remembers to check first.
    ///
    /// Refuses, changing nothing, when the region is at
    /// [`MAX_CLUSTERS_PER_REGION`], when the whole registry is at
    /// [`MAX_CLUSTERS_IN_REGISTRY`] (the backstop that bounds growth however many
    /// region names a caller invents, since the region comes from the request),
    /// or when the ARN is already registered.
    pub fn register_cluster(&mut self, cluster: Cluster) -> Result<(), RegisterRejected> {
        if self.clusters.contains_key(&cluster.arn) {
            return Err(RegisterRejected::DuplicateArn);
        }
        if self.clusters.len() >= MAX_CLUSTERS_IN_REGISTRY {
            return Err(RegisterRejected::RegistryCeiling);
        }
        if self.cluster_count_in(&cluster.region) >= MAX_CLUSTERS_PER_REGION {
            return Err(RegisterRejected::RegionQuota);
        }
        *self
            .clusters_per_region
            .entry(cluster.region.clone())
            .or_insert(0) += 1;
        self.cluster_arn_by_name.insert(
            (cluster.region.clone(), cluster.name.clone()),
            cluster.arn.clone(),
        );
        self.clusters.insert(cluster.arn.clone(), cluster);
        Ok(())
    }

    /// Remove the cluster `arn` names from every index, handing it back. The one
    /// removal path, for the same reason.
    ///
    /// A channel reads from a cluster, so a cluster that is gone holds no
    /// channels: every channel registered under `arn` is dropped here too, which
    /// is why this is also the one place that can orphan a channel — and does
    /// not.
    pub fn unregister_cluster(&mut self, arn: &str) -> Option<Cluster> {
        let cluster = self.clusters.remove(arn)?;
        self.cluster_arn_by_name
            .remove(&(cluster.region.clone(), cluster.name.clone()));
        if let Some(count) = self.clusters_per_region.get_mut(&cluster.region) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.clusters_per_region.remove(&cluster.region);
            }
        }
        for channel_arn in self
            .channel_arns_by_cluster
            .remove(arn)
            .unwrap_or_default()
        {
            if let Some(channel) = self.channels.remove(&channel_arn) {
                self.channel_arn_by_name
                    .remove(&(channel.cluster_arn, channel.name));
            }
        }
        Some(cluster)
    }

    /// The cluster `arn` names, as an endpoint for `region` sees it.
    ///
    /// A regional endpoint serves only its own region's clusters, so a
    /// well-formed ARN belonging to another region resolves to `None` exactly as
    /// an absent one does. Every reader resolves a cluster through this or
    /// [`Self::settled_cluster_in`], so none of them can disagree about which
    /// clusters this endpoint holds.
    pub fn cluster_in(&self, region: &str, arn: &str) -> Option<&Cluster> {
        self.clusters
            .get(arn)
            .filter(|cluster| cluster.region == region)
    }

    /// Add `replicator` to the registry, handing back whether it went in.
    ///
    /// The one insertion path, so no caller can overwrite a registered
    /// replicator: a repeated ARN is refused and the registry is left exactly as
    /// it was. No AWS quota is claimed here — the writer that will register
    /// replicators (`CreateReplicator`) is still a scaffold, and whatever quota
    /// it enforces belongs with it, next to the error it answers.
    pub fn register_replicator(&mut self, replicator: Replicator) -> Result<(), RegisterRejected> {
        if self.replicators.contains_key(&replicator.arn) {
            return Err(RegisterRejected::DuplicateArn);
        }
        self.replicators.insert(replicator.arn.clone(), replicator);
        Ok(())
    }

    /// The replicator `arn` names, as an endpoint for `region` sees it, with the
    /// `CREATING` -> `RUNNING` transition the clock already owed THAT replicator
    /// materialised.
    ///
    /// Scoped to the one replicator being reported: a read must not do work
    /// proportional to the whole registry, and no other replicator's state is
    /// this request's business. A regional endpoint serves only its own region's
    /// replicators, so a replicator of another region resolves to `None` exactly
    /// as an absent one does.
    pub fn settled_replicator_in(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
    ) -> Option<&Replicator> {
        let replicator = self.replicators.get_mut(arn)?;
        if replicator.region != region {
            return None;
        }
        replicator.settle(now);
        Some(replicator)
    }

    /// The same lookup, with the transitions the clock already owed THIS cluster
    /// materialised (`CREATING` -> `ACTIVE`, and an in-flight update whose
    /// completion is due).
    ///
    /// Scoped to the one cluster being reported: a read must not do work
    /// proportional to the whole registry, and no other cluster's status is this
    /// request's business.
    pub fn settled_cluster_in(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
    ) -> Option<&Cluster> {
        self.settled_cluster_in_mut(region, arn, now)
            .map(|cluster| &*cluster)
    }

    /// The same settled lookup, handed back for update — what an operation that
    /// changes the cluster resolves it through, so a mutating call and a
    /// reporting one agree about which clusters this endpoint holds and about
    /// what the clock already owed them.
    pub fn settled_cluster_in_mut(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
    ) -> Option<&mut Cluster> {
        let cluster = self.clusters.get_mut(arn)?;
        if cluster.region != region {
            return None;
        }
        cluster.settle(now);
        Some(cluster)
    }

    /// The ARN registered for channel `name` under the cluster `cluster_arn`, if
    /// any. This is the lookup that makes a repeated `CreateChannel` a conflict.
    pub fn channel_arn_for_name(&self, cluster_arn: &str, name: &str) -> Option<&String> {
        self.channel_arn_by_name
            .get(&(cluster_arn.to_string(), name.to_string()))
    }

    /// How many channels `cluster_arn` currently holds — what the per-cluster
    /// ceiling is checked against. O(1): the index is maintained as channels come
    /// and go.
    pub fn channel_count_in_cluster(&self, cluster_arn: &str) -> usize {
        self.channel_arns_by_cluster
            .get(cluster_arn)
            .map_or(0, BTreeSet::len)
    }

    /// Add `channel` to every index that must know about it. The one insertion
    /// path, so the ARN registry, the (cluster, name) index and the per-cluster
    /// index cannot drift apart — and the one place the per-cluster ceiling is
    /// enforced, so it holds for every caller of this module.
    ///
    /// Refuses, changing nothing, when the cluster is at
    /// [`MAX_CHANNELS_PER_CLUSTER`] or when the ARN is already registered.
    pub fn register_channel(&mut self, channel: Channel) -> Result<(), ChannelRejected> {
        if self.channels.contains_key(&channel.arn) {
            return Err(ChannelRejected::DuplicateArn);
        }
        if self.channel_count_in_cluster(&channel.cluster_arn) >= MAX_CHANNELS_PER_CLUSTER {
            return Err(ChannelRejected::ClusterQuota);
        }
        self.channel_arns_by_cluster
            .entry(channel.cluster_arn.clone())
            .or_default()
            .insert(channel.arn.clone());
        self.channel_arn_by_name.insert(
            (channel.cluster_arn.clone(), channel.name.clone()),
            channel.arn.clone(),
        );
        self.channels.insert(channel.arn.clone(), channel);
        Ok(())
    }

    /// Remove the channel `arn` names from every index, handing it back. The one
    /// removal path, for the same reason.
    pub fn unregister_channel(&mut self, arn: &str) -> Option<Channel> {
        let channel = self.channels.remove(arn)?;
        self.channel_arn_by_name
            .remove(&(channel.cluster_arn.clone(), channel.name.clone()));
        if let Some(arns) = self.channel_arns_by_cluster.get_mut(&channel.cluster_arn) {
            arns.remove(arn);
            if arns.is_empty() {
                self.channel_arns_by_cluster.remove(&channel.cluster_arn);
            }
        }
        Some(channel)
    }

    /// The channel `channel_arn` names, as an endpoint for `region` sees it, and
    /// only when the cluster in the request's path is the one that holds it.
    ///
    /// A channel is addressed under its cluster, so a channel ARN presented with
    /// another cluster's ARN resolves to `None` exactly as an absent one does —
    /// the same way [`Self::cluster_in`] refuses another region's cluster.
    pub fn channel_in(
        &self,
        region: &str,
        cluster_arn: &str,
        channel_arn: &str,
    ) -> Option<&Channel> {
        self.channels
            .get(channel_arn)
            .filter(|channel| channel.region == region && channel.cluster_arn == cluster_arn)
    }

    /// The same lookup, with the `CREATING` -> `ACTIVE` transition the clock
    /// already owed THIS channel materialised.
    ///
    /// Scoped to the one channel being reported, for the same reason
    /// [`Self::settled_cluster_in`] is.
    pub fn settled_channel_in(
        &mut self,
        region: &str,
        cluster_arn: &str,
        channel_arn: &str,
        now: SystemTime,
    ) -> Option<&Channel> {
        let channel = self.channels.get_mut(channel_arn)?;
        if channel.region != region || channel.cluster_arn != cluster_arn {
            return None;
        }
        channel.settle(now);
        Some(channel)
    }

    /// Choose one page of this endpoint's clusters: the keys a listing reports,
    /// plus its `NextToken`.
    ///
    /// Every cluster listing is a REGIONAL api, so only the clusters `region`
    /// holds are considered — the same set the ARN readers resolve against. The
    /// registry is ordered by ARN, so a token resumes by SEEKING to its position
    /// rather than walking the keys already returned; the scan from there stops as
    /// soon as the page is full, and only entries that fail the region or the
    /// filter are skipped past. `page_size` is clamped into the `MaxResults` range
    /// the model declares, so no caller of this module can size the page from an
    /// unchecked value.
    pub fn cluster_page(
        &self,
        region: &str,
        filter: ClusterFilter<'_>,
        resume_after: Option<&str>,
        page_size: usize,
    ) -> ClusterPage {
        let page_size = page_size.clamp(1, MAX_RESULTS_LIMIT as usize);
        let mut keys: Vec<String> = Vec::with_capacity(page_size);
        let mut more_remain = false;
        let resume_from = match resume_after {
            Some(last) => Bound::Excluded(last),
            None => Bound::Unbounded,
        };
        for (arn, cluster) in self.clusters.range::<str, _>((resume_from, Bound::Unbounded)) {
            if cluster.region != region || !filter.keeps(cluster) {
                continue;
            }
            if keys.len() == page_size {
                more_remain = true;
                break;
            }
            keys.push(arn.clone());
        }
        let next_token = if more_remain {
            keys.last().map(|last| encode_cluster_page_token(last))
        } else {
            None
        };
        ClusterPage { keys, next_token }
    }
}

/// Encode the ARN a page stopped at as a token. Base64 of a marked plaintext:
/// the token is ENCODED, not authenticated, so a caller could decode it or mint
/// one. That is harmless here — the ARN inside is only used as a `>` comparison
/// against the keys of the caller's own registry, so a forged resume point can
/// only skip or repeat entries the caller may already list.
pub fn encode_cluster_page_token(last_arn: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{CLUSTER_PAGE_TOKEN_PREFIX}{last_arn}"))
}

/// The ARN a token resumes after, or `None` when the token is not one a cluster
/// listing minted — which every listing refuses as a bad request rather than
/// serving as a first page.
pub fn decode_cluster_page_token(token: &str) -> Option<String> {
    let decoded = ::aws_smithy_types::base64::decode(token).ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    decoded
        .strip_prefix(CLUSTER_PAGE_TOKEN_PREFIX)
        .map(ToString::to_string)
}

impl Channel {
    /// Mint a channel of `cluster`: derive its identity from `suffix` (the
    /// registry's monotonic counter) and the cluster that holds it, and keep the
    /// submitted configuration whole.
    ///
    /// `now` is the creation instant and `input` the submitted configuration as
    /// `CreateChannel` deserialized it. The destination type is DERIVED from
    /// which destination configuration the request carried; the handler has
    /// already refused a request that carried both or neither, so a record can
    /// never describe a destination it does not hold.
    pub fn streaming(
        cluster: &Cluster,
        suffix: u64,
        now: SystemTime,
        input: CreateChannelInput,
    ) -> Self {
        let name = input.channel_name.as_str();
        // The registry's counter, as a 1-based ordinal; the SAME value seeds both
        // digests below, so a channel's id and its create operation's id follow
        // from one number.
        let ordinal = suffix + 1;
        let channel_id = uuid_v4_shaped_from_digest(&stable_digest(&format!(
            "{}:{}:channel:{name}:{ordinal}",
            ::engine::emulator_account_id(),
            cluster.arn
        )));
        let operation_id = uuid_v4_shaped_from_digest(&stable_digest(&format!(
            "{}:{}:channel-operation:{name}:{ordinal}",
            ::engine::emulator_account_id(),
            cluster.arn
        )));
        // The cluster's own resource id, so a channel ARN names the cluster it
        // belongs to the way an MSK sub-resource ARN does.
        let cluster_id = cluster.arn.split('/').next_back().unwrap_or_default();
        let destination_type = if input.iceberg_destination_configuration.is_some() {
            DESTINATION_TYPE_ICEBERG
        } else {
            DESTINATION_TYPE_S3
        };

        Self {
            arn: ::engine::emulator_arn(
                "kafka",
                &cluster.region,
                &format!("channel/{}/{cluster_id}/{name}/{channel_id}", cluster.name),
            ),
            name: input.channel_name,
            cluster_arn: cluster.arn.clone(),
            region: cluster.region.clone(),
            creation_time: now,
            active_due: now + CHANNEL_ACTIVATION_DELAY,
            status: "CREATING".to_string(),
            destination_type: destination_type.to_string(),
            encryption_configuration: input.encryption_configuration,
            iceberg_destination_configuration: input.iceberg_destination_configuration,
            s3_destination_configuration: input.s3_destination_configuration,
            topic_configuration_list: input.topic_configuration_list,
            logging_info: input.logging_info,
            tags: input.tags.unwrap_or_default().into_iter().collect(),
            cluster_operation_arn: ::engine::emulator_arn(
                "kafka",
                &cluster.region,
                &format!(
                    "cluster-operation/{}/{cluster_id}/{operation_id}",
                    cluster.name
                ),
            ),
        }
    }

    /// Apply the `CREATING` -> `ACTIVE` transition once `now` has reached
    /// [`Self::active_due`]. Idempotent, and a no-op for every other status: only
    /// a transition the clock already owed is materialised here.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == "CREATING" && now >= self.active_due {
            self.status = "ACTIVE".to_string();
        }
    }

    /// The channel as `DescribeChannel` reports it. Kept next to the record for
    /// the same reason [`Cluster::to_cluster_info`] is: every reader of a channel
    /// renders the one stored record the same way.
    pub fn to_describe_output(&self) -> DescribeChannelOutput {
        DescribeChannelOutput {
            channel_arn: self.arn.clone(),
            channel_name: self.name.clone(),
            encryption_configuration: self.encryption_configuration.clone(),
            iceberg_destination_configuration: self.iceberg_destination_configuration.clone(),
            s3_destination_configuration: self.s3_destination_configuration.clone(),
            status: self.status.clone(),
            destination_type: self.destination_type.clone(),
            creation_time: ::aws_smithy_types::DateTime::from(self.creation_time),
            topic_configuration_list: self.topic_configuration_list.clone(),
            logging_info: self.logging_info.clone(),
            // `ChannelStateInfo` reports why a channel is in a state that needs
            // explaining; nothing has gone wrong with a channel this emulator
            // creates, so there is none to report.
            state_info: None,
            cluster_operation_arn: Some(self.cluster_operation_arn.clone()),
            tags: Some(self.tags.clone().into_iter().collect()),
        }
    }
}

impl Cluster {
    /// Mint a provisioned cluster: resolve the submitted configuration's defaults,
    /// derive the cluster's identity from `suffix` (the registry's monotonic
    /// counter), and lay out its broker and ZooKeeper nodes.
    ///
    /// `region` is the caller's signing region, `suffix` the registry's monotonic
    /// counter (used as the 1-based ordinal `suffix + 1` throughout, so one value
    /// seeds this cluster's whole identity), `now` the creation instant, and
    /// `input` the submitted configuration as `CreateCluster` deserialized it.
    ///
    /// `input.number_of_broker_nodes` is CLAMPED to
    /// [`MAX_BROKER_NODES_PER_CLUSTER`] (and to at least one), so a request value
    /// that somehow reached here past the model's own 1..15 range cannot size the
    /// allocation below.
    pub fn provisioned(
        region: &str,
        suffix: u64,
        now: SystemTime,
        input: CreateClusterInput,
    ) -> Self {
        let name = input.cluster_name.as_str();
        // The registry's counter, as the 1-based ordinal AWS puts at the end of a
        // cluster ARN. The SAME value seeds the digest, so a cluster's uuid, DNS
        // suffix and ARN tail all follow from one number.
        let ordinal = suffix + 1;
        let digest = stable_digest(&format!(
            "{}:{region}:{name}:{ordinal}",
            ::engine::emulator_account_id()
        ));
        let cluster_uuid = uuid_v4_shaped_from_digest(&digest);
        // The DNS suffix of an MSK endpoint is a short opaque cluster id plus the
        // cell the cluster landed in: `b-1.demo.a1b2c3.c4.kafka.<region>...`.
        let dns_suffix = format!(
            "{:02x}{:02x}{:02x}.c{}",
            digest[16],
            digest[17],
            digest[18],
            1 + u32::from(digest[19] % 9)
        );
        let arn = ::engine::emulator_arn(
            "kafka",
            region,
            &format!("cluster/{name}/{cluster_uuid}-{ordinal}"),
        );

        // CLAMPED, not trusted: the model's own 1..15 range already refuses an
        // out-of-range request at the boundary, but this constructor must not let
        // any caller size a heap allocation from an unchecked value.
        let broker_count = input
            .number_of_broker_nodes
            .clamp(1, MAX_BROKER_NODES_PER_CLUSTER);
        let mut brokers = Vec::with_capacity(usize::try_from(broker_count).unwrap_or(0));
        for id in 1..=broker_count {
            brokers.push(BrokerNode {
                id,
                endpoint_host: format!("b-{id}.{name}.{dns_suffix}.kafka.{region}.amazonaws.com"),
            });
        }
        let zookeeper_hosts: Vec<String> = (1..=ZOOKEEPER_NODE_COUNT)
            .map(|id| format!("z-{id}.{name}.{dns_suffix}.kafka.{region}.amazonaws.com"))
            .collect();

        Self {
            current_version: current_version_token(&arn, 1),
            arn,
            name: input.cluster_name,
            region: region.to_string(),
            cluster_type: CLUSTER_TYPE_PROVISIONED.to_string(),
            kafka_version: input.kafka_version,
            creation_time: now,
            active_due: now + CLUSTER_ACTIVATION_DELAY,
            state: "CREATING".to_string(),
            // The stored count is the one the broker list was built from, so the
            // record can never describe more brokers than it holds.
            number_of_broker_nodes: broker_count,
            broker_node_group_info: input.broker_node_group_info,
            client_authentication: input.client_authentication,
            encryption_info: resolve_encryption_info(region, input.encryption_info),
            enhanced_monitoring: input
                .enhanced_monitoring
                .unwrap_or_else(|| DEFAULT_ENHANCED_MONITORING.to_string()),
            open_monitoring: input.open_monitoring.unwrap_or_default(),
            logging_info: input.logging_info,
            storage_mode: input
                .storage_mode
                .unwrap_or_else(|| DEFAULT_STORAGE_MODE.to_string()),
            configuration_info: input.configuration_info,
            rebalancing: input.rebalancing,
            tags: input.tags.unwrap_or_default().into_iter().collect(),
            zookeeper_connect_string: join_endpoints(
                zookeeper_hosts.iter().map(String::as_str),
                PORT_ZOOKEEPER,
            ),
            zookeeper_connect_string_tls: join_endpoints(
                zookeeper_hosts.iter().map(String::as_str),
                PORT_ZOOKEEPER_TLS,
            ),
            brokers,
            // A provisioned cluster comes up with ZooKeeper access on; only
            // `UpdateConnectivity` turns it off.
            zookeeper_access_enabled: true,
            // Revision 1 is the configuration the create submitted, and
            // `current_version` above is that revision's token.
            revision: 1,
            pending_update: None,
        }
    }

    /// Apply the transitions the clock already owes this cluster: `CREATING` ->
    /// `ACTIVE` once `now` has reached [`Self::active_due`], and the completion
    /// of an in-flight update once `now` has reached its
    /// [`PendingUpdate::due`]. Idempotent, and a no-op for every other state:
    /// nothing here depends on who called or when they called it, only on the
    /// instants already stored.
    pub fn settle(&mut self, now: SystemTime) {
        if self.state == "CREATING" && now >= self.active_due {
            self.state = "ACTIVE".to_string();
        }
        match self.pending_update.take() {
            // An update completes only for a cluster that is still `UPDATING`
            // because of it. Any other state means something else now owns the
            // record, so the change is dropped rather than folded into a
            // cluster that has moved on.
            Some(update) if now >= update.due && self.state == "UPDATING" => {
                self.apply_change(update.change);
                // "When this update operation is successful, it generates a new
                // cluster version" (`CurrentVersion` on every update request),
                // so the version token follows the new revision.
                self.revision += 1;
                self.current_version = current_version_token(&self.arn, self.revision);
                self.state = "ACTIVE".to_string();
            }
            // Due but no longer this cluster's business: dropped, changing
            // nothing.
            Some(dropped) if now >= dropped.due => {}
            // Still in flight: put it back exactly as it was.
            pending => self.pending_update = pending,
        }
    }

    /// Start a connectivity update: the cluster enters `UPDATING` with the
    /// returned operation as its active one, and the submitted settings become
    /// its own when [`Self::settle`] finds the operation due.
    ///
    /// Nothing the readers report changes yet — the real service accepts the
    /// call, hands back a cluster operation, and reports the new configuration
    /// and the new cluster version only once that operation finishes.
    ///
    /// A submitted `ConnectivityInfo` REPLACES the cluster's broker access
    /// configuration (the request carries the whole shape, so a member it omits
    /// falls back to that member's default), while an omitted one leaves the
    /// configuration alone; `ZookeeperAccess` is independent of it.
    pub fn begin_connectivity_update(
        &mut self,
        now: SystemTime,
        connectivity_info: Option<ConnectivityInfo>,
        zookeeper_access: Option<ZookeeperAccess>,
    ) -> String {
        let operation_arn = self.operation_arn(self.revision + 1);
        self.pending_update = Some(PendingUpdate {
            operation_arn: operation_arn.clone(),
            due: now + CLUSTER_UPDATE_DELAY,
            change: ClusterChange::Connectivity {
                connectivity_info,
                zookeeper_access,
            },
        });
        self.state = "UPDATING".to_string();
        operation_arn
    }

    /// Fold a completed operation's change into the stored record, so every
    /// reader derives the cluster's new behaviour from one place.
    fn apply_change(&mut self, change: ClusterChange) {
        match change {
            ClusterChange::Connectivity {
                connectivity_info,
                zookeeper_access,
            } => {
                if let Some(info) = connectivity_info {
                    self.broker_node_group_info.connectivity_info = Some(info);
                }
                if let Some(access) = zookeeper_access {
                    // `ZookeeperAccess$Enabled` is optional; a request that
                    // names the shape without it leaves access on, the setting a
                    // cluster is provisioned with.
                    self.zookeeper_access_enabled = access.enabled.unwrap_or(true);
                }
            }
        }
    }

    /// The ARN of the cluster operation that takes this cluster to `revision`:
    /// `arn:<partition>:kafka:<region>:<account>:cluster-operation/<name>/<cluster
    /// id>/<operation id>` — the cluster's own identity followed by the
    /// operation's id, the form an MSK cluster-operation ARN takes. A pure
    /// function of the cluster and the revision it produces, so one operation
    /// keeps one ARN however often it is reported.
    fn operation_arn(&self, revision: u64) -> String {
        let cluster_id = self.arn.rsplit_once('/').map_or("", |(_, id)| id);
        let digest = stable_digest(&format!("{}#operation#{revision}", self.arn));
        ::engine::emulator_arn(
            "kafka",
            &self.region,
            &format!(
                "cluster-operation/{}/{cluster_id}/{}",
                self.name,
                uuid_v4_shaped_from_digest(&digest)
            ),
        )
    }

    /// The cluster as `ClusterInfo`, the shape both `DescribeCluster` and
    /// `ListClusters` report.
    pub fn to_cluster_info(&self) -> ClusterInfo {
        ClusterInfo {
            // The operation in flight on the cluster, while one is: this is the
            // ARN the update call that started it answered with.
            active_operation_arn: self
                .pending_update
                .as_ref()
                .map(|update| update.operation_arn.clone()),
            broker_node_group_info: Some(self.broker_node_group_info.clone()),
            rebalancing: self.rebalancing.clone(),
            client_authentication: self.client_authentication.clone(),
            cluster_arn: Some(self.arn.clone()),
            cluster_name: Some(self.name.clone()),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            current_broker_software_info: Some(self.broker_software_info()),
            current_version: Some(self.current_version.clone()),
            encryption_info: Some(self.encryption_info.clone()),
            enhanced_monitoring: Some(self.enhanced_monitoring.clone()),
            open_monitoring: Some(open_monitoring_report(&self.open_monitoring)),
            logging_info: self.logging_info.clone(),
            number_of_broker_nodes: Some(self.number_of_broker_nodes),
            state: Some(self.state.clone()),
            state_info: None,
            tags: Some(self.tags.clone().into_iter().collect()),
            zookeeper_connect_string: Some(self.zookeeper_connect_string.clone()),
            zookeeper_connect_string_tls: Some(self.zookeeper_connect_string_tls.clone()),
            storage_mode: Some(self.storage_mode.clone()),
            // No customer action is pending on a cluster this emulator provisions.
            customer_action_status: Some("NONE".to_string()),
        }
    }

    /// The cluster as the `Cluster` shape, which is what the V2 readers
    /// (`DescribeClusterV2`, `ListClustersV2`) report: the identity members every
    /// cluster kind shares, plus the one per-kind member its `ClusterType` names.
    ///
    /// The per-kind member is derived from the SAME stored record `ClusterInfo` is
    /// built from, so the V1 and V2 views of one cluster cannot disagree.
    pub fn to_cluster_v2(&self) -> crate::ops::types::Cluster {
        crate::ops::types::Cluster {
            active_operation_arn: None,
            cluster_type: Some(self.cluster_type.clone()),
            cluster_arn: Some(self.arn.clone()),
            cluster_name: Some(self.name.clone()),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            current_version: Some(self.current_version.clone()),
            state: Some(self.state.clone()),
            state_info: None,
            tags: Some(self.tags.clone().into_iter().collect()),
            // Exactly one of the per-kind members is reported, the one this
            // cluster's `ClusterType` names. Only the provisioned create path is
            // implemented, so a record of any other kind would carry neither
            // rather than being reported as provisioned.
            provisioned: (self.cluster_type == CLUSTER_TYPE_PROVISIONED)
                .then(|| self.to_provisioned()),
            serverless: None,
        }
    }

    /// The provisioned-cluster detail of the `Cluster` shape: the brokers, the
    /// software, the encryption, monitoring and logging settings, and the
    /// ZooKeeper connect strings — the same values `ClusterInfo` carries inline.
    fn to_provisioned(&self) -> Provisioned {
        Provisioned {
            broker_node_group_info: self.broker_node_group_info.clone(),
            rebalancing: self.rebalancing.clone(),
            current_broker_software_info: Some(self.broker_software_info()),
            client_authentication: self.client_authentication.clone(),
            encryption_info: Some(self.encryption_info.clone()),
            enhanced_monitoring: Some(self.enhanced_monitoring.clone()),
            open_monitoring: Some(open_monitoring_effective(&self.open_monitoring)),
            logging_info: self.logging_info.clone(),
            number_of_broker_nodes: self.number_of_broker_nodes,
            zookeeper_connect_string: Some(self.zookeeper_connect_string.clone()),
            zookeeper_connect_string_tls: Some(self.zookeeper_connect_string_tls.clone()),
            storage_mode: Some(self.storage_mode.clone()),
            // No customer action is pending on a cluster this emulator provisions.
            customer_action_status: Some("NONE".to_string()),
        }
    }

    /// The Apache Kafka version deployed on the brokers, and the configuration
    /// revision they run, as the `BrokerSoftwareInfo` every reader reports.
    fn broker_software_info(&self) -> BrokerSoftwareInfo {
        BrokerSoftwareInfo {
            configuration_arn: self.configuration_info.as_ref().map(|c| c.arn.clone()),
            configuration_revision: self.configuration_info.as_ref().map(|c| c.revision),
            kafka_version: Some(self.kafka_version.clone()),
        }
    }

    /// Every broker's endpoint on `port`, comma-separated in broker-id order —
    /// the form a Kafka client's `bootstrap.servers` takes.
    pub fn broker_endpoints(&self, port: u16) -> String {
        join_endpoints(
            self.brokers
                .iter()
                .map(|broker| broker.endpoint_host.as_str()),
            port,
        )
    }

    /// The effective `EncryptionInTransit$ClientBroker` setting.
    pub fn client_broker(&self) -> &str {
        self.encryption_info
            .encryption_in_transit
            .as_ref()
            .and_then(|t| t.client_broker.as_deref())
            .unwrap_or(DEFAULT_CLIENT_BROKER)
    }

    /// Whether clients may connect in plaintext (`PLAINTEXT`/`TLS_PLAINTEXT`).
    pub fn plaintext_enabled(&self) -> bool {
        matches!(self.client_broker(), "PLAINTEXT" | "TLS_PLAINTEXT")
    }

    /// Whether clients may connect over TLS (`TLS`/`TLS_PLAINTEXT`).
    pub fn tls_enabled(&self) -> bool {
        matches!(self.client_broker(), "TLS" | "TLS_PLAINTEXT")
    }

    /// Whether SASL/SCRAM client authentication is turned on.
    pub fn sasl_scram_enabled(&self) -> bool {
        self.client_authentication
            .as_ref()
            .and_then(|auth| auth.sasl.as_ref())
            .and_then(|sasl| sasl.scram.as_ref())
            .and_then(|scram| scram.enabled)
            .unwrap_or(false)
    }

    /// Whether SASL/IAM client authentication is turned on.
    pub fn sasl_iam_enabled(&self) -> bool {
        self.client_authentication
            .as_ref()
            .and_then(|auth| auth.sasl.as_ref())
            .and_then(|sasl| sasl.iam.as_ref())
            .and_then(|iam| iam.enabled)
            .unwrap_or(false)
    }

    /// Whether the cluster carries service-provided EIPs, i.e. public access.
    pub fn public_access_enabled(&self) -> bool {
        self.broker_node_group_info
            .connectivity_info
            .as_ref()
            .and_then(|c| c.public_access.as_ref())
            .and_then(|p| p.r#type.as_deref())
            .is_some_and(|t| t == "SERVICE_PROVIDED_EIPS")
    }

    /// Whether multi-VPC connectivity accepts TLS (mTLS) clients.
    pub fn vpc_connectivity_tls_enabled(&self) -> bool {
        self.vpc_connectivity_auth()
            .and_then(|auth| auth.tls.as_ref())
            .and_then(|tls| tls.enabled)
            .unwrap_or(false)
    }

    /// Whether multi-VPC connectivity accepts SASL/SCRAM clients.
    pub fn vpc_connectivity_sasl_scram_enabled(&self) -> bool {
        self.vpc_connectivity_auth()
            .and_then(|auth| auth.sasl.as_ref())
            .and_then(|sasl| sasl.scram.as_ref())
            .and_then(|scram| scram.enabled)
            .unwrap_or(false)
    }

    /// Whether multi-VPC connectivity accepts SASL/IAM clients.
    pub fn vpc_connectivity_sasl_iam_enabled(&self) -> bool {
        self.vpc_connectivity_auth()
            .and_then(|auth| auth.sasl.as_ref())
            .and_then(|sasl| sasl.iam.as_ref())
            .and_then(|iam| iam.enabled)
            .unwrap_or(false)
    }

    /// Whether the cluster is dual-stack, i.e. also reachable over IPv6.
    pub fn dual_stack(&self) -> bool {
        self.broker_node_group_info
            .connectivity_info
            .as_ref()
            .and_then(|c| c.network_type.as_deref())
            .unwrap_or(DEFAULT_NETWORK_TYPE)
            == "DUAL"
    }

    fn vpc_connectivity_auth(
        &self,
    ) -> Option<&crate::ops::types::VpcConnectivityClientAuthentication> {
        self.broker_node_group_info
            .connectivity_info
            .as_ref()
            .and_then(|c| c.vpc_connectivity.as_ref())
            .and_then(|v| v.client_authentication.as_ref())
    }
}

/// `<host>:<port>` for each host, comma-separated. The hosts are borrowed, so
/// rendering a bootstrap string does not clone the stored endpoint list.
fn join_endpoints<'a>(hosts: impl Iterator<Item = &'a str>, port: u16) -> String {
    hosts
        .map(|host| format!("{host}:{port}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// The submitted `EncryptionInfo` with every default resolved, so the stored
/// record describes the settings the cluster actually runs with.
fn resolve_encryption_info(region: &str, submitted: Option<EncryptionInfo>) -> EncryptionInfo {
    let submitted = submitted.unwrap_or_default();
    let in_transit = submitted.encryption_in_transit.unwrap_or_default();
    EncryptionInfo {
        encryption_at_rest: Some(submitted.encryption_at_rest.unwrap_or_else(|| {
            EncryptionAtRest {
                data_volume_kms_key_id: default_kafka_kms_key_arn(region),
            }
        })),
        encryption_in_transit: Some(EncryptionInTransit {
            client_broker: Some(
                in_transit
                    .client_broker
                    .unwrap_or_else(|| DEFAULT_CLIENT_BROKER.to_string()),
            ),
            // Documented default of `EncryptionInTransit$InCluster`: true.
            in_cluster: Some(in_transit.in_cluster.unwrap_or(true)),
        }),
    }
}

/// The AWS-managed KMS key MSK encrypts data volumes with when the request names
/// none. One key per region, as an AWS-managed key is.
fn default_kafka_kms_key_arn(region: &str) -> String {
    let digest = stable_digest(&format!(
        "{}:{region}:aws/kafka",
        ::engine::emulator_account_id()
    ));
    ::engine::emulator_arn(
        "kms",
        region,
        &format!("key/{}", uuid_v4_shaped_from_digest(&digest)),
    )
}

/// The `OpenMonitoringInfo` a request submits, as the `OpenMonitoring` shape a
/// reader reports. The two carry the same Prometheus exporter switches under
/// different shape names, and an exporter nobody asked for is off.
fn open_monitoring_report(info: &OpenMonitoringInfo) -> OpenMonitoring {
    OpenMonitoring {
        prometheus: Prometheus {
            jmx_exporter: Some(JmxExporter {
                enabled_in_broker: info
                    .prometheus
                    .jmx_exporter
                    .as_ref()
                    .is_some_and(|e| e.enabled_in_broker),
            }),
            node_exporter: Some(NodeExporter {
                enabled_in_broker: info
                    .prometheus
                    .node_exporter
                    .as_ref()
                    .is_some_and(|e| e.enabled_in_broker),
            }),
        },
    }
}

/// The stored `OpenMonitoringInfo` with every exporter switch resolved, which is
/// the shape the `Provisioned` detail reports. Same normalisation as
/// [`open_monitoring_report`] in the shape a request submits: an exporter nobody
/// asked for is reported off rather than absent, so a reader never leaves the
/// caller guessing which default applied.
fn open_monitoring_effective(info: &OpenMonitoringInfo) -> OpenMonitoringInfo {
    OpenMonitoringInfo {
        prometheus: PrometheusInfo {
            jmx_exporter: Some(JmxExporterInfo {
                enabled_in_broker: info
                    .prometheus
                    .jmx_exporter
                    .as_ref()
                    .is_some_and(|e| e.enabled_in_broker),
            }),
            node_exporter: Some(NodeExporterInfo {
                enabled_in_broker: info
                    .prometheus
                    .node_exporter
                    .as_ref()
                    .is_some_and(|e| e.enabled_in_broker),
            }),
        },
    }
}

/// 32 digest bytes for `seed`, from the hasher the standard library ships
/// (`std::hash`), used four times with the output word's index mixed in so the
/// four 64-bit words are independent functions of the seed rather than four
/// copies of one.
///
/// This is an IDENTIFIER seed, not a security primitive: what the ids below need
/// is that one seed always maps to one id within a running emulator and that
/// distinct seeds map apart, which is exactly what a hasher gives. No
/// cryptographic claim is made and none is needed — nothing here authenticates
/// anything — and no hashing crate is pulled in for it. Raw bytes rather than a
/// hex string, so a consumer that folds a byte into an alphabet draws on all 256
/// values instead of the 16 a hex digit can take.
fn stable_digest(seed: &str) -> [u8; 32] {
    use std::hash::{Hash, Hasher};

    let mut digest = [0u8; 32];
    for (index, word) in digest.chunks_mut(8).enumerate() {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        index.hash(&mut hasher);
        seed.hash(&mut hasher);
        word.copy_from_slice(&hasher.finish().to_be_bytes());
    }
    digest
}

/// The first 16 digest bytes rendered as a UUID: the RFC 4122 version (4) and
/// variant bits are set and the 8-4-4-4-12 form is written out, so an id this
/// emulator mints is SHAPED exactly like the one the real service puts in an MSK
/// ARN. The name says shaped rather than random because the bytes come from the
/// digest, so the id stays a pure function of its seed.
fn uuid_v4_shaped_from_digest(digest: &[u8; 32]) -> String {
    use std::fmt::Write;

    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 4122 §4.4: the version goes in the high nibble of byte 6 and the
    // variant in the two high bits of byte 8.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let mut rendered = String::with_capacity(36);
    for (index, byte) in bytes.iter().enumerate() {
        // The hyphens of the 8-4-4-4-12 grouping fall after bytes 4, 6, 8 and 10.
        if matches!(index, 4 | 6 | 8 | 10) {
            rendered.push('-');
        }
        // Infallible: the only error a `fmt::Write` to a `String` can report is
        // one the formatter itself raises, and `{:02x}` raises none.
        let _ = write!(rendered, "{byte:02x}");
    }
    rendered
}

/// An MSK `CurrentVersion` token: `K` followed by 13 upper-case alphanumerics,
/// the form the real service's cluster version takes. It is a function of the
/// cluster ARN and its revision, so it changes exactly when the cluster does.
fn current_version_token(arn: &str, revision: u64) -> String {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let digest = stable_digest(&format!("{arn}#{revision}"));
    let mut token = String::with_capacity(14);
    token.push('K');
    for byte in digest.iter().take(13) {
        token.push(ALPHABET[usize::from(*byte) % ALPHABET.len()] as char);
    }
    token
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsKafkaKafkaState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsKafkaKafkaState>)
}

/// The signing region of the request being served, which is the region every ARN
/// and endpoint this service mints belongs to.
///
/// The region travels in the SigV4 credential scope. A request that carries no
/// usable `Authorization` header is served in `us-east-1`, the region a generated
/// emulator defaults to.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| "us-east-1".to_string())
}

/// Whether `value` is shaped like an MSK cluster ARN —
/// `arn:<partition>:kafka:<region>:<account>:cluster/<name>/<id>`.
///
/// A reader that models both errors answers `BadRequestException` for a value
/// that is not an ARN of this kind, and reserves `NotFoundException` for a
/// well-formed ARN no live cluster holds: that is the distinction the real
/// service draws between a request it cannot parse and a resource it cannot find.
pub fn is_cluster_arn(value: &str) -> bool {
    let Some(resource) = kafka_arn_resource(value) else {
        return false;
    };
    let mut segments = resource.split('/');
    matches!(
        (
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next()
        ),
        (Some("cluster"), Some(name), Some(id), None) if !name.is_empty() && !id.is_empty()
    )
}

/// Whether `value` is shaped like an MSK channel ARN —
/// `arn:<partition>:kafka:<region>:<account>:channel/<cluster>/<cluster-id>/<name>/<id>`,
/// the form [`Channel::streaming`] mints: a channel is a sub-resource of the
/// cluster it reads from, so its ARN names that cluster.
///
/// Same division of labour as [`is_cluster_arn`]: a value that is not an ARN of
/// this kind is a `BadRequestException`, and `NotFoundException` is reserved for
/// a well-formed ARN no live channel holds.
pub fn is_channel_arn(value: &str) -> bool {
    let Some(resource) = kafka_arn_resource(value) else {
        return false;
    };
    let mut segments = resource.split('/');
    matches!(
        (
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next()
        ),
        (
            Some("channel"),
            Some(cluster),
            Some(cluster_id),
            Some(name),
            Some(id),
            None,
        ) if !cluster.is_empty()
            && !cluster_id.is_empty()
            && !name.is_empty()
            && !id.is_empty()
    )
}

/// The resource part of `value` when it is an ARN of THIS service in some
/// partition, region and account — `arn:<partition>:kafka:<region>:<account>:`
/// followed by the resource — and `None` otherwise. The one ARN parse the
/// `is_*_arn` predicates share, so they cannot disagree about the five fields in
/// front of the resource.
fn kafka_arn_resource(value: &str) -> Option<&str> {
    let mut fields = value.splitn(6, ':');
    let (
        Some("arn"),
        Some(partition),
        Some("kafka"),
        Some(region),
        Some(account),
        Some(resource),
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return None;
    };
    if partition.is_empty() || region.is_empty() || account.is_empty() {
        return None;
    }
    Some(resource)
}
