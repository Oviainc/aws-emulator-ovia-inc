// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-kafka-kafka service.
//!
//! The provisioned-cluster registry lives here, together with the renderings the
//! reporting operations derive from it: `DescribeCluster` / `ListClusters` build a
//! [`ClusterInfo`] with [`Cluster::to_cluster_info`], `DescribeClusterV2` /
//! `ListClustersV2` build the v2 `Cluster` with [`Cluster::to_cluster_v2`], and
//! `GetBootstrapBrokers` builds its endpoint strings with
//! [`Cluster::broker_endpoints`] plus the access predicates beside it. Keeping them
//! next to the record is what makes the readers agree: they render the same stored
//! cluster the same way.
//!
//! The topic registry lives beside it, keyed by (cluster ARN, topic name): a topic
//! belongs to exactly one cluster, so `CreateTopic`, `DescribeTopic`, `ListTopics`
//! and `DeleteTopic` all resolve the cluster first and then the topic, through
//! [`ComAmazonawsKafkaKafkaState::settled_topic_in`] (or
//! [`ComAmazonawsKafkaKafkaState::topics_in`] for a listing) — the one
//! accessor each, so every caller sees the transitions the clock already owed that
//! topic. A cluster leaving the registry takes its topics with it, so no topic can
//! outlive the cluster that holds it.
//!
//! A cluster's configuration also changes after creation: an update operation
//! (`UpdateConnectivity`, `UpdateStorage`, `UpdateBrokerStorage`) records a
//! [`PendingUpdate`] on the cluster, and [`Cluster::settle`] folds it into the
//! record once the clock owes its completion — so one place decides what every
//! reader then reports. The operation itself is kept in the cluster-operation
//! registry, which is what its `ClusterOperationArn` names and what
//! `DescribeClusterOperation` renders with
//! [`ClusterOperation::to_cluster_operation_info`].
//!
//! The channel registry lives beside it, keyed by channel ARN and indexed by
//! (cluster ARN, channel name) — a channel is a sub-resource of the cluster it
//! streams from, so it is only ever resolved through that cluster and disappears
//! with it.
//!
//! The replicator registry sits beside them on the same principle: a [`Replicator`]
//! holds the whole replication setup, and the readers derive their shapes from that
//! one record — [`Replicator::to_replicator_summary`] for `ListReplicators`, and
//! [`Replicator::kafka_cluster_descriptions`] /
//! [`Replicator::replication_info_descriptions`] for `DescribeReplicator`.
//!
//! The service's Apache Kafka version catalog lives here too
//! ([`KAFKA_VERSION_CATALOG`], with [`compatible_target_versions`] and
//! [`kafka_version_order`]), for the same reason: it is the single list every
//! version-aware operation answers from.
//!
//! The multi-VPC (PrivateLink) connection registry lives here too:
//! `CreateVpcConnection` mints a [`VpcConnection`] with
//! [`VpcConnection::requested`] and `DescribeVpcConnection` reports that same
//! stored record.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Bound;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    BrokerEBSVolumeInfo, BrokerNodeGroupInfo, BrokerSoftwareInfo, ChannelLoggingInfo,
    ClientAuthentication, ClusterInfo, ClusterOperationInfo, ConfigurationInfo, ConnectivityInfo,
    CreateChannelInput, CreateClusterInput, CreateReplicatorInput, CreateVpcConnectionInput,
    DescribeChannelOutput, EBSStorageInfo, EncryptionAtRest, EncryptionConfiguration,
    EncryptionInTransit, EncryptionInfo, IcebergDestinationConfiguration, JmxExporter,
    JmxExporterInfo, KafkaCluster, KafkaClusterDescription, KafkaClusterSummary, LogDelivery,
    LoggingInfo, MutableClusterInfo, NodeExporter, NodeExporterInfo, OpenMonitoring,
    OpenMonitoringInfo, Prometheus, PrometheusInfo, Provisioned, ProvisionedThroughput,
    Rebalancing, ReplicationInfo, ReplicationInfoDescription, ReplicationInfoSummary,
    ReplicatorSummary, S3DestinationConfiguration, StorageInfo, TopicConfiguration, TopicInfo,
    ZookeeperAccess,
};

/// How long after `CreateCluster` a new cluster becomes `ACTIVE`.
///
/// CHOSEN, not documented: the real service takes many minutes to provision
/// brokers. The project fixes an emulated clock-owed transition at 250–750 ms, so
/// a caller that describes the cluster immediately still observes `CREATING` (and
/// the state errors that go with it) while a caller that waits sees `ACTIVE`.
pub const CLUSTER_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// The `KafkaBrokerNodeId` a broker-storage update must name, per
/// `BrokerEBSVolumeInfo`'s model documentation: "The broker identifier must be set
/// to the keyword ALL. This means the changes apply to all the brokers in the
/// cluster."
pub const ALL_BROKERS: &str = "ALL";

/// The smallest EBS data-volume size a broker may be given, from the `range` on
/// `EBSStorageInfo$VolumeSize` (`__integerMin1Max16384`, 1..16384) in the model.
pub const MIN_VOLUME_SIZE_GB: i32 = 1;

/// The largest EBS data-volume size a broker may be given, from the same `range`.
///
/// `UpdateStorageRequest$VolumeSizeGB` is a plain `__integer`, so the boundary
/// validator lets any value through: the range belongs to the stored
/// `EBSStorageInfo$VolumeSize` the update writes, and the handler is what
/// enforces it.
pub const MAX_VOLUME_SIZE_GB: i32 = 16_384;

/// How many cluster operations the registry keeps for one cluster, newest first.
///
/// CHOSEN as a process-safety backstop, not an AWS quota: every update call adds
/// one operation record, so an unbounded history would grow the store with the
/// number of calls. Sized well past any realistic client script's update count;
/// a caller holding an ARN evicted past it reads the same `NotFoundException` as
/// for an operation that never existed.
pub const MAX_OPERATIONS_PER_CLUSTER: usize = 64;

/// The `OperationType` an `UpdateStorage` call records, which is the API name in
/// the SCREAMING_SNAKE_CASE form MSK's cluster-operation types take.
pub const OPERATION_TYPE_UPDATE_STORAGE: &str = "UPDATE_STORAGE";

/// The `OperationType` an `UpdateBrokerStorage` call records, in the same form.
pub const OPERATION_TYPE_UPDATE_BROKER_STORAGE: &str = "UPDATE_BROKER_STORAGE";

/// The state MSK reports for a cluster operation that is still running.
pub const OPERATION_STATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";

/// The state MSK reports for a cluster operation that has finished.
pub const OPERATION_STATE_COMPLETE: &str = "UPDATE_COMPLETE";

/// The `ClusterState` of a cluster whose brokers are serving and which accepts an
/// update. One of the model's `ClusterState` values
/// (`crate::ops::types::CLUSTER_STATE_VALUES`), as are the two below.
pub const CLUSTER_STATE_ACTIVE: &str = "ACTIVE";

/// The `ClusterState` of a cluster that is being provisioned.
pub const CLUSTER_STATE_CREATING: &str = "CREATING";

/// The `ClusterState` of a cluster an accepted update is still being applied to.
pub const CLUSTER_STATE_UPDATING: &str = "UPDATING";

/// How long a cluster update (`UpdateConnectivity`, `UpdateStorage`,
/// `UpdateBrokerStorage` and the
/// sibling update operations) stays in flight before the cluster leaves
/// `UPDATING`.
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

/// How long after `CreateVpcConnection` a new VPC connection becomes
/// `AVAILABLE`.
///
/// CHOSEN, not documented: the real service takes minutes to place the
/// PrivateLink endpoint. The project fixes an emulated clock-owed transition at
/// 250–750 ms, so a caller that describes the connection immediately still
/// observes `CREATING` while a caller that waits sees `AVAILABLE`.
pub const VPC_CONNECTION_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// Absolute ceiling on the VPC-connection registry.
///
/// CHOSEN as a process-safety backstop, not an AWS quota: the registry must not
/// grow without bound with the number of calls a client makes. Sized far above
/// anything a realistic client script creates, so it can only be reached by a
/// caller trying to exhaust the emulator.
pub const MAX_VPC_CONNECTIONS_IN_REGISTRY: usize = 1024;

/// The authentication types an MSK VPC connection may use, from Amazon MSK's
/// multi-VPC private connectivity documentation: a connection authenticates with
/// SASL/IAM, SASL/SCRAM or TLS. `CreateVpcConnectionRequest$Authentication` is a
/// plain `__string` in the model, so the handler is what refuses anything else.
pub const VPC_CONNECTION_AUTHENTICATION_TYPES: [&str; 3] = ["SASL_IAM", "SASL_SCRAM", "TLS"];

/// How long after `CreateTopic` a new topic becomes `ACTIVE`.
///
/// CHOSEN, not documented: the real service creates the topic on the cluster's
/// brokers, which takes as long as it takes. The project fixes an emulated
/// clock-owed transition at 250–750 ms, so a caller that describes the topic
/// immediately still observes `CREATING` while one that waits sees `ACTIVE`.
pub const TOPIC_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// How long after `DeleteTopic` a topic marked `DELETING` is actually gone.
///
/// CHOSEN, not documented, for the same reason: `DeleteTopic` answers the
/// non-terminal `DELETING` status, so the record must remain readable for a span
/// before it leaves the registry.
pub const TOPIC_DELETION_DELAY: Duration = Duration::from_millis(500);

/// The longest legal Apache Kafka topic name: 249 characters
/// (`org.apache.kafka.common.internals.Topic.MAX_NAME_LENGTH`, which reserves
/// room for the `-<partition>` suffix a log directory name adds).
pub const MAX_TOPIC_NAME_LENGTH: usize = 249;

/// Topics one cluster may hold in this emulator.
///
/// CHOSEN as a process-safety backstop, not an AWS quota: Amazon MSK bounds a
/// cluster by partitions per broker rather than by topic count, and the topic
/// name comes from the request, so without a ceiling a caller could grow the
/// registry one topic at a time. Sized well above any realistic client script.
pub const MAX_TOPICS_PER_CLUSTER: usize = 1_000;

/// The `TopicState` values this emulator reports, spelled as the model's enum
/// declares them. Named here so every topic operation reports the same
/// spellings. (`UPDATING`, the model's fourth value, belongs to `UpdateTopic`,
/// which is still a scaffold.)
pub const TOPIC_STATE_CREATING: &str = "CREATING";
/// The state of a topic the cluster has finished creating.
pub const TOPIC_STATE_ACTIVE: &str = "ACTIVE";
/// The state of a topic whose deletion has been accepted but not completed.
pub const TOPIC_STATE_DELETING: &str = "DELETING";

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

/// How long after `CreateReplicator` a new replicator becomes `RUNNING`.
///
/// CHOSEN, not documented: the real service takes minutes to stand a MirrorMaker
/// fleet up. The project fixes an emulated clock-owed transition at 250–750 ms, so
/// a caller that describes the replicator immediately still observes `CREATING`
/// while a caller that waits sees `RUNNING` — the same span, and the same
/// reasoning, as [`CLUSTER_ACTIVATION_DELAY`].
pub const REPLICATOR_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// Absolute ceiling on the replicator registry.
///
/// CHOSEN as a process-safety backstop, not an AWS quota (AWS publishes no
/// per-region replicator limit this emulator could cite): the signing region comes
/// from the request, so without a ceiling a caller could grow the store by naming
/// new regions. Sized well past anything a realistic client script creates, so it
/// can only be reached by a caller trying to exhaust the emulator.
pub const MAX_REPLICATORS_IN_REGISTRY: usize = 1024;

/// The most `KafkaClusters` entries one replicator may hold.
///
/// CHOSEN, not an AWS quota (the model declares `__listOfKafkaCluster` with no
/// `length` trait): a replicator's cluster list and its replication list are both
/// caller-sized, and every reader resolves each replication against the cluster
/// list, so without ceilings on BOTH one create would size the work of every
/// later read. Sized far above any real setup — AWS Replicator documents
/// replication between a source and a target — so a realistic client never meets
/// it, while the product with [`MAX_FLOWS_PER_REPLICATOR`] stays small.
pub const MAX_CLUSTERS_PER_REPLICATOR: usize = 16;

/// The most `ReplicationInfoList` entries one replicator may hold. CHOSEN for the
/// same reason, and on the same model evidence, as
/// [`MAX_CLUSTERS_PER_REPLICATOR`]: a replicator cannot hold more distinct
/// ordered cluster pairs than this anyway.
pub const MAX_FLOWS_PER_REPLICATOR: usize = 32;

/// The most tags one replicator may carry: the general per-resource limit, which
/// [`MAX_TAGS_PER_RESOURCE`] states and every tag write is admitted against. Kept
/// as its own name because `Replicator::created` clamps with it, so the clamp and
/// the admission check cannot drift apart.
pub const MAX_TAGS_PER_REPLICATOR: usize = MAX_TAGS_PER_RESOURCE;

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

/// The message Amazon MSK answers a parameter it cannot parse with: its
/// `BadRequestException` carries one fixed, request-independent sentence rather
/// than a description of the offending value.
pub const BAD_REQUEST_MESSAGE: &str = "One or more of the parameters are not valid.";

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

/// Where `version` sits in [`KAFKA_VERSION_CATALOG`], or `None` when MSK knows
/// no such version. This is the lookup a `ListKafkaVersions` pagination token
/// resolves through, so a token can only ever name a position in the catalog
/// itself.
pub fn kafka_version_index(version: &str) -> Option<usize> {
    KAFKA_VERSION_CATALOG
        .iter()
        .position(|entry| entry.version == version)
}

/// Marker that prefixes a version page token's plaintext, so a token minted for
/// some other list cannot be mistaken for one of these.
const KAFKA_VERSION_PAGE_TOKEN_PREFIX: &str = "kafka:kafka-versions:";

/// Encode the catalog version a page stopped at as a token, through the same
/// [`encode_page_token`] every listing here mints its tokens with. The version
/// inside is only resolved against the public catalog, so a forged resume point
/// can only skip or repeat entries every caller may already list.
pub fn encode_kafka_version_page_token(last_version: &str) -> String {
    encode_page_token(KAFKA_VERSION_PAGE_TOKEN_PREFIX, last_version)
}

/// The version a token resumes after, or `None` when the token is not one the
/// version listing minted — which that listing refuses as a bad request rather
/// than serving as a first page.
pub fn decode_kafka_version_page_token(token: &str) -> Option<String> {
    decode_page_token(KAFKA_VERSION_PAGE_TOKEN_PREFIX, token)
}

/// One page of the version catalog: the entries it reports, and the token that
/// resumes after them — absent on the last page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KafkaVersionPage {
    pub entries: &'static [KafkaVersionEntry],
    pub next_token: Option<String>,
}

/// Choose one page of [`KAFKA_VERSION_CATALOG`]: the `page_size` entries that
/// follow `resume_after` (a catalog position, as
/// [`decode_kafka_version_page_token`] plus [`kafka_version_index`] resolve one),
/// or the first `page_size` entries when a request names no token.
///
/// `page_size` is clamped into the `MaxResults` range the model declares, so no
/// caller of this module can size the page from an unchecked value. Every page is
/// a contiguous run of the catalog in its published order, so the pages of any
/// walk partition the catalog exactly once.
///
/// A `resume_after` naming the LAST catalogued version yields an empty final page
/// with no token: the listing never mints a token for the last entry (the page
/// that reports it closes the list), so only a forged resume point reaches that
/// case, and an empty page is the truthful answer to "what follows the end".
pub fn kafka_version_page(resume_after: Option<usize>, page_size: usize) -> KafkaVersionPage {
    let page_size = page_size.clamp(1, MAX_RESULTS_LIMIT as usize);
    let start = resume_after
        .map_or(0, |index| index.saturating_add(1))
        .min(KAFKA_VERSION_CATALOG.len());
    let end = start
        .saturating_add(page_size)
        .min(KAFKA_VERSION_CATALOG.len());
    let entries = &KAFKA_VERSION_CATALOG[start..end];
    // The token names the entry this page stopped at — the same value the resume
    // lookup resolves — and is absent once the page reaches the end of the
    // catalog, which is what tells a paginating client to stop.
    let next_token = if end < KAFKA_VERSION_CATALOG.len() {
        entries
            .last()
            .map(|entry| encode_kafka_version_page_token(entry.version))
    } else {
        None
    };
    KafkaVersionPage {
        entries,
        next_token,
    }
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

/// The largest page a listing may report, from the `range` on the `MaxResults`
/// shape (1..100) that `ListClusters`, `ListClustersV2` and `ListKafkaVersions`
/// all bind. Also the page size used when a request names none.
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
    /// The `CurrentVersion` token an update call must quote, derived from
    /// [`Self::revision`] so it changes exactly when the cluster does.
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
    /// The ARNs of this cluster's recorded operations, oldest first. It is the
    /// cluster's own index into the operation registry, which is what keeps the
    /// history bound a walk of ONE cluster's operations rather than of every
    /// operation the registry holds.
    pub operation_arns: Vec<String>,
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
    /// `UpdateStorage` and `UpdateBrokerStorage`: the brokers' EBS volume size,
    /// their provisioned throughput, the cluster's storage mode, or any
    /// combination the request named.
    Storage(StorageChange),
}

/// The storage attributes one update call asked to change. A member left `None`
/// is one the request did not name, which the update leaves as it is.
#[derive(Debug, Clone, Default)]
pub struct StorageChange {
    /// The EBS data-volume size each broker is to be given, in GiB.
    pub volume_size_gb: Option<i32>,
    /// The EBS provisioned-throughput setting the volumes are to run with.
    pub provisioned_throughput: Option<ProvisionedThroughput>,
    /// The `StorageMode` the cluster is to run in.
    pub storage_mode: Option<String>,
}

impl StorageChange {
    /// Whether the request named no storage attribute at all, so there is
    /// nothing for an update to do.
    pub fn is_empty(&self) -> bool {
        self.volume_size_gb.is_none()
            && self.provisioned_throughput.is_none()
            && self.storage_mode.is_none()
    }
}

/// One cluster operation: the record an update call leaves behind, which is what
/// its `ClusterOperationArn` names.
#[derive(Debug, Clone)]
pub struct ClusterOperation {
    pub arn: String,
    pub cluster_arn: String,
    /// The region the operation ran in — the signing region of the call that
    /// started it, and the region segment of its ARN.
    pub region: String,
    /// A `ClusterOperationInfo$OperationType` value.
    pub operation_type: String,
    pub creation_time: SystemTime,
    /// The instant the clock owes this operation its completion. STORED, never
    /// slept on: the reported state is a function of it and the reading instant.
    pub end_due: SystemTime,
    /// The attributes this operation changes, as they were before it ran.
    pub source_cluster_info: MutableClusterInfo,
    /// The same attributes, as the operation sets them.
    pub target_cluster_info: MutableClusterInfo,
}

impl ClusterOperation {
    /// The operation's state at `now`: MSK reports a running update as
    /// `UPDATE_IN_PROGRESS` and a finished one as `UPDATE_COMPLETE`.
    pub fn operation_state(&self, now: SystemTime) -> &'static str {
        if now >= self.end_due {
            OPERATION_STATE_COMPLETE
        } else {
            OPERATION_STATE_IN_PROGRESS
        }
    }

    /// The operation as `ClusterOperationInfo`, the shape
    /// `DescribeClusterOperation` reports.
    ///
    /// `EndTime` is reported only once the operation has finished, since an
    /// operation still running has not ended.
    pub fn to_cluster_operation_info(&self, now: SystemTime) -> ClusterOperationInfo {
        ClusterOperationInfo {
            // The id of the API request that triggered the operation is the
            // caller's, and this emulator mints none.
            client_request_id: None,
            cluster_arn: Some(self.cluster_arn.clone()),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            end_time: (now >= self.end_due)
                .then(|| ::aws_smithy_types::DateTime::from(self.end_due)),
            // Nothing fails an emulated update, so there is no error to describe.
            error_info: None,
            operation_arn: Some(self.arn.clone()),
            operation_state: Some(self.operation_state(now).to_string()),
            // No step list: this emulator runs no broker-by-broker steps, and
            // reporting invented ones would describe work nothing did.
            operation_steps: None,
            operation_type: Some(self.operation_type.clone()),
            source_cluster_info: Some(self.source_cluster_info.clone()),
            target_cluster_info: Some(self.target_cluster_info.clone()),
            // A VPC connection is described only by the operations that manage
            // one.
            vpc_connection_info: None,
        }
    }
}

/// One MSK VPC connection: a client VPC's private (PrivateLink) connection to a
/// cluster, as `CreateVpcConnection` requested it. The whole submitted
/// configuration is held here so every reader computes its answer from the
/// record rather than echoing a request.
#[derive(Debug, Clone)]
pub struct VpcConnection {
    pub arn: String,
    /// The region the connection lives in: the signing region of the request
    /// that created it, and the region segment of its ARN. A regional endpoint
    /// serves only its own region's connections.
    pub region: String,
    pub target_cluster_arn: String,
    /// One of [`VPC_CONNECTION_AUTHENTICATION_TYPES`].
    pub authentication: String,
    pub vpc_id: String,
    /// The client subnets the connection reaches the cluster from, in the order
    /// the request listed them. `DescribeVpcConnection` reports them as
    /// `Subnets`, `CreateVpcConnection` as `ClientSubnets`.
    pub client_subnets: Vec<String>,
    pub security_groups: Vec<String>,
    pub creation_time: SystemTime,
    /// The instant the clock owes this connection its `CREATING` ->
    /// `AVAILABLE` transition. Stored, never slept on.
    pub available_due: SystemTime,
    /// A `VpcConnectionState` value.
    pub state: String,
    /// Ordered so every reader renders a stable map.
    pub tags: BTreeMap<String, String>,
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

    /// The cluster as the richer `KafkaClusterDescription` `DescribeReplicator`
    /// reports: the alias plus every member of the submitted `KafkaCluster`.
    pub fn to_description(&self) -> KafkaClusterDescription {
        KafkaClusterDescription {
            amazon_msk_cluster: self.cluster.amazon_msk_cluster.clone(),
            apache_kafka_cluster: self.cluster.apache_kafka_cluster.clone(),
            kafka_cluster_alias: Some(self.alias.clone()),
            vpc_config: self.cluster.vpc_config.clone(),
            client_authentication: self.cluster.client_authentication.clone(),
            encryption_in_transit: self.cluster.encryption_in_transit.clone(),
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
    /// that created it, and the region segment of its ARN. `ListReplicators` and
    /// `DescribeReplicator` are regional, so this is what scopes it to one
    /// endpoint. A replicator name is unique per account and region, so this is
    /// also half of its registry key.
    pub region: String,
    pub current_version: String,
    /// `ReplicatorDescription` — the summary description the request carried, if
    /// any.
    pub description: Option<String>,
    /// The IAM role the replicator uses to reach the source and target clusters.
    pub service_execution_role_arn: String,
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
    /// Where the replicator's own logs are delivered, as submitted.
    pub log_delivery: Option<LogDelivery>,
    /// Ordered so a reader renders a stable map.
    pub tags: BTreeMap<String, String>,
}

impl Replicator {
    /// Mint a replicator from the submitted configuration.
    ///
    /// `region` is the caller's signing region, `suffix` the registry's monotonic
    /// counter (used as the 1-based ordinal `suffix + 1`, so one value seeds the
    /// whole identity), `now` the creation instant, and `input` the submitted
    /// configuration as `CreateReplicator` deserialized it.
    pub fn provisioned(
        region: &str,
        suffix: u64,
        now: SystemTime,
        input: CreateReplicatorInput,
    ) -> Self {
        let name = input.replicator_name.as_str();
        let ordinal = suffix + 1;
        let digest = stable_digest(&format!(
            "{}:{region}:replicator:{name}:{ordinal}",
            ::engine::emulator_account_id()
        ));
        let arn = ::engine::emulator_arn(
            "kafka",
            region,
            &format!(
                "replicator/{name}/{}-{ordinal}",
                uuid_v4_shaped_from_digest(&digest)
            ),
        );
        // CAPPED, not trusted: the handler refuses an over-size request with the
        // modelled `BadRequestException`, but this constructor must not let any
        // caller of this module store a record whose size — and so the work of
        // every later read of it — comes from an unchecked request value.
        let kafka_clusters = input
            .kafka_clusters
            .into_iter()
            .take(MAX_CLUSTERS_PER_REPLICATOR)
            .map(|cluster| ReplicatorCluster {
                // UNREACHABLE empty: the handler refuses an entry that identifies
                // no cluster, so every stored entry carries the alias its MSK ARN
                // or its Apache cluster id gives it.
                alias: kafka_cluster_alias(&cluster).unwrap_or_default(),
                cluster,
            })
            .collect();
        Self {
            current_version: current_version_token(&arn, 1),
            resource_arn: arn.clone(),
            arn,
            name: input.replicator_name,
            region: region.to_string(),
            description: input.description,
            service_execution_role_arn: input.service_execution_role_arn,
            creation_time: now,
            running_due: now + REPLICATOR_ACTIVATION_DELAY,
            state: REPLICATOR_STATE_CREATING.to_string(),
            // A replicator created here is the replicator itself, never the
            // read-only reference AWS surfaces in the OTHER region of a
            // cross-region setup.
            is_replicator_reference: false,
            kafka_clusters,
            // Capped for the same reason as the cluster list above.
            replication_info: input
                .replication_info_list
                .into_iter()
                .take(MAX_FLOWS_PER_REPLICATOR)
                .collect(),
            log_delivery: input.log_delivery,
            // The handler already refuses an over-tagged request, so this cap is
            // an unreachable backstop — and it takes the surviving tags in KEY
            // order out of an ordered map, so if it ever did fire the subset it
            // keeps is deterministic rather than whatever order the request map
            // happened to iterate in.
            tags: input
                .tags
                .unwrap_or_default()
                .into_iter()
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .take(MAX_TAGS_PER_REPLICATOR)
                .collect(),
        }
    }

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
    ///
    /// A single lookup, linear in the cluster list. A reader resolving EVERY
    /// replication builds [`Self::alias_index`] once instead, so its work is
    /// `O(n + m)` in the replications and the clusters rather than their product.
    pub fn alias_of(&self, arn: Option<&str>, id: Option<&str>) -> Option<String> {
        self.kafka_clusters
            .iter()
            .find(|cluster| cluster.names(arn, id))
            .map(|cluster| cluster.alias.clone())
    }

    /// A lookup from the way a `ReplicationInfo` can name a cluster — by ARN for
    /// an Amazon MSK cluster (`true`) or by id for an Apache Kafka one (`false`)
    /// — to that cluster's alias.
    ///
    /// Built ONCE per rendering, so resolving every replication costs `O(n + m)`
    /// rather than one linear scan of the clusters per replication endpoint. The
    /// first cluster to claim a key keeps it, which is the same cluster
    /// [`Self::alias_of`]'s `find` would have returned.
    fn alias_index(&self) -> HashMap<(bool, &str), &str> {
        let mut index = HashMap::with_capacity(self.kafka_clusters.len());
        for entry in &self.kafka_clusters {
            if let Some(msk) = entry.cluster.amazon_msk_cluster.as_ref() {
                index
                    .entry((true, msk.msk_cluster_arn.as_str()))
                    .or_insert(entry.alias.as_str());
            }
            if let Some(apache) = entry.cluster.apache_kafka_cluster.as_ref() {
                index
                    .entry((false, apache.apache_kafka_cluster_id.as_str()))
                    .or_insert(entry.alias.as_str());
            }
        }
        index
    }

    /// The replicator as the `ReplicatorSummary` `ListReplicators` reports.
    ///
    /// Each replication is reported by the ALIASES of its endpoints, resolved
    /// against this replicator's own cluster list — the summary shape carries no
    /// ARN or id member, so the aliases are derived from the stored setup rather
    /// than echoed from it.
    pub fn to_replicator_summary(&self) -> ReplicatorSummary {
        let aliases = self.alias_index();
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
                        source_kafka_cluster_alias: indexed_alias(
                            &aliases,
                            info.source_kafka_cluster_arn.as_deref(),
                            info.source_kafka_cluster_id.as_deref(),
                        ),
                        target_kafka_cluster_alias: indexed_alias(
                            &aliases,
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

    /// The replicator's Kafka clusters as `DescribeReplicator` reports them: the
    /// same stored records [`Self::to_replicator_summary`] summarises, rendered
    /// into the richer description shape.
    pub fn kafka_cluster_descriptions(&self) -> Vec<KafkaClusterDescription> {
        self.kafka_clusters
            .iter()
            .map(ReplicatorCluster::to_description)
            .collect()
    }

    /// The replicator's replications as `DescribeReplicator` reports them: the
    /// stored configuration, with each end named by the ALIAS resolved against
    /// this replicator's own cluster list — through the SAME index
    /// [`Self::to_replicator_summary`] uses, so the two readers cannot disagree
    /// about which cluster an end names, and neither does work proportional to
    /// the product of the two lists.
    pub fn replication_info_descriptions(&self) -> Vec<ReplicationInfoDescription> {
        let aliases = self.alias_index();
        self.replication_info
            .iter()
            .map(|info| ReplicationInfoDescription {
                consumer_group_replication: Some(info.consumer_group_replication.clone()),
                source_kafka_cluster_alias: indexed_alias(
                    &aliases,
                    info.source_kafka_cluster_arn.as_deref(),
                    info.source_kafka_cluster_id.as_deref(),
                ),
                target_compression_type: Some(info.target_compression_type.clone()),
                target_kafka_cluster_alias: indexed_alias(
                    &aliases,
                    info.target_kafka_cluster_arn.as_deref(),
                    info.target_kafka_cluster_id.as_deref(),
                ),
                topic_replication: Some(info.topic_replication.clone()),
            })
            .collect()
    }
}

/// The alias a replication endpoint resolves to through an index
/// [`Replicator::alias_index`] built, or `None` when the setup holds no cluster
/// the endpoint names. The ARN is tried first and the id second, exactly as
/// [`ReplicatorCluster::names`] matches them.
fn indexed_alias(
    aliases: &HashMap<(bool, &str), &str>,
    arn: Option<&str>,
    id: Option<&str>,
) -> Option<String> {
    if let Some(arn) = arn {
        if let Some(alias) = aliases.get(&(true, arn)) {
            return Some((*alias).to_string());
        }
    }
    aliases
        .get(&(false, id?))
        .map(|alias| (*alias).to_string())
}

/// Why [`ComAmazonawsKafkaKafkaState::register_cluster`] or
/// [`ComAmazonawsKafkaKafkaState::register_replicator`] refused to register a
/// resource. Every variant leaves the registry exactly as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterRejected {
    /// The region already holds [`MAX_CLUSTERS_PER_REGION`] clusters.
    RegionQuota,
    /// The registry already holds its ceiling of resources of this kind
    /// ([`MAX_CLUSTERS_IN_REGISTRY`], [`MAX_REPLICATORS_IN_REGISTRY`]).
    RegistryCeiling,
    /// A cluster with this ARN is already registered. The registry's monotonic
    /// ordinal makes this unreachable; it is refused rather than overwriting an
    /// existing cluster.
    DuplicateArn,
}

/// Why [`ComAmazonawsKafkaKafkaState::register_topic`] refused to register a
/// topic. Every variant leaves the registry exactly as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopicRejected {
    /// The cluster already holds a topic with this name — including one that is
    /// `DELETING` but still in the registry.
    DuplicateName,
    /// The cluster already holds [`MAX_TOPICS_PER_CLUSTER`] topics.
    ClusterCeiling,
}

/// One topic of a provisioned cluster, holding everything a reader reports so
/// `DescribeTopic` computes its answer from the record rather than echoing a
/// request.
#[derive(Debug, Clone)]
pub struct Topic {
    /// The topic's own name, unique within its cluster.
    pub name: String,
    /// The ARN of the cluster that holds the topic — half of the registry key.
    pub cluster_arn: String,
    /// `arn:<partition>:kafka:<region>:<account>:topic/<cluster>/<cluster-id>/<topic>`,
    /// the form Amazon MSK uses for a topic in an IAM policy.
    pub arn: String,
    pub partition_count: i32,
    pub replication_factor: i32,
    /// The topic-level Apache Kafka configuration the request supplied, stored
    /// verbatim because the service hands it back the same way.
    pub configs: Option<String>,
    /// A `TopicState` value.
    pub state: String,
    /// The instant the clock owes this topic its `CREATING` -> `ACTIVE`
    /// transition. Stored, never slept on.
    pub active_due: SystemTime,
    /// Set by `DeleteTopic`: the instant the clock owes this topic's removal from
    /// the registry. `None` for a topic nobody has deleted.
    pub removal_due: Option<SystemTime>,
}

impl Topic {
    /// Mint a topic of `cluster` from an accepted `CreateTopic` request. The ARN
    /// is derived from the cluster's identity, so a topic's ARN belongs to the
    /// cluster that holds it and to that cluster's region.
    pub fn created(
        cluster: &Cluster,
        name: String,
        partition_count: i32,
        replication_factor: i32,
        configs: Option<String>,
        now: SystemTime,
    ) -> Self {
        Self {
            arn: cluster.topic_arn(&name),
            name,
            cluster_arn: cluster.arn.clone(),
            partition_count,
            replication_factor,
            configs,
            state: TOPIC_STATE_CREATING.to_string(),
            active_due: now + TOPIC_ACTIVATION_DELAY,
            removal_due: None,
        }
    }

    /// Apply the `CREATING` -> `ACTIVE` transition once `now` has reached
    /// [`Self::active_due`]. Idempotent, and a no-op for every other state: only a
    /// transition the clock already owed is materialised here. A topic that is
    /// `DELETING` never goes back to `ACTIVE`.
    pub fn settle(&mut self, now: SystemTime) {
        if self.state == TOPIC_STATE_CREATING && now >= self.active_due {
            self.state = TOPIC_STATE_ACTIVE.to_string();
        }
    }

    /// The topic as the `TopicInfo` `ListTopics` reports: its identity and the
    /// counts it was created with.
    ///
    /// `OutOfSyncReplicaCount` is 0 because every replica of an emulated topic is
    /// in sync: nothing here can put a broker behind, so the count is computed
    /// from that fact rather than invented.
    pub fn to_topic_info(&self) -> TopicInfo {
        TopicInfo {
            topic_arn: Some(self.arn.clone()),
            topic_name: Some(self.name.clone()),
            replication_factor: Some(self.replication_factor),
            partition_count: Some(self.partition_count),
            out_of_sync_replica_count: Some(0),
        }
    }
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
    /// Every cluster operation an update call recorded, keyed by its own ARN.
    /// `BTreeMap` so a reader walking the history has one total order.
    pub cluster_operations: BTreeMap<String, ClusterOperation>,
    /// Every live topic, keyed by (cluster ARN, topic name). A topic name is
    /// unique within its cluster, and the `BTreeMap` groups one cluster's topics
    /// contiguously in name order — the order `ListTopics` pages through.
    pub topics: BTreeMap<(String, String), Topic>,
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
    /// (region, replicator name) -> ARN. A replicator name is unique within an
    /// account and region, which is what makes a repeated `CreateReplicator`
    /// under one name a conflict.
    pub replicator_arn_by_name: HashMap<(String, String), String>,
    /// Monotonic counter feeding replicator ARNs, so each replicator gets a
    /// distinct identity.
    pub next_replicator_suffix: u64,
    /// Every live VPC connection, keyed by ARN. `BTreeMap` so the readers have
    /// one total order to walk.
    pub vpc_connections: BTreeMap<String, VpcConnection>,
    /// Monotonic counter feeding each VPC connection's ARN, so each one gets a
    /// distinct identity.
    pub next_vpc_connection_suffix: u64,
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
    /// Its operation history goes with it: an operation names the cluster it ran
    /// on, so keeping records of a cluster nobody can reach would grow the store
    /// without bound.
    ///
    /// A topic belongs to the cluster whose brokers hold it, so the cluster's
    /// topics leave with it: nothing in the registry can outlive its cluster.
    ///
    /// A channel reads from a cluster, so a cluster that is gone holds no
    /// channels: every channel registered under `arn` is dropped here too, which
    /// is why this is also the one place that can orphan a channel — and does
    /// not.
    pub fn unregister_cluster(&mut self, arn: &str) -> Option<Cluster> {
        let cluster = self.clusters.remove(arn)?;
        self.cluster_arn_by_name
            .remove(&(cluster.region.clone(), cluster.name.clone()));
        for operation_arn in &cluster.operation_arns {
            self.cluster_operations.remove(operation_arn);
        }
        self.topics.retain(|(topic_cluster, _), _| topic_cluster != arn);
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
        // A regional endpoint serves only its own region's clusters, so this
        // resolves a cluster of another region to `None` exactly as an absent one.
        if cluster.region != region {
            return None;
        }
        cluster.settle(now);
        Some(cluster)
    }

    /// Add `connection` to the VPC-connection registry. The one insertion path,
    /// so it is also the one place the registry's ceiling is enforced — a create
    /// past it is refused here rather than for whichever handler remembered to
    /// check.
    ///
    /// Refuses, changing nothing, when the registry is at
    /// [`MAX_VPC_CONNECTIONS_IN_REGISTRY`] or when the ARN is already
    /// registered.
    pub fn register_vpc_connection(
        &mut self,
        connection: VpcConnection,
    ) -> Result<(), RegisterRejected> {
        if self.vpc_connections.contains_key(&connection.arn) {
            return Err(RegisterRejected::DuplicateArn);
        }
        if self.vpc_connections.len() >= MAX_VPC_CONNECTIONS_IN_REGISTRY {
            return Err(RegisterRejected::RegistryCeiling);
        }
        self.vpc_connections
            .insert(connection.arn.clone(), connection);
        Ok(())
    }

    /// The VPC connection `arn` names, as an endpoint for `region` sees it: a
    /// regional endpoint serves only its own region's connections, so a
    /// well-formed ARN from another region resolves to `None` exactly as an
    /// absent one does.
    pub fn vpc_connection_in(&self, region: &str, arn: &str) -> Option<&VpcConnection> {
        self.vpc_connections
            .get(arn)
            .filter(|connection| connection.region == region)
    }

    /// Remove the VPC connection `arn` names, as an endpoint for `region` sees
    /// it: an ARN belonging to another region resolves to `None` exactly as an
    /// absent one does, so no endpoint can delete another region's connection.
    pub fn unregister_vpc_connection_in(
        &mut self,
        region: &str,
        arn: &str,
    ) -> Option<VpcConnection> {
        self.vpc_connection_in(region, arn)?;
        self.vpc_connections.remove(arn)
    }

    /// The same lookup, with the `CREATING` -> `AVAILABLE` transition the clock
    /// already owed THIS connection materialised. Scoped to the one connection
    /// being reported, for the same reason as [`Self::settled_cluster_in`].
    pub fn settled_vpc_connection_in(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
    ) -> Option<&VpcConnection> {
        let connection = self.vpc_connections.get_mut(arn)?;
        if connection.region != region {
            return None;
        }
        connection.settle(now);
        Some(connection)
    }

    /// The cluster `arn` names in `region`, mutably — the write counterpart of
    /// [`Self::cluster_in`], with the same regional scoping, so a mutating
    /// operation can only ever reach a cluster a reader of this endpoint also
    /// sees.
    pub fn cluster_mut_in(&mut self, region: &str, arn: &str) -> Option<&mut Cluster> {
        self.clusters
            .get_mut(arn)
            .filter(|cluster| cluster.region == region)
    }

    /// The stored tag set of the resource `arn` names in `region`.
    ///
    /// This is the ONE resolution the tagging operations share, so
    /// `TagResource`, `UntagResource` and `ListTagsForResource` cannot disagree
    /// about which ARNs name a taggable resource or about which failure a given
    /// ARN earns. Tags live on the resource record itself, which is why a tag
    /// written here is visible to every other reader of that resource (a
    /// cluster's tags are reported by `DescribeCluster` and `ListClusters`, a
    /// channel's by `DescribeChannel` and `ListChannels`).
    pub fn resource_tags_in(
        &self,
        region: &str,
        arn: &str,
    ) -> Result<&BTreeMap<String, String>, TagTargetRejected> {
        match resource_type_of_arn(arn)? {
            CLUSTER_RESOURCE_TYPE => self
                .cluster_in(region, arn)
                .map(|cluster| &cluster.tags)
                .ok_or(TagTargetRejected::NoSuchResource),
            CHANNEL_RESOURCE_TYPE => self
                .channel_by_arn_in(region, arn)
                .map(|channel| &channel.tags)
                .ok_or(TagTargetRejected::NoSuchResource),
            REPLICATOR_RESOURCE_TYPE => self
                .replicator_in(region, arn)
                .map(|replicator| &replicator.tags)
                .ok_or(TagTargetRejected::NoSuchResource),
            // Another MSK resource type: a well-formed ARN of a kind no
            // registry here carries a tag set for yet — the creates that accept
            // `Tags` for them (`CreateConfiguration`, `CreateVpcConnection`, …)
            // are still scaffolds — so this region holds no such taggable
            // resource. That is a miss, not a malformed request, and this arm is
            // the one place to extend when one of those registries starts
            // storing tags.
            _ => Err(TagTargetRejected::NoSuchResource),
        }
    }

    /// The same resolution, mutably — what `TagResource` and `UntagResource`
    /// write through.
    pub fn resource_tags_mut_in(
        &mut self,
        region: &str,
        arn: &str,
    ) -> Result<&mut BTreeMap<String, String>, TagTargetRejected> {
        match resource_type_of_arn(arn)? {
            CLUSTER_RESOURCE_TYPE => self
                .cluster_mut_in(region, arn)
                .map(|cluster| &mut cluster.tags)
                .ok_or(TagTargetRejected::NoSuchResource),
            CHANNEL_RESOURCE_TYPE => self
                .channel_by_arn_mut_in(region, arn)
                .map(|channel| &mut channel.tags)
                .ok_or(TagTargetRejected::NoSuchResource),
            REPLICATOR_RESOURCE_TYPE => self
                .replicator_mut_in(region, arn)
                .map(|replicator| &mut replicator.tags)
                .ok_or(TagTargetRejected::NoSuchResource),
            _ => Err(TagTargetRejected::NoSuchResource),
        }
    }

    /// The channel `arn` names, as an endpoint for `region` sees it, resolved by
    /// its OWN ARN alone.
    ///
    /// That is the form an operation that takes any resource ARN addresses a
    /// channel in: the cluster is not a separate request member there, it is a
    /// segment of the channel ARN itself (`channel/<cluster>/<cluster-id>/...`),
    /// so a channel ARN already names the cluster that holds it.
    /// [`Self::channel_in`] stays the resolution for the paths that carry the
    /// cluster separately and must agree with it.
    pub fn channel_by_arn_in(&self, region: &str, arn: &str) -> Option<&Channel> {
        self.channels
            .get(arn)
            .filter(|channel| channel.region == region)
    }

    /// The same lookup, mutably — the write counterpart of
    /// [`Self::channel_by_arn_in`], with the same regional scoping.
    pub fn channel_by_arn_mut_in(&mut self, region: &str, arn: &str) -> Option<&mut Channel> {
        self.channels
            .get_mut(arn)
            .filter(|channel| channel.region == region)
    }

    /// The replicator `arn` names in `region`, mutably — the write counterpart of
    /// [`Self::replicator_in`], with the same regional scoping, so a mutating
    /// operation can only ever reach a replicator a reader of this endpoint also
    /// sees.
    pub fn replicator_mut_in(&mut self, region: &str, arn: &str) -> Option<&mut Replicator> {
        self.replicators
            .get_mut(arn)
            .filter(|replicator| replicator.region == region)
    }

    /// The cluster operation `arn` names, as an endpoint for `region` sees it —
    /// `None` for an operation of another region, exactly as for an absent one.
    pub fn cluster_operation_in(&self, region: &str, arn: &str) -> Option<&ClusterOperation> {
        self.cluster_operations
            .get(arn)
            .filter(|operation| operation.region == region)
    }

    /// Start a storage update on the cluster `arn` names, as an endpoint for
    /// `region` sees it: record the cluster operation, move the cluster to
    /// `UPDATING`, and hand it the change the clock will apply at
    /// [`CLUSTER_UPDATE_DELAY`]. Answers the new operation's ARN, or `None` when
    /// this region holds no cluster for `arn`.
    ///
    /// The cluster is resolved through [`Self::settled_cluster_in_mut`], so a
    /// writer is scoped to its own region exactly as every reader is, and it
    /// updates a record whose clock-owed transitions have landed.
    ///
    /// The one path that starts a storage update — `UpdateStorage` and
    /// `UpdateBrokerStorage` both come through it, each naming its own
    /// `operation_type` — so the operation record, the cluster's pending change,
    /// its active operation ARN and its operation history cannot drift apart. The
    /// history is bounded here: the records past
    /// [`MAX_OPERATIONS_PER_CLUSTER`] are the cluster's own oldest, so the trim
    /// costs one walk of THAT cluster's history rather than of the registry.
    pub fn start_storage_update(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
        operation_type: &str,
        change: StorageChange,
    ) -> Option<String> {
        let cluster = self.settled_cluster_in_mut(region, arn, now)?;
        let cluster_arn = cluster.arn.clone();
        let operation_region = cluster.region.clone();
        let source_cluster_info = cluster.storage_cluster_info();
        let target_cluster_info = cluster.storage_cluster_info_after(&change);
        // The cluster mints the operation's ARN and holds the pending change,
        // the same way `UpdateConnectivity` starts its own: one in-flight update
        // per cluster, whatever began it.
        let operation_arn = cluster.begin_storage_update(now, change);
        let operation = ClusterOperation {
            arn: operation_arn.clone(),
            cluster_arn,
            region: operation_region,
            operation_type: operation_type.to_string(),
            creation_time: now,
            end_due: now + CLUSTER_UPDATE_DELAY,
            source_cluster_info,
            target_cluster_info,
        };
        cluster.operation_arns.push(operation_arn.clone());
        // The history is bounded: anything past the ceiling is this cluster's
        // own oldest record.
        let excess = cluster
            .operation_arns
            .len()
            .saturating_sub(MAX_OPERATIONS_PER_CLUSTER);
        let evicted: Vec<String> = cluster.operation_arns.drain(..excess).collect();
        self.cluster_operations
            .insert(operation_arn.clone(), operation);
        for stale in evicted {
            self.cluster_operations.remove(&stale);
        }
        Some(operation_arn)
    }

    /// How many topics the cluster `cluster_arn` names holds as a reader at
    /// `now` sees it — what the per-cluster ceiling is checked against. A topic
    /// whose removal the clock already owes is NOT counted, so the ceiling
    /// agrees with what every reader reports rather than reserving room for
    /// records that are gone. The registry key leads with the cluster ARN, so
    /// one cluster's topics are a contiguous range.
    pub fn topic_count_in(&self, cluster_arn: &str, now: SystemTime) -> usize {
        self.topics_in(cluster_arn)
            .filter(|topic| topic.removal_due.is_none_or(|due| now < due))
            .count()
    }

    /// Every topic of the cluster `cluster_arn` names, in topic-name order.
    pub fn topics_in<'a>(&'a self, cluster_arn: &'a str) -> impl Iterator<Item = &'a Topic> + 'a {
        self.topics
            .range((cluster_arn.to_string(), String::new())..)
            .take_while(move |((arn, _), _)| arn == cluster_arn)
            .map(|(_, topic)| topic)
    }

    /// Add `topic` to the registry. The one insertion path, and the one place the
    /// per-cluster ceiling is enforced, so it holds for every caller of this
    /// module rather than only for a handler that remembers to check first.
    ///
    /// `now` is the instant the create is served: the removals this cluster's
    /// topics are already owed are materialised FIRST, so a name whose topic is
    /// gone as far as every reader is concerned is free here too (whether or not
    /// anybody read it in between) and the ceiling bounds what the registry
    /// actually holds.
    ///
    /// Refuses, changing nothing, when the cluster still holds a topic of that
    /// name (a `DELETING` one whose removal is not yet due still counts: it is
    /// still there) or already holds [`MAX_TOPICS_PER_CLUSTER`] topics.
    pub fn register_topic(
        &mut self,
        topic: Topic,
        now: SystemTime,
    ) -> Result<(), TopicRejected> {
        // Materialise every removal THIS CLUSTER's topics are already owed, not
        // only the addressed name: the ceiling below has to bound the records the
        // registry holds, so a caller looping create-and-delete under fresh names
        // cannot grow it past the ceiling. Each removal was due at the same
        // instant whether or not anybody addressed that topic.
        self.prune_removed_topics(&topic.cluster_arn, now);
        let key = (topic.cluster_arn.clone(), topic.name.clone());
        if self.topics.contains_key(&key) {
            return Err(TopicRejected::DuplicateName);
        }
        if self.topic_count_in(&topic.cluster_arn, now) >= MAX_TOPICS_PER_CLUSTER {
            return Err(TopicRejected::ClusterCeiling);
        }
        self.topics.insert(key, topic);
        Ok(())
    }

    /// Drop the topics of `cluster_arn` whose removal the clock owed by `now`.
    /// Scoped to the one cluster, which the registry key makes a contiguous
    /// range, so this never walks the whole registry.
    fn prune_removed_topics(&mut self, cluster_arn: &str, now: SystemTime) {
        let removed: Vec<(String, String)> = self
            .topics_in(cluster_arn)
            .filter(|topic| topic.removal_due.is_some_and(|due| now >= due))
            .map(|topic| (topic.cluster_arn.clone(), topic.name.clone()))
            .collect();
        for key in removed {
            self.topics.remove(&key);
        }
    }

    /// The same lookup, with the transitions the clock already owed THIS topic
    /// materialised: a topic whose removal is due is gone (so this answers
    /// `None`), and one whose activation is due is `ACTIVE`.
    ///
    /// Scoped to the one topic being addressed: a read must not do work
    /// proportional to the registry, and no other topic's status is this
    /// request's business.
    pub fn settled_topic_in(
        &mut self,
        cluster_arn: &str,
        name: &str,
        now: SystemTime,
    ) -> Option<&Topic> {
        let key = (cluster_arn.to_string(), name.to_string());
        if self
            .topics
            .get(&key)
            .and_then(|topic| topic.removal_due)
            .is_some_and(|due| now >= due)
        {
            self.topics.remove(&key);
            return None;
        }
        let topic = self.topics.get_mut(&key)?;
        topic.settle(now);
        Some(topic)
    }

    /// Accept the deletion of the topic `name` in `cluster_arn`, handing back the
    /// record as the caller must now see it: `DELETING`, with the instant the
    /// clock owes its removal set.
    ///
    /// Answers `None` when the cluster holds no such topic — including one whose
    /// removal the clock already owed, which this materialises first so a topic
    /// is never deleted twice. A topic already `DELETING` keeps the removal
    /// instant it was given, so repeating the call cannot postpone it.
    pub fn begin_topic_deletion(
        &mut self,
        cluster_arn: &str,
        name: &str,
        now: SystemTime,
    ) -> Option<&Topic> {
        self.settled_topic_in(cluster_arn, name, now)?;
        let topic = self
            .topics
            .get_mut(&(cluster_arn.to_string(), name.to_string()))?;
        if topic.removal_due.is_none() {
            topic.state = TOPIC_STATE_DELETING.to_string();
            topic.removal_due = Some(now + TOPIC_DELETION_DELAY);
        }
        Some(topic)
    }

    /// The ARN registered for the replicator `name` in `region`, if any. This is
    /// the lookup that makes a repeated `CreateReplicator` a conflict.
    pub fn replicator_arn_for_name(&self, region: &str, name: &str) -> Option<&String> {
        self.replicator_arn_by_name
            .get(&(region.to_string(), name.to_string()))
    }

    /// Add `replicator` to every index that must know about it. The one insertion
    /// path, so the ARN registry and the (region, name) index cannot drift apart
    /// — and the one place the registry's ceiling is enforced, so it holds for
    /// every caller of this module rather than only for a handler that remembers
    /// to check first.
    ///
    /// Refuses, changing nothing, when the registry is at
    /// [`MAX_REPLICATORS_IN_REGISTRY`] or when the ARN is already registered.
    pub fn register_replicator(&mut self, replicator: Replicator) -> Result<(), RegisterRejected> {
        if self.replicators.contains_key(&replicator.arn) {
            return Err(RegisterRejected::DuplicateArn);
        }
        if self.replicators.len() >= MAX_REPLICATORS_IN_REGISTRY {
            return Err(RegisterRejected::RegistryCeiling);
        }
        self.replicator_arn_by_name.insert(
            (replicator.region.clone(), replicator.name.clone()),
            replicator.arn.clone(),
        );
        self.replicators.insert(replicator.arn.clone(), replicator);
        Ok(())
    }

    /// Remove the replicator `arn` names from every index, handing it back. The
    /// one removal path, for the same reason.
    pub fn unregister_replicator(&mut self, arn: &str) -> Option<Replicator> {
        let replicator = self.replicators.remove(arn)?;
        self.replicator_arn_by_name
            .remove(&(replicator.region.clone(), replicator.name.clone()));
        Some(replicator)
    }

    /// The replicator `arn` names, as an endpoint for `region` sees it.
    ///
    /// A regional endpoint serves only its own region's replicators, so a
    /// well-formed ARN belonging to another region resolves to `None` exactly as
    /// an absent one does.
    pub fn replicator_in(&self, region: &str, arn: &str) -> Option<&Replicator> {
        self.replicators
            .get(arn)
            .filter(|replicator| replicator.region == region)
    }

    /// The same lookup, with the `CREATING` -> `RUNNING` transition the clock
    /// already owed THIS replicator materialised.
    ///
    /// Scoped to the one replicator being reported: a read must not do work
    /// proportional to the whole registry, and no other replicator's status is
    /// this request's business.
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

impl VpcConnection {
    /// Mint a VPC connection to the cluster `cluster_arn`/`cluster_name` names
    /// from the submitted request.
    ///
    /// `region` is the caller's signing region, `suffix` the registry's
    /// monotonic counter (used as the 1-based ordinal `suffix + 1`, so one value
    /// seeds the whole identity) and `now` the creation instant. The connection
    /// starts in `CREATING`, which is the state the real service reports a
    /// just-requested connection in.
    pub fn requested(
        region: &str,
        suffix: u64,
        now: SystemTime,
        cluster_arn: &str,
        cluster_name: &str,
        input: CreateVpcConnectionInput,
    ) -> Self {
        let ordinal = suffix + 1;
        let account = ::engine::emulator_account_id();
        let digest = stable_digest(&format!(
            "{account}:{region}:{cluster_arn}:{}:{ordinal}",
            input.vpc_id
        ));
        // An MSK VPC-connection ARN names the cluster owner's account and the
        // cluster alongside the connection's own id:
        // `vpc-connection/<cluster-owner-account>/<cluster-name>/<uuid>-<n>`.
        let arn = ::engine::emulator_arn(
            "kafka",
            region,
            &format!(
                "vpc-connection/{account}/{cluster_name}/{}-{ordinal}",
                uuid_v4_shaped_from_digest(&digest)
            ),
        );
        Self {
            arn,
            region: region.to_string(),
            target_cluster_arn: cluster_arn.to_string(),
            authentication: input.authentication,
            vpc_id: input.vpc_id,
            client_subnets: input.client_subnets,
            security_groups: input.security_groups,
            creation_time: now,
            available_due: now + VPC_CONNECTION_ACTIVATION_DELAY,
            state: "CREATING".to_string(),
            tags: input.tags.unwrap_or_default().into_iter().collect(),
        }
    }

    /// Apply the `CREATING` -> `AVAILABLE` transition once `now` has reached
    /// [`Self::available_due`]. Idempotent, and a no-op for every other state:
    /// only a transition the clock already owed is materialised here.
    pub fn settle(&mut self, now: SystemTime) {
        if self.state == "CREATING" && now >= self.available_due {
            self.state = "AVAILABLE".to_string();
        }
    }

    /// The connection's tags as the map every reader reports.
    pub fn tag_map(&self) -> HashMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }
}

/// Why a tagging operation could not resolve its `ResourceArn` to a resource.
///
/// The two variants are the two failures the real service distinguishes: a value
/// it cannot parse as one of its own resource ARNs, and a well-formed one that
/// names nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagTargetRejected {
    /// Not an ARN of an MSK resource at all — a client `BadRequestException`.
    NotAnArn,
    /// A well-formed MSK resource ARN this endpoint's region holds no resource
    /// for: absent, deleted, or belonging to another region. A `NotFoundException`.
    NoSuchResource,
}

/// The resource-type segment of an MSK cluster ARN.
pub const CLUSTER_RESOURCE_TYPE: &str = "cluster";

/// The resource-type segment of an MSK channel ARN.
pub const CHANNEL_RESOURCE_TYPE: &str = "channel";

/// The resource-type segment of an MSK replicator ARN.
pub const REPLICATOR_RESOURCE_TYPE: &str = "replicator";

/// The message AWS's resource-tagging plumbing answers a missing resource with.
///
/// RECORDED behaviour, not modelled: the Smithy model gives `NotFoundException`
/// only a free-form `message`, and the recorded responses for the MSK tagging
/// operations carry this fixed text (the same text the tagging APIs of other
/// services answer with) rather than one built from the request. The requirement
/// here is the 404 and the `NotFoundException` code; this constant keeps the
/// three tagging operations answering with one text instead of three.
///
/// It is NOT the text the resource readers use for a resource they cannot find:
/// those build a message from the request (which region looked, for which ARN).
/// Reach for this one only from the tagging operations.
pub const TAGGING_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The longest a tag key may be, from the model's own documentation of
/// `UntagResourceRequest$TagKeys`: "Tag keys must be between 1 and 128 Unicode
/// characters in length."
pub const MAX_TAG_KEY_LENGTH: usize = 128;

/// The tag-key prefix the model reserves for AWS itself: "You can't start a tag
/// key with aws: because this prefix is reserved for use by AWS. AWS creates tags
/// that begin with this prefix on your behalf, but you can't edit or delete them."
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Why `key` is not a tag key a caller may set or remove, phrased as the message
/// a `BadRequestException` carries — or `None` when the key is acceptable.
///
/// Every rule here is stated by the model's own `UntagResourceRequest$TagKeys`
/// documentation: 1..128 Unicode characters, no `aws:` prefix, and a character
/// set of "Unicode letters, digits, white space, and the following special
/// characters: _ . / = + - @". The message names the member's constraint and the
/// offending key, so a caller can fix the request from the text alone.
pub fn tag_key_violation(key: &str) -> Option<String> {
    let length = key.chars().count();
    if !(1..=MAX_TAG_KEY_LENGTH).contains(&length) {
        return Some(format!(
            "Tag key '{key}' is {length} Unicode characters long; a tag key must be between 1 and {MAX_TAG_KEY_LENGTH} Unicode characters in length."
        ));
    }
    if key.starts_with(RESERVED_TAG_KEY_PREFIX) {
        return Some(format!(
            "Tag key '{key}' starts with '{RESERVED_TAG_KEY_PREFIX}', a prefix reserved for use by AWS."
        ));
    }
    if let Some(offending) = key.chars().find(|c| !is_tag_key_character(*c)) {
        return Some(format!(
            "Tag key '{key}' contains the unsupported character '{offending}'; a tag key must consist of Unicode letters, digits, white space and the special characters _ . / = + - @."
        ));
    }
    None
}

/// Whether `c` is one of the characters the model's tag-key documentation allows.
fn is_tag_key_character(c: char) -> bool {
    c.is_alphabetic()
        || c.is_numeric()
        || c.is_whitespace()
        || matches!(c, '_' | '.' | '/' | '=' | '+' | '-' | '@')
}

/// The most tags one resource may carry, from the Amazon MSK tag restrictions
/// ("Maximum number of tags per resource: 50") — the same limit, from the same
/// source, that [`MAX_TAGS_PER_REPLICATOR`] enforces on the tags a replicator is
/// created with.
///
/// It is also what bounds this store: a tag key comes from the request, so
/// without a ceiling on the resulting count a caller could grow one resource's
/// tag map with every call it makes. The ceiling is checked BEFORE the first
/// insert, so a refused request writes nothing.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The longest a tag value may be, from the same restrictions ("Maximum value
/// length: 256 Unicode characters"). An empty value is allowed.
pub const MAX_TAG_VALUE_LENGTH: usize = 256;

/// Why `submitted` may not be written onto a resource that already carries
/// `stored`, phrased as the message a `BadRequestException` takes — or `None`
/// when the write is acceptable.
///
/// The ONE admission check for a tag write, shared by `TagResource` and by the
/// `Tags` a create carries, so the two cannot disagree about which tag sets a
/// resource may hold. It checks every key and value against the documented
/// limits and then the resulting entry count: only keys the resource does not
/// already carry add to it, so re-tagging an existing key at the ceiling is
/// still allowed. The submitted pairs are walked in key order, so one request
/// always earns the same message.
pub fn tag_write_violation<'a, I>(stored: &BTreeMap<String, String>, submitted: I) -> Option<String>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let submitted: BTreeMap<&str, &str> = submitted.into_iter().collect();
    for (key, value) in &submitted {
        if let Some(reason) = tag_key_violation(key) {
            return Some(reason);
        }
        let length = value.chars().count();
        if length > MAX_TAG_VALUE_LENGTH {
            return Some(format!(
                "The value of tag key '{key}' is {length} Unicode characters long; a tag value must be at most {MAX_TAG_VALUE_LENGTH} Unicode characters in length."
            ));
        }
    }
    let added = submitted
        .keys()
        .filter(|key| !stored.contains_key(**key))
        .count();
    let resulting = stored.len() + added;
    if resulting > MAX_TAGS_PER_RESOURCE {
        return Some(format!(
            "The resource carries {} tag(s) and the request adds {added} more, which would give it {resulting}; a resource may carry at most {MAX_TAGS_PER_RESOURCE} tags.",
            stored.len()
        ));
    }
    None
}

/// Encode the ARN a page stopped at as a token. Base64 of a marked plaintext:
/// the token is ENCODED, not authenticated, so a caller could decode it or mint
/// one. That is harmless here — the ARN inside is only used as a `>` comparison
/// against the keys of the caller's own registry, so a forged resume point can
/// only skip or repeat entries the caller may already list.
pub fn encode_cluster_page_token(last_arn: &str) -> String {
    encode_page_token(CLUSTER_PAGE_TOKEN_PREFIX, last_arn)
}

/// The ARN a token resumes after, or `None` when the token is not one a cluster
/// listing minted — which every listing refuses as a bad request rather than
/// serving as a first page.
pub fn decode_cluster_page_token(token: &str) -> Option<String> {
    decode_page_token(CLUSTER_PAGE_TOKEN_PREFIX, token)
}

/// Mint a page token for `value`, marked with `prefix` so a token minted for one
/// listing cannot be mistaken for another's.
///
/// The ONE place a page token is encoded, whatever listing mints it, so a change
/// to the encoding cannot leave two listings disagreeing. Base64 of a marked
/// plaintext: a token is ENCODED, not authenticated — see
/// [`encode_cluster_page_token`] for why that is harmless for these listings.
fn encode_page_token(prefix: &str, value: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{prefix}{value}"))
}

/// The value a token carries, or `None` when it is not a token minted with
/// `prefix` — the inverse of [`encode_page_token`], and the one place a page
/// token is decoded.
fn decode_page_token(prefix: &str, token: &str) -> Option<String> {
    let decoded = ::aws_smithy_types::base64::decode(token).ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    decoded.strip_prefix(prefix).map(ToString::to_string)
}

/// The alias of one `KafkaCluster` entry: an Amazon MSK cluster is aliased by its
/// cluster name (the first segment of its ARN's resource part), and an Apache
/// Kafka cluster by the id the caller gave it. The alias is what prefixes the
/// names of replicated topics, per `KafkaClusterDescription$KafkaClusterAlias`.
pub fn kafka_cluster_alias(cluster: &KafkaCluster) -> Option<String> {
    if let Some(msk) = cluster.amazon_msk_cluster.as_ref() {
        return flow_cluster_alias(Some(msk.msk_cluster_arn.as_str()), None);
    }
    cluster
        .apache_kafka_cluster
        .as_ref()
        .map(|apache| apache.apache_kafka_cluster_id.clone())
}

/// The alias of the cluster a replication flow names at one end, from the ARN or
/// the cluster id the flow carries. Same rule as [`kafka_cluster_alias`]: the
/// cluster name for an MSK ARN, the id itself for an Apache Kafka cluster.
pub fn flow_cluster_alias(arn: Option<&str>, id: Option<&str>) -> Option<String> {
    if let Some(arn) = arn.filter(|value| !value.is_empty()) {
        return Some(cluster_name_in_arn(arn).unwrap_or(arn).to_string());
    }
    id.filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

/// The cluster name inside an MSK cluster ARN (`...:cluster/<name>/<id>`), or
/// `None` when `value` is not an ARN of that form.
fn cluster_name_in_arn(value: &str) -> Option<&str> {
    if !is_cluster_arn(value) {
        return None;
    }
    value.splitn(6, ':').nth(5)?.split('/').nth(1)
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
            // The first version of a cluster is revision 1; each applied update
            // generates the next one.
            current_version: current_version_token(&arn, 1),
            arn,
            name: input.cluster_name,
            region: region.to_string(),
            cluster_type: CLUSTER_TYPE_PROVISIONED.to_string(),
            kafka_version: input.kafka_version,
            creation_time: now,
            active_due: now + CLUSTER_ACTIVATION_DELAY,
            state: CLUSTER_STATE_CREATING.to_string(),
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
            operation_arns: Vec::new(),
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
        if self.state == CLUSTER_STATE_CREATING && now >= self.active_due {
            self.state = CLUSTER_STATE_ACTIVE.to_string();
        }
        match self.pending_update.take() {
            // An update completes only for a cluster that is still `UPDATING`
            // because of it. Any other state means something else now owns the
            // record, so the change is dropped rather than folded into a
            // cluster that has moved on.
            Some(update) if now >= update.due && self.state == CLUSTER_STATE_UPDATING => {
                self.apply_change(update.change);
                // "When this update operation is successful, it generates a new
                // cluster version" (`CurrentVersion` on every update request),
                // so the version token follows the new revision.
                self.revision += 1;
                self.current_version = current_version_token(&self.arn, self.revision);
                self.state = CLUSTER_STATE_ACTIVE.to_string();
            }
            // Due but no longer this cluster's business: dropped, changing
            // nothing.
            Some(dropped) if now >= dropped.due => {}
            // Still in flight: put it back exactly as it was.
            pending => self.pending_update = pending,
        }
    }

    /// The EBS data-volume size each broker's drive is stored with, when the
    /// cluster's configuration names one.
    pub fn volume_size_gb(&self) -> Option<i32> {
        self.ebs_storage_info()
            .and_then(|ebs| ebs.volume_size)
    }

    /// The EBS provisioned-throughput setting the cluster's volumes are stored
    /// with, when its configuration names one.
    pub fn provisioned_throughput(&self) -> Option<&ProvisionedThroughput> {
        self.ebs_storage_info()
            .and_then(|ebs| ebs.provisioned_throughput.as_ref())
    }

    fn ebs_storage_info(&self) -> Option<&EBSStorageInfo> {
        self.broker_node_group_info
            .storage_info
            .as_ref()
            .and_then(|storage| storage.ebs_storage_info.as_ref())
    }

    /// The cluster's storage attributes as the `MutableClusterInfo` a cluster
    /// operation reports: the attributes a storage update can change, and only
    /// those, since an operation describes what it touches rather than the whole
    /// cluster.
    pub fn storage_cluster_info(&self) -> MutableClusterInfo {
        storage_cluster_info(
            self.volume_size_gb(),
            self.provisioned_throughput().cloned(),
            &self.storage_mode,
        )
    }

    /// The same attributes as `change` leaves them: a member the change does not
    /// name keeps the stored value.
    pub fn storage_cluster_info_after(&self, change: &StorageChange) -> MutableClusterInfo {
        storage_cluster_info(
            change.volume_size_gb.or_else(|| self.volume_size_gb()),
            change
                .provisioned_throughput
                .clone()
                .or_else(|| self.provisioned_throughput().cloned()),
            change
                .storage_mode
                .as_deref()
                .unwrap_or(&self.storage_mode),
        )
    }

    /// Write `change` into the stored configuration, so every reader reports the
    /// settings the cluster now runs with. A member the change does not name is
    /// left exactly as it was.
    fn apply_storage_change(&mut self, change: StorageChange) {
        if change.volume_size_gb.is_some() || change.provisioned_throughput.is_some() {
            let ebs = self
                .broker_node_group_info
                .storage_info
                .get_or_insert_with(StorageInfo::default)
                .ebs_storage_info
                .get_or_insert_with(EBSStorageInfo::default);
            if let Some(volume_size) = change.volume_size_gb {
                ebs.volume_size = Some(volume_size);
            }
            if let Some(throughput) = change.provisioned_throughput {
                ebs.provisioned_throughput = Some(throughput);
            }
        }
        if let Some(storage_mode) = change.storage_mode {
            self.storage_mode = storage_mode;
        }
    }

    /// Start a storage update: the cluster enters `UPDATING` with the returned
    /// operation as its active one, and the submitted storage settings become
    /// its own when [`Self::settle`] finds the operation due — the same
    /// acceptance the real service gives an `UpdateStorage` call, which reports
    /// the new volume size, throughput or storage mode and the new cluster
    /// version only once that operation finishes.
    ///
    /// A member `change` does not name is left alone, so an update of one
    /// storage attribute does not reset the others.
    pub fn begin_storage_update(&mut self, now: SystemTime, change: StorageChange) -> String {
        self.begin_update(now, ClusterChange::Storage(change))
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
        self.begin_update(
            now,
            ClusterChange::Connectivity {
                connectivity_info,
                zookeeper_access,
            },
        )
    }

    /// Record `change` as the cluster's one in-flight update and move it to
    /// `UPDATING`, handing back the ARN of the operation that will apply it.
    ///
    /// The one path that starts an update, so a cluster is `UPDATING` with an
    /// active operation ARN exactly while it holds a [`PendingUpdate`], whichever
    /// update operation recorded it.
    fn begin_update(&mut self, now: SystemTime, change: ClusterChange) -> String {
        let operation_arn = self.operation_arn(self.revision + 1);
        self.pending_update = Some(PendingUpdate {
            operation_arn: operation_arn.clone(),
            due: now + CLUSTER_UPDATE_DELAY,
            change,
        });
        self.state = CLUSTER_STATE_UPDATING.to_string();
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
            ClusterChange::Storage(change) => self.apply_storage_change(change),
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

    /// The ARN of the topic `topic_name` on this cluster.
    ///
    /// Amazon MSK names a topic as
    /// `arn:<partition>:kafka:<region>:<account>:topic/<cluster-name>/<cluster-uuid>/<topic-name>`
    /// (the resource an MSK IAM policy grants topic actions on), so the topic ARN
    /// is this cluster's own identity under the `topic` resource type. Derived
    /// from the stored cluster ARN, which keeps a topic's ARN in the same
    /// partition, region and account as the cluster that holds it.
    pub fn topic_arn(&self, topic_name: &str) -> String {
        match self.arn.split_once(":cluster/") {
            Some((prefix, identity)) => format!("{prefix}:topic/{identity}/{topic_name}"),
            // Unreachable for a registered cluster: `provisioned` mints every ARN
            // with a `cluster/<name>/<id>` resource. Rebuilt from the cluster's
            // own region and name rather than guessed, so the fallback is still a
            // well-formed topic ARN of this cluster.
            None => ::engine::emulator_arn(
                "kafka",
                &self.region,
                &format!("topic/{}/{topic_name}", self.name),
            ),
        }
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

/// The storage attributes a cluster operation reports, as `MutableClusterInfo`.
///
/// A volume size and throughput belong to the brokers, so they are reported
/// through `BrokerEBSVolumeInfo`; `StorageMode` belongs to the cluster. Every
/// other member is left unset: an operation describes the attributes it changes,
/// not the whole cluster.
fn storage_cluster_info(
    volume_size_gb: Option<i32>,
    provisioned_throughput: Option<ProvisionedThroughput>,
    storage_mode: &str,
) -> MutableClusterInfo {
    MutableClusterInfo {
        broker_ebs_volume_info: Some(vec![BrokerEBSVolumeInfo {
            // "The broker identifier must be set to the keyword ALL. This means
            // the changes apply to all the brokers in the cluster."
            // (`BrokerEBSVolumeInfo`'s model documentation.)
            kafka_broker_node_id: "ALL".to_string(),
            provisioned_throughput,
            volume_size_gb,
        }]),
        storage_mode: Some(storage_mode.to_string()),
        ..MutableClusterInfo::default()
    }
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

/// The resource type and id path of an MSK resource ARN —
/// `arn:<partition>:kafka:<region>:<account>:<type>/<id-path>` — or `None` when
/// `value` is not one.
///
/// Built on [`kafka_arn_resource`], the one ARN parse this module has, so an
/// operation that takes ANY resource ARN (the tagging operations) draws the
/// "this is not an ARN of mine" line exactly where the `is_*_arn` predicates
/// draw it.
pub fn kafka_resource_arn_parts(value: &str) -> Option<(&str, &str)> {
    let resource = kafka_arn_resource(value)?;
    let (resource_type, id_path) = resource.split_once('/')?;
    (!resource_type.is_empty() && !id_path.is_empty()).then_some((resource_type, id_path))
}

/// The resource type `value` names, for an operation that takes any MSK resource
/// ARN.
///
/// A value that is not an ARN of this service, or one whose type this service
/// mints in a shape `value` does not have, is [`TagTargetRejected::NotAnArn`] —
/// the request the service cannot parse, as distinct from the resource it cannot
/// find. The shape check goes through the same `is_*_arn` predicate the readers
/// of that resource type use, so one value is never a bad request to one
/// operation and a miss to another.
fn resource_type_of_arn(value: &str) -> Result<&str, TagTargetRejected> {
    let Some((resource_type, _)) = kafka_resource_arn_parts(value) else {
        return Err(TagTargetRejected::NotAnArn);
    };
    match resource_type {
        CLUSTER_RESOURCE_TYPE if !is_cluster_arn(value) => Err(TagTargetRejected::NotAnArn),
        CHANNEL_RESOURCE_TYPE if !is_channel_arn(value) => Err(TagTargetRejected::NotAnArn),
        REPLICATOR_RESOURCE_TYPE if !is_replicator_arn(value) => {
            Err(TagTargetRejected::NotAnArn)
        }
        other => Ok(other),
    }
}

/// Whether `value` is shaped like an MSK cluster ARN —
/// `arn:<partition>:kafka:<region>:<account>:cluster/<name>/<id>`.
///
/// A reader that models both errors answers `BadRequestException` for a value
/// that is not an ARN of this kind, and reserves `NotFoundException` for a
/// well-formed ARN no live cluster holds: that is the distinction the real
/// service draws between a request it cannot parse and a resource it cannot find.
pub fn is_cluster_arn(value: &str) -> bool {
    is_kafka_arn_for(value, "cluster")
}

/// Whether `value` is shaped like an MSK replicator ARN —
/// `arn:<partition>:kafka:<region>:<account>:replicator/<name>/<id>`.
///
/// Same distinction, same reason, as [`is_cluster_arn`]: a value that is not an
/// ARN of this kind is a `BadRequestException`, and a well-formed ARN no live
/// replicator holds is a `NotFoundException`.
pub fn is_replicator_arn(value: &str) -> bool {
    is_kafka_arn_for(value, "replicator")
}

/// Whether `value`'s resource part is `<kind>/<name>/<id>` — the shape both a
/// cluster and a replicator ARN take, so the two predicates above cannot
/// disagree about what a well-formed ARN of their kind is.
fn is_kafka_arn_for(value: &str, kind: &str) -> bool {
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
        (Some(found), Some(name), Some(id), None)
            if found == kind && !name.is_empty() && !id.is_empty()
    )
}

/// Whether `value` is shaped like an MSK VPC-connection ARN —
/// `arn:<partition>:kafka:<region>:<account>:vpc-connection/<cluster-owner-account>/<cluster-name>/<id>`,
/// the form [`VpcConnection::requested`] mints and the one the real service
/// puts on a multi-VPC connection.
///
/// Same split of the two client errors as [`is_cluster_arn`]: a value of any
/// other shape is a request the service cannot parse, not a resource it cannot
/// find.
pub fn is_vpc_connection_arn(value: &str) -> bool {
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
            segments.next()
        ),
        (Some("vpc-connection"), Some(owner), Some(cluster), Some(id), None)
            if !owner.is_empty() && !cluster.is_empty() && !id.is_empty()
    )
}

/// Whether `value` is shaped like an MSK cluster-operation ARN —
/// `arn:<partition>:kafka:<region>:<account>:cluster-operation/<cluster
/// name>/<cluster id>/<operation id>`, the form
/// [`Cluster::operation_arn`] mints and `ClusterOperationArn` carries.
///
/// Same distinction as above: a value that is not an ARN of this kind is a bad
/// request, while a well-formed one the registry does not hold is a resource
/// that cannot be found.
pub fn is_cluster_operation_arn(value: &str) -> bool {
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
            segments.next()
        ),
        (
            Some("cluster-operation"),
            Some(name),
            Some(cluster_id),
            Some(operation_id),
            None
        ) if !name.is_empty() && !cluster_id.is_empty() && !operation_id.is_empty()
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

/// Whether `value` is an EC2 resource id of `kind` (`"vpc"`, `"subnet"`,
/// `"sg"`): the prefix, a hyphen, and then either 8 or 17 lowercase
/// hexadecimal digits — the two published EC2 resource-id forms, the original
/// 8-character one and the longer 17-character one.
///
/// A handler that takes EC2 ids refuses anything else as a bad request: the
/// emulator holds no VPC inventory to look the id up in, but a value that is
/// not an id at all is a request the service cannot act on either way.
pub fn is_ec2_resource_id(kind: &str, value: &str) -> bool {
    let Some(suffix) = value
        .strip_prefix(kind)
        .and_then(|rest| rest.strip_prefix('-'))
    else {
        return false;
    };
    matches!(suffix.len(), 8 | 17)
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// The resource part of `value` when it is an ARN of THIS service in some
/// partition, region and account — `arn:<partition>:kafka:<region>:<account>:`
/// followed by the resource — and `None` otherwise. Borrowed from `value` and
/// split by the caller, so a predicate on the hot path of every reader allocates
/// nothing. The one ARN parse the `is_*_arn` predicates share, so they cannot
/// disagree about the five fields in front of the resource.
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

/// Why `name` is not a legal Apache Kafka topic name, or `None` when it is.
///
/// Apache Kafka's own rule (`org.apache.kafka.common.internals.Topic.validate`),
/// which the MSK topic operations sit in front of: a name is at most
/// [`MAX_TOPIC_NAME_LENGTH`] characters, is neither `.` nor `..`, and is made up
/// only of `[a-zA-Z0-9._-]`. The returned sentence names the member and the
/// constraint it broke, so a refusal says which part of the request was wrong.
pub fn topic_name_error(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("A topic name must not be empty.".to_string());
    }
    if name == "." || name == ".." {
        return Some(format!("The topic name {name} is reserved and cannot be used."));
    }
    let length = name.chars().count();
    if length > MAX_TOPIC_NAME_LENGTH {
        return Some(format!(
            "A topic name is at most {MAX_TOPIC_NAME_LENGTH} characters long, but this one is {length}."
        ));
    }
    if let Some(illegal) = name
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
    {
        return Some(format!(
            "A topic name may contain only ASCII alphanumerics, '.', '_' and '-', but this one contains {illegal:?}."
        ));
    }
    None
}
