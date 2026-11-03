// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-kafka-kafka service.
//!
//! The provisioned-cluster registry lives here, together with the renderings the
//! reporting operations derive from it: `DescribeCluster` / `ListClusters` build a
//! [`ClusterInfo`] with [`Cluster::to_cluster_info`], `DescribeClusterV2` /
//! `ListClustersV2` build the v2 `Cluster` with [`Cluster::to_cluster_v2`], and
//! `GetBootstrapBrokers` builds its endpoint strings with
//! [`Cluster::broker_endpoints`] plus the access predicates beside it, and
//! `ListNodes` renders one stored broker node with [`Cluster::to_node_info`].
//! Keeping them
//! next to the record is what makes the readers agree: they render the same stored
//! cluster the same way.
//!
//! A cluster comes in the two kinds the model's `ClusterType` names, and both live
//! in that one registry: `CreateCluster` and `CreateClusterV2`'s `Provisioned` body
//! mint a provisioned record with [`Cluster::provisioned`], while
//! `CreateClusterV2`'s `Serverless` body mints one with [`Cluster::serverless`],
//! whose [`ServerlessConfig`] is what the V2 readers derive the `Serverless` shape
//! from. One registry, so a cluster name is unique across both kinds and a reader
//! resolves either through the same lookups.
//!
//! The MSK configuration registry lives here for the same reason:
//! `CreateConfiguration` stores a [`ConfigurationRecord`] and both
//! `ListConfigurations` and `DescribeConfiguration` render it with
//! [`ConfigurationRecord::to_configuration`], so the writer and every reader
//! agree on one record. A configuration's revisions are an append-only history
//! on that record: `UpdateConfiguration` adds the next one with
//! [`ConfigurationRecord::append_revision`] and
//! `DescribeConfigurationRevision` reports a stored one through
//! [`ConfigurationRecord::revision`], so no update rewrites a revision an
//! earlier call already stored.
//!
//! A configuration's deletion is ASYNCHRONOUS, so it is accepted on the record
//! rather than applied to the registry: `DeleteConfiguration` moves it to
//! `DELETING` through [`ComAmazonawsKafkaKafkaState::begin_configuration_deletion`],
//! which records the instant the clock owes its removal, and the configuration
//! stays a resource every reader resolves and lists in that state until then.
//! The lookups ([`ComAmazonawsKafkaKafkaState::configuration_in`] for a read and
//! [`ComAmazonawsKafkaKafkaState::settled_configuration_in_mut`] for a call that
//! writes) are what materialise that removal, so every reader agrees about when
//! a deleted configuration is gone, and a call that CHANGES a configuration is
//! gated on the state it is in
//! ([`CONFIGURATION_STATES_A_CHANGE_IS_ACCEPTED_FROM`]) rather than reporting a
//! resource it still holds as a reference it cannot resolve.
//!
//! The topic registry lives beside it, keyed by (cluster ARN, topic name): a topic
//! belongs to exactly one cluster, so `CreateTopic`, `DescribeTopic`, `ListTopics`
//! and `DeleteTopic` all resolve the cluster first and then the topic, through
//! [`ComAmazonawsKafkaKafkaState::settled_topic_in`] (or
//! [`ComAmazonawsKafkaKafkaState::topics_in`] for a listing) — the one
//! accessor each, so every caller sees the transitions the clock already owed that
//! topic. A cluster leaving the registry takes its topics with it, so no topic can
//! outlive the cluster that holds it. Those operations serve a provisioned cluster
//! only: each refuses a serverless one on its kind, in the same words, through
//! [`serverless_topic_apis_unsupported`].
//!
//! A cluster reports the topics a running replicator REPLICATES onto it as well as
//! the ones it holds itself, and the topic readers answer from the one derivation
//! that puts the two together:
//! [`ComAmazonawsKafkaKafkaState::observable_topics_in`] for a listing and
//! [`ComAmazonawsKafkaKafkaState::observable_topic_in`] for one name. A replicated
//! topic is DERIVED from the source cluster's own topic rather than stored a second
//! time, so a listing and a describe of one cluster cannot disagree, and the
//! deletion of the source topic or of the replicator withdraws it without anything
//! having to remember to.
//!
//! A cluster's configuration also changes after creation: an update operation
//! (`UpdateConnectivity`, `UpdateStorage`, `UpdateBrokerStorage`,
//! `UpdateClusterKafkaVersion`, `UpdateClusterConfiguration`,
//! `UpdateSecurity`, `UpdateBrokerCount`) records a
//! [`PendingUpdate`] on the cluster, and [`Cluster::settle`] folds it into the
//! record once the clock owes its completion — so one place decides what every
//! reader then reports. The operation itself is kept in the cluster-operation
//! registry, which is what its `ClusterOperationArn` names and what
//! `DescribeClusterOperation` renders with
//! [`ClusterOperation::to_cluster_operation_info`] and
//! `DescribeClusterOperationV2` with
//! [`ClusterOperation::to_cluster_operation_v2`] — one stored record, two
//! renderings, so the V1 and V2 readers of one operation cannot disagree. A
//! cluster's whole history is derived once, by
//! [`ComAmazonawsKafkaKafkaState::cluster_operation_history`], which is what
//! `ListClusterOperations` pages: the listing reports the same records the
//! by-ARN readers resolve, in the one order
//! [`ClusterOperation::history_key`] defines.
//!
//! The channel registry lives beside it, keyed by channel ARN and indexed by
//! (cluster ARN, channel name) — a channel is a sub-resource of the cluster it
//! streams from, so it is only ever resolved through that cluster and disappears
//! with it. `DeleteChannel` accepts a channel's deletion through
//! [`ComAmazonawsKafkaKafkaState::begin_channel_deletion`], which moves it to
//! `DELETING` and records the instant the clock owes its removal, and the
//! channel lookups ([`ComAmazonawsKafkaKafkaState::settled_channel_in`] and the
//! by-ARN pair the tagging operations resolve through) materialise that removal
//! — so every reader agrees about when a deleted channel is gone.
//!
//! The replicator registry sits beside them on the same principle: a [`Replicator`]
//! holds the whole replication setup, and the readers derive their shapes from that
//! one record — [`Replicator::to_replicator_summary`] for `ListReplicators`, and
//! [`Replicator::kafka_cluster_descriptions`] /
//! [`Replicator::replication_info_descriptions`] for `DescribeReplicator`.
//!
//! The service's Apache Kafka version catalog lives here too
//! ([`KAFKA_VERSION_CATALOG`], with [`kafka_version_status`] and
//! [`compatible_target_versions`]), for the same reason: it is the single list
//! every version-aware operation answers from.
//!
//! A cluster's resource-based policy is held ON that cluster record, as a
//! [`ClusterPolicy`]: `PutClusterPolicy` attaches or replaces it through
//! [`Cluster::put_policy`], `GetClusterPolicy` reports the stored document and
//! its version, and `DeleteClusterPolicy` takes it off through
//! [`Cluster::take_policy`] — one stored document, so the writer and both
//! readers cannot disagree, and a cluster that leaves the registry takes its
//! policy with it.
//!
//! A cluster's SASL/SCRAM secrets are held ON the record for the same reason, as
//! the [`Cluster::scram_secret_arns`] set: `BatchAssociateScramSecret` adds to it
//! through [`Cluster::associate_scram_secrets`],
//! `BatchDisassociateScramSecret` removes from it through
//! [`Cluster::disassociate_scram_secrets`] and `ListScramSecrets` pages that one
//! set, so the two writers and the reader cannot disagree about which secrets a
//! cluster authenticates its clients with, and the secrets leave with the
//! cluster.
//!
//! The multi-VPC (PrivateLink) connection registry lives here too:
//! `CreateVpcConnection` mints a [`VpcConnection`] with
//! [`VpcConnection::requested`] and `DescribeVpcConnection` reports that same
//! stored record, while `ListVpcConnections` pages the region's connections and
//! renders each one with [`VpcConnection::to_vpc_connection`] — so a listed
//! connection and a described one are two views of one record.
//! `ListClientVpcConnections` pages the SAME registry narrowed to one target
//! cluster, rendering each record with
//! [`VpcConnection::to_client_vpc_connection`], so a cluster's client
//! connections are that same one record seen from the cluster's side.

/// The account's EC2 subnets, which the creates that place a resource in a client
/// VPC resolve what they are given against. They are not this service's resources,
/// so they are not held here — they are resolved, which is why that module sits
/// beside this one rather than in it.
pub mod network;

/// The authorization decision a cluster-scoped operation takes on the ARN its
/// request names, BEFORE the resolution below runs. It is decided from the request
/// alone, which is why it sits beside this module rather than in it — nothing it
/// reads is held here.
pub mod authorization;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Bound;
use std::time::{Duration, SystemTime};

use regex::RegexSet;

use crate::ops::types::{
    BadRequestException, BrokerCountUpdateInfo, BrokerEBSVolumeInfo, BrokerNodeGroupInfo,
    BrokerNodeInfo,
    BrokerSoftwareInfo,
    ChannelLoggingInfo, ClientAuthentication, ClientVpcConnection, ClusterInfo, ClusterOperationInfo,
    ClusterOperationV2,
    ClusterOperationV2Provisioned, ConflictException, Configuration,
    ConfigurationInfo, ConfigurationRevision, ConnectivityInfo, ConsumerGroupReplicationUpdate,
    CreateChannelInput,
    CreateClusterInput, CreateConfigurationInput, CreateReplicatorInput, CreateVpcConnectionInput,
    DescribeChannelOutput, EBSStorageInfo, EncryptionAtRest, EncryptionConfiguration,
    EncryptionInTransit, EncryptionInfo, IcebergDestinationConfiguration, JmxExporter,
    JmxExporterInfo, KafkaCluster, KafkaClusterDescription, KafkaClusterSummary, LogDelivery,
    LoggingInfo, MutableClusterInfo, NodeExporter, NodeExporterInfo, NodeInfo, OpenMonitoring,
    OpenMonitoringInfo, Prometheus, PrometheusInfo, Provisioned, ProvisionedRequest,
    ProvisionedThroughput, Rebalancing, ReplicationInfo, ReplicationInfoDescription,
    ReplicationInfoSummary, ReplicationStartingPosition, ReplicationStateInfo,
    ReplicationTopicNameConfiguration, ReplicatorSummary, S3DestinationConfiguration, Serverless,
    ServerlessClientAuthentication, ServerlessConnectivityInfo, ServerlessRequest, StorageInfo,
    TopicConfiguration, TopicInfo, TopicReplication, TopicReplicationUpdate, VpcConfig,
    ZookeeperAccess,
};

/// How long after a cluster create a new cluster becomes `ACTIVE`.
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

/// The `OperationType` an `UpdateBrokerType` call records, in the same form.
pub const OPERATION_TYPE_UPDATE_BROKER_TYPE: &str = "UPDATE_BROKER_TYPE";

/// The `OperationType` an `UpdateBrokerCount` call records, in the same form.
pub const OPERATION_TYPE_UPDATE_BROKER_COUNT: &str = "UPDATE_BROKER_COUNT";

/// The `OperationType` an `UpdateStorage` call records, which is the API name in
/// the SCREAMING_SNAKE_CASE form MSK's cluster-operation types take.
pub const OPERATION_TYPE_UPDATE_STORAGE: &str = "UPDATE_STORAGE";

/// The `OperationType` an `UpdateBrokerStorage` call records, in the same form.
pub const OPERATION_TYPE_UPDATE_BROKER_STORAGE: &str = "UPDATE_BROKER_STORAGE";

/// The `OperationType` an `UpdateConnectivity` call records, in the same form.
pub const OPERATION_TYPE_UPDATE_CONNECTIVITY: &str = "UPDATE_CONNECTIVITY";

/// The `OperationType` an `UpdateClusterKafkaVersion` call records, in the same
/// form.
pub const OPERATION_TYPE_UPDATE_CLUSTER_KAFKA_VERSION: &str = "UPDATE_CLUSTER_KAFKA_VERSION";

/// The `OperationType` an `UpdateClusterConfiguration` call records, in the same
/// form.
pub const OPERATION_TYPE_UPDATE_CLUSTER_CONFIGURATION: &str = "UPDATE_CLUSTER_CONFIGURATION";

/// The `OperationType` an `UpdateSecurity` call records, in the same form.
pub const OPERATION_TYPE_UPDATE_SECURITY: &str = "UPDATE_SECURITY";

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

/// The `ClusterState` of a cluster whose provisioning or repair did not succeed.
pub const CLUSTER_STATE_FAILED: &str = "FAILED";

/// The `ClusterState` of a cluster the service is repairing a broker of.
pub const CLUSTER_STATE_HEALING: &str = "HEALING";

/// The `ClusterState` of a cluster the service is applying maintenance to.
pub const CLUSTER_STATE_MAINTENANCE: &str = "MAINTENANCE";

/// The `ClusterState` of a cluster whose brokers are being rebooted.
pub const CLUSTER_STATE_REBOOTING_BROKER: &str = "REBOOTING_BROKER";

/// The `ClusterState` values a cluster delete is accepted from.
///
/// These are the two values of the model's `ClusterState` enum
/// (`crate::ops::types::CLUSTER_STATE_VALUES`) a cluster STAYS in until a caller
/// asks for something else; every other value (`CREATING`, `UPDATING`,
/// `DELETING`, `HEALING`, `MAINTENANCE`, `REBOOTING_BROKER`) is one the cluster
/// is still moving out of under the service's own control, and a destructive
/// call made while it is in flight is refused rather than applied. A cluster
/// whose provisioning failed is still deletable — otherwise nothing could ever
/// clear it.
pub const CLUSTER_STATES_DELETE_IS_ACCEPTED_FROM: [&str; 2] =
    [CLUSTER_STATE_ACTIVE, CLUSTER_STATE_FAILED];

/// Whether a cluster reported in `state` may be deleted, per
/// [`CLUSTER_STATES_DELETE_IS_ACCEPTED_FROM`]. One predicate, so every caller
/// that destroys a cluster decides it the same way.
pub fn is_deletable_cluster_state(state: &str) -> bool {
    CLUSTER_STATES_DELETE_IS_ACCEPTED_FROM.contains(&state)
}

/// Whether a cluster reported in `state` has its broker nodes yet.
///
/// A cluster the service is still provisioning has none — that is what
/// `CREATING` means — so an operation whose answer IS the brokers (their
/// endpoints, their node records) or which acts on one of them has nothing to
/// serve from until the cluster leaves that state. Every later state has the
/// brokers, including the ones a cluster passes through while an update or a
/// repair is applied to them.
pub fn cluster_has_broker_nodes(state: &str) -> bool {
    state != CLUSTER_STATE_CREATING
}

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

/// Shortest and longest a broker instance type may be, from the `length` trait on
/// `BrokerNodeGroupInfo$InstanceType` and `MutableClusterInfo$InstanceType`
/// (`__stringMin5Max32`, 5..32) in the model.
///
/// `UpdateBrokerTypeRequest$TargetInstanceType` is a plain `__string`, so the
/// boundary validator lets any value through: the constraint belongs to the
/// member the update WRITES, and [`is_broker_instance_type`] is what enforces it.
pub const INSTANCE_TYPE_MIN_LEN: usize = 5;
/// See [`INSTANCE_TYPE_MIN_LEN`].
pub const INSTANCE_TYPE_MAX_LEN: usize = 32;

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

/// How many entries one list of a serverless cluster's network configuration may
/// carry: `ServerlessRequest$VpcConfigs`, and each `VpcConfig`'s subnets and
/// security groups.
///
/// CHOSEN as a process-safety backstop, not a published AWS quota (the model
/// declares no `length` on any of those lists): the lists come from the request
/// and are STORED on the cluster, so without a ceiling one create would size
/// every later read of that cluster. Sized well above the handful of VPCs and
/// subnets a serverless cluster is really reached through.
pub const MAX_SERVERLESS_VPC_ENTRIES: usize = 16;

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

/// How long after `DeleteChannel` a channel marked `DELETING` is actually gone.
///
/// CHOSEN, not documented, for the same reason: `DeleteChannel` is documented to
/// move the channel through the non-terminal `DELETING` status and remove it once
/// the asynchronous delete completes, so the record must stay readable for a span
/// before it leaves the registry.
pub const CHANNEL_DELETION_DELAY: Duration = Duration::from_millis(500);

/// The `ChannelStatus` values this emulator reports, spelled as the model's enum
/// declares them. Named here so every channel operation reports the same
/// spellings. (`UPDATING`, `FAILED`, `SUSPENDING` and `SUSPENDED`, the model's
/// other values, belong to operations that are still scaffolds.)
pub const CHANNEL_STATUS_CREATING: &str = "CREATING";
/// The status of a channel whose delivery pipeline is up.
pub const CHANNEL_STATUS_ACTIVE: &str = "ACTIVE";
/// The status of a channel whose deletion has been accepted but not completed.
pub const CHANNEL_STATUS_DELETING: &str = "DELETING";

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

/// The most `KafkaClusters` entries one replicator RECORD may hold.
///
/// CHOSEN, not an AWS quota (the model declares `__listOfKafkaCluster` with no
/// `length` trait): a replicator's cluster list and its replication list are both
/// caller-sized, and every reader resolves each replication against the cluster
/// list, so without ceilings on BOTH one create would size the work of every
/// later read. `CreateReplicator` admits exactly
/// [`KAFKA_CLUSTERS_PER_REPLICATOR`] entries, so this is the backstop the
/// record-building path keeps rather than a bound a caller meets, and the product
/// with [`MAX_FLOWS_PER_REPLICATOR`] stays small.
pub const MAX_CLUSTERS_PER_REPLICATOR: usize = 16;

/// The most `ReplicationInfoList` entries one replicator may hold. CHOSEN for the
/// same reason, and on the same model evidence, as
/// [`MAX_CLUSTERS_PER_REPLICATOR`]: a replicator cannot hold more distinct
/// ordered cluster pairs than this anyway.
pub const MAX_FLOWS_PER_REPLICATOR: usize = 32;

/// How many Kafka clusters ONE replicator replicates between.
///
/// A replicator carries one source and one target — `KafkaClusters` is documented
/// as the "Kafka Clusters to use in setting up sources / targets for replication"
/// and every `ReplicationInfo` names one of each — so the list names exactly that
/// many, and a create naming any other number is refused with
/// [`KAFKA_CLUSTER_COUNT_MESSAGE`] before anything is minted. The model declares no
/// `length` on `__listOfKafkaCluster`, so the service is what states the count; the
/// behavioural record for a list of one is that refusal rather than a replicator.
pub const KAFKA_CLUSTERS_PER_REPLICATOR: usize = 2;

/// What MSK answers a `KafkaClusters` list that does not name
/// [`KAFKA_CLUSTERS_PER_REPLICATOR`] clusters: fixed text end to end, carrying
/// neither the submitted count nor the required one.
pub const KAFKA_CLUSTER_COUNT_MESSAGE: &str = "Too many or too few kafka clusters specified.";

/// What MSK answers a `KafkaClusters` list that names one Kafka cluster more
/// than once: the list is a SET of clusters — a replicator replicates between
/// distinct clusters — so a repeated cluster ARN is refused with this fixed
/// text, which names neither the repeated ARN nor its position.
pub const DUPLICATE_KAFKA_CLUSTERS_MESSAGE: &str = "Kafka cluster list contains duplicate cluster \
     ARNs. Please ensure all the cluster ARNs are unique.";

/// The fewest subnets ONE Kafka cluster's client VPC configuration may name
/// (`KafkaClusterClientVpcConfig$SubnetIds`, "The list of subnets in the client VPC
/// to connect to").
///
/// Amazon MSK creates the elastic network interfaces a replicator reaches a Kafka
/// cluster through one per Availability Zone, and a client VPC configuration spans
/// two or three of them — the same two-or-three-Availability-Zone placement MSK
/// requires of a cluster's own client subnets. The model declares no `length` on the
/// list, so the count is the service's rule: a list outside this range describes a
/// placement MSK does not admit and is refused with [`SUBNET_COUNT_MESSAGE`], which
/// is what the behavioural record shows for a list of one.
pub const MIN_CLIENT_VPC_SUBNETS: usize = 2;

/// The most that configuration may name, the other end of the same range.
pub const MAX_CLIENT_VPC_SUBNETS: usize = 3;

/// What MSK answers a client VPC configuration naming a number of subnets outside
/// [`MIN_CLIENT_VPC_SUBNETS`]`..=`[`MAX_CLIENT_VPC_SUBNETS`]: fixed text end to end,
/// carrying neither the submitted count, nor the subnet ids, nor the bounds.
pub const SUBNET_COUNT_MESSAGE: &str = "Too many or too few subnet ids specified.";

/// How long after a replication flow and a source topic it covers are both in
/// place the clock owes that topic's appearance on the flow's target cluster.
///
/// CHOSEN, not documented: AWS documents that a matching source topic becomes
/// visible on the target cluster but no propagation latency. The project fixes an
/// emulated clock-owed transition at 250–750 ms, so a caller that lists the target
/// cluster's topics immediately still sees the topic absent while one that waits
/// sees it — the same span, and the same reasoning, as
/// [`TOPIC_ACTIVATION_DELAY`].
pub const TOPIC_REPLICATION_DELAY: Duration = Duration::from_millis(500);

/// The most `TopicsToReplicate` / `TopicsToExclude` patterns one replication flow
/// may carry.
///
/// CHOSEN as a process-safety backstop, not an AWS quota (the model declares both
/// members as `__listOf__stringMax249`, with no `length` trait): the patterns are
/// caller-supplied and every listing of a target cluster matches that cluster's
/// replicated topics against them, so without a ceiling one create would size the
/// work of every later read. Sized far above the handful of patterns a real
/// replication configuration names.
pub const MAX_TOPIC_PATTERNS_PER_FLOW: usize = 100;

/// The longest one topic-selection pattern may be: the `length` the model puts on
/// the member of `TopicsToReplicate` / `TopicsToExclude`
/// (`__listOf__stringMax249`, whose member is `__stringMax249`, 0..249).
///
/// The framework's boundary validator refuses a longer value off the wire, with
/// the service's own constraint wording; this module checks it as well, so no
/// caller of it can hand a pattern of unbounded length to the regex compiler.
pub const MAX_TOPIC_PATTERN_LENGTH: usize = 249;

/// The most memory the matcher compiled from ONE of a flow's pattern lists may
/// take.
///
/// CHOSEN as a process-safety bound, not an AWS quota: the patterns come from a
/// request and the matcher compiled from them is kept on the stored flow, so this
/// is what bounds the memory one create can leave behind. A list is at most
/// [`MAX_TOPIC_PATTERNS_PER_FLOW`] patterns of [`MAX_TOPIC_PATTERN_LENGTH`]
/// characters — about 25 KiB of pattern text — and a matcher that would not fit
/// here makes the request a `BadRequestException` instead of a large allocation.
/// A tenth of the regex crate's own default, and far above what a real
/// replication configuration's handful of patterns compiles to.
pub const TOPIC_MATCHER_SIZE_LIMIT: usize = 1024 * 1024;

/// The most replication flows INTO one cluster a single reader of that cluster's
/// topics derives from.
///
/// CHOSEN as a process-safety backstop, not an AWS quota: a caller can register
/// [`MAX_REPLICATORS_IN_REGISTRY`] replicators of
/// [`MAX_FLOWS_PER_REPLICATOR`] flows each pointing at one cluster, so without a
/// cap on the PRODUCT one small listing would walk every one of them. Sized at the
/// flows a single replicator may carry, which is already far above the one or two
/// a real target cluster is fed by.
pub const MAX_REPLICATION_FLOWS_PER_TARGET: usize = MAX_FLOWS_PER_REPLICATOR;

/// The `ReplicationTopicNameConfigurationType` members, spelled as the model's
/// enum declares them. `PREFIXED_WITH_SOURCE_CLUSTER_ALIAS` names a replicated
/// topic after its source topic prefixed with the source cluster's alias, which is
/// what `KafkaClusterDescription$KafkaClusterAlias` ("Used to prefix names of
/// replicated topics") describes.
pub const TOPIC_NAME_PREFIXED_WITH_SOURCE_CLUSTER_ALIAS: &str =
    "PREFIXED_WITH_SOURCE_CLUSTER_ALIAS";
/// The other member: a replicated topic carries the same name as its source
/// topic, so a flow that asks for it mints no prefix.
pub const TOPIC_NAME_IDENTICAL: &str = "IDENTICAL";

/// What separates the source cluster's alias from the source topic's name in a
/// replicated topic's name, when the flow prefixes names with the alias:
/// `<source-cluster-alias>.<source-topic>`, the form Amazon MSK's replicated
/// topics carry.
pub const REPLICATED_TOPIC_SEPARATOR: char = '.';

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
/// than a description of the offending value. The generic one of the service's
/// fixed sentences; a few surfaces answer a more specific one for a cluster ARN
/// ([`CLUSTER_ARN_NOT_VALID_MESSAGE`]).
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

/// The variant name of the `.kraft` lines: the versions whose cluster metadata is
/// held by a KRaft controller quorum instead of Apache ZooKeeper.
pub const KAFKA_VERSION_VARIANT_KRAFT: &str = "kraft";

/// The variant name of the `.tiered` spelling: a ZooKeeper-mode version with
/// tiered storage, which MSK offers as a storage option of that same build (every
/// release from 3.6.0 on carries it without a spelling of its own).
pub const KAFKA_VERSION_VARIANT_TIERED: &str = "tiered";

/// Every Apache Kafka version this emulator's MSK knows, each published ONCE, in
/// the order the service publishes them: ascending by release, with a release's
/// base version ahead of the variant lines built on it.
///
/// Source: the Amazon MSK Developer Guide, "Supported Apache Kafka versions"
/// (<https://docs.aws.amazon.com/msk/latest/developerguide/supported-kafka-versions.html>),
/// which lists the versions MSK offers, the variants it offers them in (the
/// `.kraft` metadata-mode lines, the `.tiered` storage line, the `.link` line)
/// and the ones it has deprecated. A deprecated version is still a valid SOURCE
/// version — a cluster already running it can be upgraded away from it — but
/// never a target.
///
/// Where a version's published status and the upgrade matrix the service also
/// publishes disagree about it, the status that AGREES WITH THE MATRIX is the one
/// kept, because the matrix is the answer a caller acts on: `3.9.x` is offered as
/// an upgrade from every older ZooKeeper-mode version, so it is ACTIVE here, while
/// `4.2.x.kraft` is in no published target list, so it is DEPRECATED — which is
/// why the newest KRaft release the catalog names is not an upgrade target and
/// `4.1.x.kraft` is the terminal one.
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
    KafkaVersionEntry { version: "2.8.1", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.8.2.tiered", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "3.1.1", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "3.2.0", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "3.3.1", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "3.3.2", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "3.4.0", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "3.5.1", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "3.6.0", status: KAFKA_VERSION_ACTIVE },
    KafkaVersionEntry { version: "3.6.0.1", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "3.7.x", status: KAFKA_VERSION_ACTIVE },
    KafkaVersionEntry { version: "3.7.x.kraft", status: KAFKA_VERSION_ACTIVE },
    KafkaVersionEntry { version: "3.8.x", status: KAFKA_VERSION_ACTIVE },
    KafkaVersionEntry { version: "3.8.x.kraft", status: KAFKA_VERSION_ACTIVE },
    KafkaVersionEntry { version: "3.8.link", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "3.9.x", status: KAFKA_VERSION_ACTIVE },
    KafkaVersionEntry { version: "3.9.x.kraft", status: KAFKA_VERSION_ACTIVE },
    KafkaVersionEntry { version: "4.0.x.kraft", status: KAFKA_VERSION_ACTIVE },
    KafkaVersionEntry { version: "4.1.x.kraft", status: KAFKA_VERSION_ACTIVE },
    KafkaVersionEntry { version: "4.2.x.kraft", status: KAFKA_VERSION_DEPRECATED },
];

/// The versions a cluster running `source` can be upgraded to: the per-source
/// upgrade graph the catalog itself describes, in catalog order.
///
/// Amazon MSK upgrades a cluster's Apache Kafka version IN PLACE, never
/// downgrades, and never upgrades to a version it has deprecated — and an upgrade
/// stays inside the source's own LINE ([`kafka_version_line`]), because a line is
/// a different build of the broker rather than a later release of the same one.
/// So a target must be [`KAFKA_VERSION_ACTIVE`], sit on the SAME line as the
/// source and name a strictly newer release; the one cross-line move MSK offers
/// is the KRaft migration, from a ZooKeeper-mode version to the `.kraft` version
/// of its OWN release. A version with no newer active member of its line, and no
/// KRaft version of its release, is terminal and reports an EMPTY list.
///
/// `source` need not be catalogued: a cluster running a version this catalog does
/// not list is still placed against it by [`kafka_version_release`] and
/// [`kafka_version_line`].
pub fn compatible_target_versions(source: &str) -> Vec<String> {
    let release = kafka_version_release(source);
    let line = kafka_version_line(source);
    KAFKA_VERSION_CATALOG
        .iter()
        .filter(|entry| is_offered_kafka_version(entry.version))
        .filter(|entry| {
            let target_release = kafka_version_release(entry.version);
            match (line, kafka_version_line(entry.version)) {
                // The same line, further along it.
                (KafkaVersionLine::ZooKeeper, KafkaVersionLine::ZooKeeper)
                | (KafkaVersionLine::Kraft, KafkaVersionLine::Kraft) => target_release > release,
                // The KRaft migration: the same release, in KRaft metadata mode.
                (KafkaVersionLine::ZooKeeper, KafkaVersionLine::Kraft) => {
                    target_release == release
                }
                _ => false,
            }
        })
        .map(|entry| entry.version.to_string())
        .collect()
}

/// Which build of the broker a catalogued version is: the thing an upgrade stays
/// inside, per [`compatible_target_versions`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KafkaVersionLine {
    /// A version whose cluster metadata is held by Apache ZooKeeper — the plain
    /// releases, and the `.tiered` spelling of one, since tiered storage is a
    /// storage option of that same build rather than a build of its own.
    ZooKeeper,
    /// A `.kraft` version: cluster metadata held by a KRaft controller quorum.
    Kraft,
    /// Any other spelling MSK publishes a release under. It names a build of its
    /// own, with no upgrade path off it.
    Other,
}

/// The line `version` belongs to, read from the trailing non-numeric component
/// MSK spells the line with ([`KAFKA_VERSION_VARIANT_KRAFT`],
/// [`KAFKA_VERSION_VARIANT_TIERED`]).
///
/// `x` is not a line: it is MSK's name for "the newest patch of this minor", so
/// it belongs to the release ([`kafka_version_release`] reads it).
pub fn kafka_version_line(version: &str) -> KafkaVersionLine {
    match kafka_version_variant(version) {
        KAFKA_VERSION_VARIANT_KRAFT => KafkaVersionLine::Kraft,
        "" | KAFKA_VERSION_VARIANT_TIERED => KafkaVersionLine::ZooKeeper,
        _ => KafkaVersionLine::Other,
    }
}

/// The variant line `version` belongs to: `""` for a plain ZooKeeper-mode
/// release, and otherwise the trailing non-numeric component MSK names the line
/// with (`kraft`, `tiered`, `link`).
///
/// `x` is not a variant: it is MSK's name for "the newest patch of this minor",
/// so it belongs to the release ([`kafka_version_release`] reads it).
pub fn kafka_version_variant(version: &str) -> &str {
    version
        .rsplit('.')
        .next()
        .filter(|last| *last != "x" && last.parse::<u32>().is_err())
        .unwrap_or_default()
}

/// The numeric release `version` names, as the components that order one release
/// against another: `3.6.0` as `[3, 6, 0]`, `2.4.1.1` as `[2, 4, 1, 1]`, and a
/// `.x` component — "the newest patch of this minor" — as the largest patch there
/// can be, so `3.7.x` orders above every numbered `3.7` patch.
///
/// The variant component is NOT part of the release, so a line's base version and
/// its variants name the same release (`3.9.x` and `3.9.x.kraft` are both release
/// `3.9.x`), which is what makes the KRaft migration a move between lines at ONE
/// release. `Vec<u32>` compares lexicographically, so a release that is a prefix
/// of another (`2.4.1` of `2.4.1.1`) orders below it.
pub fn kafka_version_release(version: &str) -> Vec<u32> {
    version
        .split('.')
        .filter_map(|component| match component.parse::<u32>() {
            Ok(number) => Some(number),
            // "the latest patch of this minor": above every numbered patch.
            Err(_) if component == "x" => Some(u32::MAX),
            // The variant component, which names a line rather than a release.
            Err(_) => None,
        })
        .collect()
}

/// Where `version` sits in [`KAFKA_VERSION_CATALOG`], or `None` when MSK knows
/// no such version. The catalog publishes each version once, so this is both the
/// membership lookup and the position a `ListKafkaVersions` pagination token
/// resolves through.
pub fn kafka_version_index(version: &str) -> Option<usize> {
    KAFKA_VERSION_CATALOG
        .iter()
        .position(|entry| entry.version == version)
}

/// The support status the catalog publishes for `version`, or `None` when MSK
/// knows no such version.
pub fn kafka_version_status(version: &str) -> Option<&'static str> {
    KAFKA_VERSION_CATALOG
        .iter()
        .find(|entry| entry.version == version)
        .map(|entry| entry.status)
}

/// The sentence Amazon MSK answers a request naming an Apache Kafka version it
/// does not offer with: the fixed text `Unsupported KafkaVersion [`, `].
/// Valid values: [`, `]`, around two variable parts — the rejected version,
/// taken verbatim from the request, and the versions that ARE valid, read from
/// [`KAFKA_VERSION_CATALOG`] in its published order.
///
/// The listed set is exactly the set the check this refusal comes from admits
/// ([`kafka_version_index`]): every version the catalog publishes, a deprecated
/// one included, since a configuration may name a version existing clusters
/// still run. A list narrower than the gate would name a value the same request
/// is accepted with, which is worse than no list at all.
///
/// The valid-value list is the actionable half of this refusal, so it is
/// derived from the catalog rather than written out: a version the catalog gains
/// or loses is gained or lost here too.
pub fn unsupported_kafka_version_message(rejected: &str) -> String {
    let valid: Vec<&str> = KAFKA_VERSION_CATALOG
        .iter()
        .map(|entry| entry.version)
        .collect();
    format!(
        "Unsupported KafkaVersion [{rejected}]. Valid values: [{}]",
        valid.join(", ")
    )
}

/// Whether the service still OFFERS `version`: whether the catalog publishes it
/// and its status is [`KAFKA_VERSION_ACTIVE`]. This is what makes a version an
/// upgrade target and what a deprecation check reads.
pub fn is_offered_kafka_version(version: &str) -> bool {
    kafka_version_status(version) == Some(KAFKA_VERSION_ACTIVE)
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

/// The wire error code that refusal carries, and the status the model gives the
/// `NotFoundException` shape (`@httpError 404`).
///
/// Named here because an operation whose model declares NO `NotFoundException`
/// still answers one for a reference it cannot resolve — the service answers the
/// code whatever its published model lists — so such an operation carries it
/// through the unmodelled channel
/// ([`crate::scaffolded::Scaffolded::unmodeled`]) with these two values.
pub const NOT_FOUND_CODE: &str = "NotFoundException";
/// The status that refusal carries.
pub const NOT_FOUND_STATUS: u16 = 404;

/// The same refusal, as the replicator APIs phrase it: the input member that
/// failed to resolve, then [`RESOURCE_NOT_FOUND_MESSAGE`] and nothing else.
///
/// `member` is the member's name as the wire carries it (`replicatorArn`), so the
/// text names what the CALLER sent rather than anything about the serving
/// endpoint — which is the form the behavioural corpus records MSK's replicator
/// APIs answering, where the cluster APIs answer the bare sentence. Built here,
/// from one place, so every operation that resolves a replicator by ARN answers
/// one form instead of describing its own state.
pub fn parameter_not_found_message(member: &str) -> String {
    format!("Invalid parameter {member}: {RESOURCE_NOT_FOUND_MESSAGE}")
}

/// The message Amazon MSK answers a cluster call it refuses because the cluster
/// is not in a state that call is accepted from.
///
/// One fixed sentence per call, in the wording the service uses for that call —
/// they are not one template — with one variable part each: the state the cluster
/// is actually in, read from the stored record, so the caller is told why the
/// call was refused and when to retry it.
pub fn cluster_not_deletable_message(state: &str) -> String {
    format!("You can't delete cluster in {state} state.")
}

/// The same refusal, for the reader of a cluster's bootstrap endpoints: there
/// are no brokers to bootstrap against until the cluster has them. Fixed text
/// apart from the state, in the form the service words this family of refusals
/// in ("You can't … in <STATE> state.", as the delete above).
pub fn bootstrap_brokers_unavailable_message(state: &str) -> String {
    format!("You can't get bootstrap broker nodes for a cluster in {state} state.")
}

/// The same refusal, for the reader of a cluster's broker nodes, in the same
/// form. The subject is this reader's own; the form is the family's.
pub fn nodes_unavailable_message(state: &str) -> String {
    format!("You can't list nodes for a cluster in {state} state.")
}

/// The input member every replicator operation resolves its target through, named
/// as the wire carries it — what the refusals of those operations report as the
/// parameter at fault.
pub const REPLICATOR_ARN_MEMBER: &str = "replicatorArn";

/// The `ClusterType` of a cluster whose brokers this emulator provisions — one of
/// the two values the model's `ClusterType` enum names. It is STORED on the record
/// (see [`Cluster::cluster_type`]) rather than assumed by a reader, so a listing's
/// `ClusterTypeFilter` tests what each cluster IS.
pub const CLUSTER_TYPE_PROVISIONED: &str = "PROVISIONED";

/// The `ClusterType` of a cluster that has no customer broker nodes — the model's
/// other `ClusterType` value, which [`Cluster::serverless`] stores on the record
/// together with the [`ServerlessConfig`] the create submitted.
pub const CLUSTER_TYPE_SERVERLESS: &str = "SERVERLESS";

/// What an operation whose subject is the broker nodes answers for a cluster that
/// has none: the broker-node views (`ClusterInfo`, the bootstrap strings' access
/// paths) and the broker-reconfiguring updates all describe a provisioned
/// cluster, so each refuses a serverless one — and they refuse it in the same
/// words, so the service says one thing about one condition.
pub const PROVISIONED_ONLY_MESSAGE: &str =
    "The cluster is a serverless cluster; this operation applies to provisioned clusters only.";

/// The wire error code Amazon MSK answers a Topic Management call against a cluster
/// whose `ClusterType` is [`CLUSTER_TYPE_SERVERLESS`] with. The surface is not
/// offered for that cluster kind at all — topics on such a cluster are created and
/// read over the native Apache Kafka protocol — so the refusal is about the kind of
/// cluster and nothing else about the request.
///
/// This service's model declares no shape for the code, so it leaves through
/// [`crate::scaffolded::Scaffolded::unmodeled`] rather than as a modelled variant: a
/// client branches on the code, so the code the service answers is the code that
/// goes on the wire.
pub const SERVERLESS_TOPIC_APIS_CODE: &str = "ServerlessUnsupportedException";

/// The HTTP status that refusal carries. The call is refused because of what the
/// named cluster IS, which no retry changes and the caller fixes by addressing a
/// provisioned cluster (or the native protocol) instead — a client error, and 4xx
/// so no SDK retry classifier treats it as retryable.
pub const SERVERLESS_TOPIC_APIS_STATUS: u16 = 400;

/// The whole message of that refusal: fixed text end to end, carrying no value from
/// the request or from state.
pub const SERVERLESS_TOPIC_APIS_MESSAGE: &str = "Topic APIs are not supported on serverless clusters.";

/// The refusal every Topic Management operation answers for a serverless cluster.
///
/// Answered immediately after the cluster is resolved and BEFORE any further
/// state-dependent check (a broker-count bound, a topic lookup) and before anything
/// is registered, so the answer depends on the cluster's kind alone and a refused
/// call leaves the registry untouched. One helper, so the service says one thing
/// about one condition on every one of those operations.
pub fn serverless_topic_apis_unsupported<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::unmodeled(
        SERVERLESS_TOPIC_APIS_CODE,
        SERVERLESS_TOPIC_APIS_STATUS,
        SERVERLESS_TOPIC_APIS_MESSAGE,
    )
}

/// The one `NodeType` the model names: `ListNodes` reports a cluster's broker
/// nodes, so every node it lists is of this type.
pub const NODE_TYPE_BROKER: &str = "BROKER";

/// The largest page a listing may report, from the `range` on the `MaxResults`
/// shape (1..100) that this service's paginated listings bind. Also the page
/// size used when a request names none.
pub const MAX_RESULTS_LIMIT: i32 = 100;

/// Marker that prefixes a cluster page token's plaintext, so a token minted for
/// some other list cannot be mistaken for one of these.
const CLUSTER_PAGE_TOKEN_PREFIX: &str = "kafka:clusters:";

/// One broker node of a provisioned cluster: the DNS name clients reach it on,
/// and the placement the service gave it when it joined the cluster.
/// `GetBootstrapBrokers` renders every bootstrap string from these hosts, and
/// `ListNodes` reports each node through [`Cluster::to_node_info`].
#[derive(Debug, Clone)]
pub struct BrokerNode {
    /// 1-based broker id, as it appears in the `b-<id>` DNS label.
    pub id: i32,
    /// `b-<id>.<cluster>.<suffix>.kafka.<region>.amazonaws.com` — the port is
    /// appended per access path.
    pub endpoint_host: String,
    /// The one subnet of `BrokerNodeGroupInfo$ClientSubnets` this broker's
    /// network interface sits in. Stored rather than recomputed per read, so the
    /// placement a node was given does not move when the cluster's
    /// configuration is read again.
    pub client_subnet: String,
    /// The id of the elastic network interface the service created for this
    /// broker in [`Self::client_subnet`] — `ClientSubnets`' documentation is
    /// what says one is created per node.
    pub attached_eni_id: String,
    /// The private IPv4 address that interface answers on inside the client VPC.
    pub client_vpc_ip_address: String,
    /// When the node joined the cluster: the creation instant for a broker the
    /// create laid out, so a node added later carries its own.
    pub added_to_cluster_time: SystemTime,
}

impl BrokerNode {
    /// Lay out broker `id` of a cluster whose endpoints sit under `dns_suffix`:
    /// its endpoint host, the client subnet it is placed in, the network
    /// interface created for it there, and the instant it joined.
    ///
    /// `client_subnets` is the cluster's own `BrokerNodeGroupInfo$ClientSubnets`
    /// and the brokers are spread over it in order, which is what makes an
    /// `MSK` cluster's nodes sit across the Availability Zones those subnets
    /// name. The ids are pure functions of the node's endpoint host — unique per
    /// cluster and per broker — so a node keeps them for as long as the record
    /// holds it.
    fn placed(
        region: &str,
        name: &str,
        dns_suffix: &str,
        id: i32,
        client_subnets: &[String],
        now: SystemTime,
    ) -> Self {
        let endpoint_host = format!("b-{id}.{name}.{dns_suffix}.kafka.{region}.amazonaws.com");
        // The brokers are placed round-robin over the submitted subnets, so
        // broker 1 lands in the first. A cluster whose stored group names none
        // (which the model's `required` `ClientSubnets` does not allow a create
        // to submit) places the node nowhere rather than inventing a subnet.
        let client_subnet = client_subnets
            .get(usize::try_from(id - 1).unwrap_or(0) % client_subnets.len().max(1))
            .cloned()
            .unwrap_or_default();
        Self {
            attached_eni_id: ec2_long_id("eni", &format!("{endpoint_host}#eni")),
            client_vpc_ip_address: private_ipv4(&format!("{endpoint_host}#ip")),
            client_subnet,
            endpoint_host,
            id,
            added_to_cluster_time: now,
        }
    }
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
    /// The opaque cluster id plus cell every endpoint of this cluster sits
    /// under, as [`cluster_identity`] derived it from the cluster's ordinal:
    /// `b-1.<name>.<dns suffix>.kafka.<region>.amazonaws.com`. STORED, because
    /// a broker node added to the cluster after its creation has to be given an
    /// endpoint under the same suffix as the nodes already there.
    pub dns_suffix: String,
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
    /// The serverless configuration, for a record whose [`Self::cluster_type`] is
    /// [`CLUSTER_TYPE_SERVERLESS`]; `None` for a provisioned cluster. It is what
    /// the `Cluster` shape's `Serverless` member is derived from, so the per-kind
    /// detail a reader reports comes off the record rather than from the kind's
    /// name.
    pub serverless: Option<ServerlessConfig>,
    /// The resource-based policy attached to this cluster, or `None` while it
    /// has none. A policy is a sub-resource of exactly one cluster — the model
    /// addresses it as `/v1/clusters/{ClusterArn}/policy` — so it is stored on
    /// the cluster record and disappears when the cluster does.
    pub policy: Option<ClusterPolicy>,
    /// The AWS Secrets Manager secrets this cluster authenticates its SASL/SCRAM
    /// clients with, as `BatchAssociateScramSecret` associated them.
    ///
    /// A SET, so associating a secret the cluster already holds changes nothing
    /// and `ListScramSecrets` reports each secret exactly once; `BTreeSet` so
    /// that listing has one total order to page through, the way the cluster
    /// registry gives `ListClusters` one. Addressed as
    /// `/v1/clusters/{ClusterArn}/scram-secrets`, hence stored on the cluster
    /// record: the secrets leave with the cluster they authenticate clients to.
    pub scram_secret_arns: BTreeSet<String>,
}

/// The resource-based policy a cluster carries: the document a caller attached
/// and which revision of the policy it is, so the `CurrentVersion` every reader
/// reports is derived from the stored policy rather than echoed from a request.
#[derive(Debug, Clone)]
pub struct ClusterPolicy {
    /// The policy document exactly as submitted — the service stores a policy,
    /// it does not rewrite one.
    pub document: String,
    /// Which revision of this cluster's policy the stored document is: 1 for the
    /// first one attached, the next for each one that replaces it.
    pub revision: u64,
    /// The `CurrentVersion` token of [`Self::revision`], minted the same way a
    /// cluster's version token is but over the policy's own address, so a policy
    /// version and a cluster version are never the same token and the policy's
    /// changes exactly when the document does.
    pub current_version: String,
}

/// The configuration of a serverless cluster: the VPCs its clients reach it
/// through, the client authentication it accepts, and the one endpoint they
/// bootstrap on. Held beside the cluster record for the same reason the
/// provisioned configuration is: every reader derives its answer from this one
/// stored copy.
#[derive(Debug, Clone)]
pub struct ServerlessConfig {
    /// The `VpcConfigs` the create submitted, in the order it listed them.
    pub vpc_configs: Vec<VpcConfig>,
    /// The `ServerlessClientAuthentication` the create submitted, if any.
    pub client_authentication: Option<ServerlessClientAuthentication>,
    /// The single host a client bootstraps this cluster through:
    /// `boot-<id>.kafka-serverless.<region>.amazonaws.com`, the form Amazon MSK
    /// Serverless publishes. Reached on [`PORT_SASL_IAM`], since IAM access
    /// control is the only client authentication MSK Serverless offers, which is
    /// why the serverless arm of `GetBootstrapBrokers` reports one string.
    pub bootstrap_host: String,
}

impl ServerlessConfig {
    /// The configuration as the `Serverless` shape the V2 readers report.
    pub fn to_serverless(&self) -> Serverless {
        Serverless {
            vpc_configs: self.vpc_configs.clone(),
            client_authentication: self.client_authentication.clone(),
            // `ServerlessRequest` models no connectivity member, so a serverless
            // cluster runs the documented default network type and the reader
            // reports that effective setting rather than leaving the caller to
            // guess which default applied.
            connectivity_info: Some(ServerlessConnectivityInfo {
                network_type: Some(DEFAULT_NETWORK_TYPE.to_string()),
            }),
        }
    }
}

/// A cluster operation in flight: the change a caller asked for, the ARN of the
/// operation that will apply it, and the instant the clock owes its completion.
/// Stored, never slept on — [`Cluster::settle`] materialises it.
#[derive(Debug, Clone)]
pub struct PendingUpdate {
    /// The active operation's ARN while the operation runs — reported as
    /// `ClusterInfo$ActiveOperationArn` and as `Cluster$ActiveOperationArn` —
    /// and the `ClusterOperationArn` the update call answered with.
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
    /// `UpdateBrokerType`: the EC2 instance type every broker of the cluster
    /// runs on.
    BrokerType { instance_type: String },
    /// `UpdateBrokerCount`: how many broker nodes the cluster is to hold once
    /// the operation completes. The request always names one, so there is
    /// nothing optional about this change.
    BrokerCount { number_of_broker_nodes: i32 },
    /// `UpdateClusterKafkaVersion`: the Apache Kafka version the brokers run,
    /// and the custom configuration to apply on that new version when the
    /// request named one — `None` leaves the cluster's configuration as it is.
    KafkaVersion {
        kafka_version: String,
        configuration_info: Option<ConfigurationInfo>,
    },
    /// `UpdateClusterConfiguration`: the MSK configuration revision the brokers
    /// run their Apache Kafka version with. The request always names one, so
    /// there is nothing optional about this change.
    Configuration { configuration_info: ConfigurationInfo },
    /// `UpdateSecurity`: the client authentication the cluster accepts, the
    /// encryption it runs with, or both. `None` means the request left that
    /// setting alone.
    Security {
        client_authentication: Option<ClientAuthentication>,
        encryption_info: Option<EncryptionInfo>,
    },
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
    /// A `ClusterType` value: which kind of cluster the operation acted on,
    /// taken off that cluster's record when the operation was filed. The V2
    /// reader reports it as `ClusterOperationV2$ClusterType` and derives the
    /// per-kind detail arm from it, so the kind is stored rather than assumed
    /// from what filed the operation.
    pub cluster_type: String,
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

    /// The instant this operation was filed, in whole milliseconds since the
    /// Unix epoch — the granularity a listing's pagination token can carry, so
    /// it is also the granularity [`Self::history_key`] orders by.
    pub fn creation_millis(&self) -> u128 {
        self.creation_time
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_millis())
    }

    /// The key a cluster's operation history is ordered by: the instant the
    /// operation was filed, latest first, with its own ARN breaking a tie so
    /// the order is TOTAL.
    ///
    /// A listing resumes by comparing this key against the one its page token
    /// carries, so the order the page is built in and the order the token
    /// resumes in are the same key rather than two that can drift apart.
    pub fn history_key(&self) -> (std::cmp::Reverse<u128>, &str) {
        (std::cmp::Reverse(self.creation_millis()), self.arn.as_str())
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

    /// The same stored operation as `ClusterOperationV2`, the shape
    /// `DescribeClusterOperationV2` reports: the members every cluster kind
    /// shares, plus the one per-kind detail arm this operation's
    /// [`Self::cluster_type`] names.
    ///
    /// Derived from the SAME record [`Self::to_cluster_operation_info`] renders
    /// and at the same reading instant, so the V1 and V2 views of one operation
    /// cannot disagree about when it started, what it changes or how far along
    /// it is.
    pub fn to_cluster_operation_v2(&self, now: SystemTime) -> ClusterOperationV2 {
        ClusterOperationV2 {
            cluster_arn: Some(self.cluster_arn.clone()),
            cluster_type: Some(self.cluster_type.clone()),
            start_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            end_time: (now >= self.end_due)
                .then(|| ::aws_smithy_types::DateTime::from(self.end_due)),
            // Nothing fails an emulated update, so there is no error to describe.
            error_info: None,
            operation_arn: Some(self.arn.clone()),
            operation_state: Some(self.operation_state(now).to_string()),
            operation_type: Some(self.operation_type.clone()),
            provisioned: (self.cluster_type == CLUSTER_TYPE_PROVISIONED).then(|| {
                ClusterOperationV2Provisioned {
                    // No step list, for the reason the V1 view reports none:
                    // this emulator runs no broker-by-broker steps.
                    operation_steps: None,
                    source_cluster_info: Some(self.source_cluster_info.clone()),
                    target_cluster_info: Some(self.target_cluster_info.clone()),
                    // A VPC connection is described only by the operations that
                    // manage one.
                    vpc_connection_info: None,
                }
            }),
            // The serverless arm reports a serverless cluster's connectivity
            // before and after the operation, and every operation this registry
            // holds was filed by an update that a serverless cluster is refused
            // on, so no stored operation has that detail to report.
            serverless: None,
        }
    }
}

/// A stored MSK configuration (a `server.properties` bundle a cluster can be
/// created with), holding every property its readers report so each one is
/// computed from the record rather than echoed from a request.
///
/// A configuration is revisioned: `CreateConfiguration` stores revision 1 and an
/// update appends the next one, so the record keeps the revisions rather than a
/// single body.
#[derive(Debug, Clone)]
pub struct ConfigurationRecord {
    pub arn: String,
    pub name: String,
    /// The region the configuration lives in: the signing region of the request
    /// that created it, and the region segment of its ARN. A configuration name
    /// is unique per account and region, so this is half of its registry key.
    pub region: String,
    /// The description this configuration has, or `None` when it has none — the
    /// request either omitted `Description` or carried an empty one, which is
    /// the same thing ([`stored_description`]). Every reader whose own
    /// `Description` member is optional reports no description by OMITTING the
    /// member.
    pub description: Option<String>,
    /// The Apache Kafka versions the configuration may be used with, as
    /// submitted; empty when the request named none.
    pub kafka_versions: Vec<String>,
    pub creation_time: SystemTime,
    /// A `ConfigurationState` value. A configuration AWS accepts is usable at
    /// once, so it is created `ACTIVE`; a delete moves it to `DELETING` through
    /// [`Self::begin_deletion`].
    pub state: String,
    /// Set by `DeleteConfiguration`: the instant the clock owes this
    /// configuration's removal from the registry. `None` for a configuration
    /// nobody has deleted. Stored, never slept on.
    pub removal_due: Option<SystemTime>,
    /// Every revision in revision order, so the first is revision 1 and the last
    /// is the latest. Never empty: a configuration exists only once its first
    /// revision has been stored.
    pub revisions: Vec<ConfigurationRevisionRecord>,
}

/// One revision of a stored configuration: the `server.properties` body plus the
/// description and instant that went with it.
#[derive(Debug, Clone)]
pub struct ConfigurationRevisionRecord {
    /// 1-based revision number, as `ConfigurationRevision$Revision` reports it.
    pub revision: i64,
    pub creation_time: SystemTime,
    /// The description the call that stored this revision gave it, or `None`
    /// when it gave none ([`stored_description`]) —
    /// `ConfigurationRevision$Description` is optional, so a revision with no
    /// description is reported by omitting the member.
    pub description: Option<String>,
    /// The `server.properties` contents exactly as submitted, kept as bytes
    /// because the member is a blob and may hold any byte string.
    pub server_properties: Vec<u8>,
}

/// The state a configuration this emulator accepts is in, from the
/// `ConfigurationState` enum (`ACTIVE | DELETING | DELETE_FAILED`): AWS returns a
/// freshly created configuration ready to use, so it starts `ACTIVE` and the
/// clock owes it no transition.
pub const CONFIGURATION_STATE_ACTIVE: &str = "ACTIVE";

/// The `ConfigurationState` a configuration a delete accepted is reported in,
/// the same way a deleted cluster is reported `DELETING`. The delete is
/// asynchronous, so this state is STORED on the record
/// ([`ConfigurationRecord::begin_deletion`]) and reported by every reader until
/// the clock owes the removal: a configuration on its way out is still a
/// resource of this endpoint, not an unresolvable reference.
pub const CONFIGURATION_STATE_DELETING: &str = "DELETING";

/// How long after `DeleteConfiguration` a configuration marked `DELETING` is
/// actually gone from the registry.
///
/// CHOSEN, not documented: Amazon MSK answers the delete with the non-terminal
/// `DELETING` state and completes the removal on its own, which takes as long as
/// it takes. The span is never zero, so a caller that retries its update right
/// after the delete still meets the configuration in `DELETING` — and short
/// enough that a caller polling the deletion to completion sees it leave the
/// registry.
pub const CONFIGURATION_DELETION_DELAY: Duration = Duration::from_secs(3);

/// The description a configuration or one of its revisions stores for the
/// `Description` a request submitted: the value where the request carried one,
/// and NONE where it omitted the member or carried an empty string.
///
/// An empty description is no description: Amazon MSK describes a configuration
/// created with `Description: ""` back with the member ABSENT, exactly as it
/// describes one created without the member at all, so the two requests store
/// the same thing and no reader has to decide between them. Applied on the two
/// paths that store a description (a create and an append), so the record holds
/// one normalised value whichever call wrote it.
pub fn stored_description(submitted: Option<String>) -> Option<String> {
    submitted.filter(|description| !description.is_empty())
}

/// The `ConfigurationState` values a call that CHANGES a configuration is
/// accepted from: a configuration is only ever changed while it is `ACTIVE`,
/// since the other two states (`DELETING`, `DELETE_FAILED`) are a configuration
/// the service is taking out. Named here so the update and the delete gate on
/// one set and report the one refusal
/// ([`configuration_not_in_accepted_state`]) built from it.
pub const CONFIGURATION_STATES_A_CHANGE_IS_ACCEPTED_FROM: [&str; 1] =
    [CONFIGURATION_STATE_ACTIVE];

/// How Amazon MSK refuses a call against a resource whose current state that
/// call is not accepted from: one fixed sentence that names the set of states it
/// IS accepted from, which is what tells such a refusal apart from a reference
/// the service cannot resolve at all
/// ([`CONFIGURATION_ARN_NOT_FOUND_MESSAGE`]).
///
/// `accepted` is that set, in the spelling the `ConfigurationState` enum
/// declares — so the sentence is built from the gate the handler applied rather
/// than written out beside it, and a gate that admits more than one state says
/// so.
pub fn configuration_not_in_accepted_state(accepted: &[&str]) -> BadRequestException {
    BadRequestException {
        // The configuration the `Arn` names is what the call was refused on, as
        // the cluster refusals name the cluster's own ARN member.
        invalid_parameter: Some("Arn".to_string()),
        message: Some(format!(
            "This operation is only valid for resources that are in one of the following states :[{}]",
            accepted.join(", ")
        )),
    }
}

/// What Amazon MSK answers any configuration operation whose configuration
/// reference — its `Arn`, or that `Arn` together with the `Revision` of it being
/// named — resolves to nothing it holds.
///
/// A CONSTANT of the service, not built from the request: every operation of the
/// configuration family reports a reference it cannot resolve as a bad request
/// with this one fixed sentence — not as the `NotFoundException` the other
/// resource families answer — which is what the behavioural corpus records the
/// real service answering, for a configuration that was never minted or already
/// deleted and for a revision number of an existing configuration that was never
/// issued alike. The parameter at fault is named by
/// `BadRequestException$InvalidParameter`, not by the message.
pub const CONFIGURATION_ARN_NOT_FOUND_MESSAGE: &str = "Configuration ARN does not exist.";

/// The refusal every configuration operation answers a configuration reference
/// that resolves to nothing this endpoint's region holds with: a
/// `BadRequestException` (HTTP 400) carrying
/// [`CONFIGURATION_ARN_NOT_FOUND_MESSAGE`] and naming `arn` as the parameter at
/// fault. The reference is the `Arn` alone where the operation names only a
/// configuration, and the `Arn` plus the `Revision` where it names one revision
/// of one, since a revision number the configuration never issued resolves to a
/// revision just as little as an absent configuration resolves to a
/// configuration.
///
/// Built HERE rather than in each handler so the readers, the update, the delete
/// and the cluster updates that apply a configuration cannot drift apart on how
/// an unresolvable configuration reference is reported: one condition, one
/// envelope, whichever operation met it.
pub fn configuration_arn_not_found() -> BadRequestException {
    BadRequestException {
        // `InvalidParameter` reports the parameter as the API names it on the
        // wire — the configuration operations bind their `Arn` label as `arn` —
        // not as the Smithy structure member is spelled.
        invalid_parameter: Some("arn".to_string()),
        message: Some(CONFIGURATION_ARN_NOT_FOUND_MESSAGE.to_string()),
    }
}

/// What Amazon MSK answers a create whose name is already taken: ONE fixed
/// sentence for every resource the service names by name, disclosing neither the
/// name submitted nor the resource already holding it.
///
/// A CONSTANT of the service, not built from the request: the conflict is about
/// the name being taken, and the caller already knows which name it sent, so
/// nothing of the request or of the stored resource reaches the text. The
/// request member at fault is named by `ConflictException$InvalidParameter`
/// instead.
pub const NAME_ALREADY_EXISTS_MESSAGE: &str = "A resource with this name already exists.";

/// The refusal a create answers a name another resource of its kind already
/// holds with: a `ConflictException` (HTTP 409) carrying
/// [`NAME_ALREADY_EXISTS_MESSAGE`] and naming, in `invalidParameter`, the
/// request member that carried the name — by its wire name, which is what the
/// API documents and what the caller's own request holds.
///
/// Built HERE rather than in each create handler because the sentence is the
/// service's, not the resource type's: two creates refusing a taken name must
/// not answer differently.
pub fn name_already_exists(parameter: &str) -> ConflictException {
    ConflictException {
        invalid_parameter: Some(parameter.to_string()),
        message: Some(NAME_ALREADY_EXISTS_MESSAGE.to_string()),
    }
}

/// How this emulator refuses a delete of a configuration a cluster still uses:
/// one fixed sentence about the condition, in the same request-independent form
/// as [`RESOURCE_NOT_FOUND_MESSAGE`] (the apostrophe is U+2019, as the service
/// writes it). UNVERIFIED wording — no recording of this path was available, so
/// unlike [`CONFIGURATION_ARN_NOT_FOUND_MESSAGE`] this is the condition stated
/// in the service's style rather than text observed from Amazon MSK, and it is
/// held here, in one place, to be reconciled against the real sentence once a
/// recording of it exists. Which clusters hold the configuration is a listing
/// the caller gets from the cluster readers, so the refusal describes the
/// condition rather than this endpoint's registry.
pub const CONFIGURATION_IN_USE_MESSAGE: &str =
    "The configuration is in use by one or more clusters and can\u{2019}t be deleted.";

/// Configurations one account may hold in one region, from the Amazon MSK
/// service quota "Number of configurations per account" (100, per Region). A
/// create past it is refused with the modelled `TooManyRequestsException`, so the
/// registry's cardinality is bounded rather than growing with the number of
/// calls.
pub const MAX_CONFIGURATIONS_PER_REGION: usize = 100;

/// Absolute ceiling on the configuration registry across every region a caller
/// signs for, for the same reason [`MAX_CLUSTERS_IN_REGISTRY`] exists: the
/// signing region comes from the request, so the per-region quota alone would
/// still let a caller grow the store by naming new regions.
pub const MAX_CONFIGURATIONS_IN_REGISTRY: usize = MAX_CONFIGURATIONS_PER_REGION * 64;

/// How many revisions this emulator keeps for one configuration.
///
/// CHOSEN as a process-safety backstop, not an AWS quota: every update call
/// appends a revision holding the body the request submitted, so an unbounded
/// history would grow the store with the number of calls. Sized well past any
/// realistic client script's update count, and enforced by refusing the append
/// rather than evicting a revision, because a stored revision is immutable: a
/// reader of an earlier revision must still see the body it was stored with.
pub const MAX_REVISIONS_PER_CONFIGURATION: usize = 100;

/// How many bytes of `server.properties` one stored configuration revision may
/// hold.
///
/// CHOSEN as a process-safety backstop, not an AWS quota: the modelled member is
/// a blob with no length constraint, so without a ceiling a single call could
/// store an arbitrarily large body. Sized far past any realistic
/// `server.properties` file, which holds at most a few hundred short properties.
pub const MAX_SERVER_PROPERTIES_BYTES: usize = 256 * 1024;

/// Whether a submitted `server.properties` body is longer than this emulator
/// stores for one revision. The one test both paths that store a body (a create
/// and an update) apply, so a stored body is bounded whichever call stored it.
pub fn server_properties_too_large(body: &[u8]) -> bool {
    body.len() > MAX_SERVER_PROPERTIES_BYTES
}

/// How many bytes of policy document one cluster's resource-based policy may
/// hold.
///
/// CHOSEN as a process-safety backstop, not an AWS quota: the modelled member is
/// a string with no length constraint, so without a ceiling one call could store
/// an arbitrarily large document. Sized far past any realistic resource-based
/// policy, which holds at most a few dozen short statements.
pub const MAX_CLUSTER_POLICY_BYTES: usize = 256 * 1024;

/// How many SASL/SCRAM secrets one cluster may hold in this emulator.
///
/// CHOSEN as a process-safety backstop, not an AWS quota: the secret ARNs come
/// from the request and the modelled list carries no length constraint, so
/// without a ceiling a caller could grow one cluster's secret set one call at a
/// time. Sized well above any realistic client script.
pub const MAX_SCRAM_SECRETS_PER_CLUSTER: usize = 1_000;

/// The message Amazon MSK answers a SCRAM-secret operation against a cluster
/// whose client authentication does not turn SASL/SCRAM on.
///
/// A cluster holds SCRAM secrets only to authenticate SASL/SCRAM clients with,
/// so the condition is about the cluster's configuration and nothing else about
/// the request: the whole sentence is fixed text, with no cluster ARN, secret
/// ARN or other request value in it. The parameter at fault is reported through
/// `BadRequestException$InvalidParameter`.
pub const SASL_SCRAM_NOT_ENABLED_MESSAGE: &str =
    "SASL/SCRAM client authentication must be enabled for the cluster.";

/// The `ClusterState` values a SCRAM-secret operation acts on a cluster's secret
/// set from.
///
/// A cluster's SCRAM secrets are credentials its BROKERS authenticate clients
/// with, so an operation that reaches that set is served only from a state in
/// which the cluster has brokers that are not being taken away from it: every
/// value of the model's `ClusterState` enum (`crate::ops::types::CLUSTER_STATE_VALUES`)
/// except the one in which the cluster has no brokers yet (`CREATING`) and the
/// ones in which it is losing or has lost them (`DELETING`, `FAILED`). Spelled
/// in the order the refusal sentence lists them, which is the order
/// [`invalid_cluster_state_for_scram_message`] builds it from.
pub const CLUSTER_STATES_SCRAM_SECRETS_ARE_SERVED_FROM: [&str; 5] = [
    CLUSTER_STATE_ACTIVE,
    CLUSTER_STATE_UPDATING,
    CLUSTER_STATE_MAINTENANCE,
    CLUSTER_STATE_HEALING,
    CLUSTER_STATE_REBOOTING_BROKER,
];

/// Whether a cluster reported in `state` serves a SCRAM-secret operation, per
/// [`CLUSTER_STATES_SCRAM_SECRETS_ARE_SERVED_FROM`]. One predicate, so every
/// operation that reaches a cluster's secret set decides it the same way.
pub fn scram_secrets_served_from_cluster_state(state: &str) -> bool {
    CLUSTER_STATES_SCRAM_SECRETS_ARE_SERVED_FROM.contains(&state)
}

/// What a SCRAM-secret operation DOES to the secret set, as the state refusal's
/// sentence names it: the association's verb phrase, and the listing's.
pub const SCRAM_SECRET_ACTION_ASSOCIATE: &str = "associate secrets";
/// See [`SCRAM_SECRET_ACTION_ASSOCIATE`].
pub const SCRAM_SECRET_ACTION_LIST: &str = "list secrets";

/// The message a SCRAM-secret operation named by `action` (one of
/// [`SCRAM_SECRET_ACTION_ASSOCIATE`], [`SCRAM_SECRET_ACTION_LIST`]) is refused
/// with on a cluster whose state does not serve it.
///
/// Fixed text: the states come from
/// [`CLUSTER_STATES_SCRAM_SECRETS_ARE_SERVED_FROM`] and the verb phrase from the
/// operation, so nothing of the request — not the cluster ARN, not a secret ARN,
/// not the cluster's own state — is interpolated. Built here rather than written
/// out per operation, so the sentence and the set it names cannot drift apart.
pub fn invalid_cluster_state_for_scram_message(action: &str) -> String {
    format!(
        "Invalid cluster state. Cluster must be in the {} state to {action}.",
        comma_or_list(&CLUSTER_STATES_SCRAM_SECRETS_ARE_SERVED_FROM)
    )
}

/// `values` as English lists them in a sentence: separated by commas, with `or`
/// before the last.
fn comma_or_list(values: &[&str]) -> String {
    match values.split_last() {
        None => String::new(),
        Some((last, [])) => (*last).to_string(),
        Some((last, rest)) => format!("{}, or {last}", rest.join(", ")),
    }
}

/// Why a batch of SASL/SCRAM secrets was not associated with a cluster. Decided
/// on the one association path ([`Cluster::associate_scram_secrets`]) rather
/// than by each caller, and leaving the stored set exactly as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScramSecretsRejected {
    /// How many secrets the cluster already holds.
    pub held: usize,
    /// How many of the request's secrets the cluster does not hold yet.
    pub added: usize,
    /// What the two would give it — more than
    /// [`MAX_SCRAM_SECRETS_PER_CLUSTER`], which is why the batch was refused.
    pub resulting: usize,
}

/// Why an append to a configuration's revision history was refused. Decided on
/// the one append path ([`ConfigurationRecord::append_revision`]) rather than by
/// each caller, and leaving the history exactly as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionRejected {
    /// The configuration already holds [`MAX_REVISIONS_PER_CONFIGURATION`]
    /// revisions.
    HistoryFull,
    /// The submitted body is longer than [`MAX_SERVER_PROPERTIES_BYTES`].
    BodyTooLarge,
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
    /// The account that requested the connection, as the account segment of its
    /// own ARN names it. Reported as `ClientVpcConnection$Owner` by the cluster's
    /// listing of its client connections.
    pub owner: String,
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
    /// Set by `DeleteChannel`: the instant the clock owes this channel's removal
    /// from the registry. `None` for a channel nobody has deleted.
    pub removal_due: Option<SystemTime>,
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
    /// The cluster operation carrying out this channel's delete, minted once when
    /// the delete is accepted. `None` until then, so `DeleteChannel` and a reader
    /// inside the `DELETING` window report the same in-flight operation however
    /// often the idempotent delete is repeated.
    pub deletion_operation_arn: Option<String>,
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
/// The `ReplicatorState` value a deleted replicator is reported in — the state
/// `DeleteReplicator` answers with, another member of the same model enum.
pub const REPLICATOR_STATE_DELETING: &str = "DELETING";
/// The `ReplicatorState` value a replicator holds while a configuration change
/// it accepted is being rolled out, until the clock owes it
/// [`REPLICATOR_STATE_RUNNING`] again — another member of the same model enum.
pub const REPLICATOR_STATE_UPDATING: &str = "UPDATING";
/// The `ReplicatorState` value a replicator whose provisioning failed converges
/// to — the terminal member of the same model enum. A replicator that reaches it
/// reports WHY in `StateInfo` ([`Replicator::state_info`]).
pub const REPLICATOR_STATE_FAILED: &str = "FAILED";

/// The `ReplicationStateInfo$Code` Amazon MSK reports for a replicator it could
/// not provision because an Amazon MSK cluster its setup names does not exist.
/// A fixed token, not built from the request.
pub const REPLICATION_CODE_INVALID_KAFKA_CLUSTER: &str = "InvalidInput.InvalidKafkaCluster";

/// The `ReplicationStateInfo$Message` that goes with
/// [`REPLICATION_CODE_INVALID_KAFKA_CLUSTER`]: one fixed sentence, whose one
/// variable part is the cluster ARN the request named.
pub fn invalid_kafka_cluster_message(cluster_arn: &str) -> String {
    format!("The specified Amazon MSK Cluster does not exist. Cluster ARN: {cluster_arn}")
}

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

/// Which of a source cluster's topics one replication flow replicates: the
/// compiled form of that flow's `TopicsToReplicate` and `TopicsToExclude`.
///
/// Both members are documented as a "List of regular expression patterns", and a
/// pattern describes a WHOLE topic name rather than a part of one, so each is
/// anchored before it is compiled. A topic is replicated when some
/// `TopicsToReplicate` pattern describes it and no `TopicsToExclude` pattern does
/// — one rule, so the flow that decides a listing and the flow that decides a
/// single read cannot disagree.
///
/// Compiled ONCE, when the configuration it comes from is stored, and kept on the
/// replicator record beside that configuration: the patterns come from a request,
/// so compiling them per read would let one create size the work of every later
/// listing of the target cluster.
#[derive(Debug, Clone, Default)]
pub struct TopicFilter {
    replicate: RegexSet,
    exclude: RegexSet,
}

impl TopicFilter {
    /// The filter a stored `TopicReplication` describes.
    pub fn of(topic_replication: &TopicReplication) -> Self {
        Self {
            replicate: compile_topic_patterns(&topic_replication.topics_to_replicate),
            exclude: compile_topic_patterns(
                topic_replication.topics_to_exclude.as_deref().unwrap_or(&[]),
            ),
        }
    }

    /// Whether the flow replicates the topic `name`.
    pub fn covers(&self, name: &str) -> bool {
        self.replicate.is_match(name) && !self.exclude.is_match(name)
    }
}

/// `pattern` as the anchored expression a topic-selection pattern means: the
/// pattern describes a topic's whole name, and the group keeps an alternation
/// inside it from reaching past the anchors.
fn anchored_topic_pattern(pattern: &str) -> String {
    format!("^(?:{pattern})$")
}

/// Why `pattern` cannot be used to select topics, or `None` when it compiles.
///
/// The one rule a flow's patterns are admitted by, so `CreateReplicator` and
/// `UpdateReplicationInfo` refuse the same values — and a stored configuration
/// holds only patterns the readers can actually match topics against, rather than
/// quietly replicating nothing. A pattern longer than
/// [`MAX_TOPIC_PATTERN_LENGTH`] is refused before it is compiled, so no
/// caller-sized string reaches the regex compiler — a backstop behind the
/// boundary validator, which refuses that length off the wire.
pub fn topic_pattern_error(pattern: &str) -> Option<String> {
    let length = pattern.chars().count();
    if length > MAX_TOPIC_PATTERN_LENGTH {
        return Some(format!(
            "A topic pattern is at most {MAX_TOPIC_PATTERN_LENGTH} characters long, but this one \
             is {length}."
        ));
    }
    ::regex::Regex::new(&anchored_topic_pattern(pattern))
        .err()
        .map(|_| format!("'{pattern}' is not a valid regular expression."))
}

/// Why a replication flow's topic-selection pattern lists cannot be stored:
/// WHICH `TopicReplication` member is at fault and why.
///
/// Named so `CreateReplicator` and `UpdateReplicationInfo` admit the same values
/// and name the same member, each under its own request path — a create reports
/// the member inside the `ReplicationInfoList` entry it came in, an update
/// reports it under the update shape.
#[derive(Debug, Clone)]
pub struct TopicPatternsViolation {
    /// The member at fault, spelled as the model names it.
    pub member: &'static str,
    /// What is wrong with it.
    pub message: String,
}

/// Check both pattern lists of one replication flow: no more patterns than
/// [`MAX_TOPIC_PATTERNS_PER_FLOW`], none longer than
/// [`MAX_TOPIC_PATTERN_LENGTH`], every one of them a regular expression topics
/// can be selected by, and the list usable — within
/// [`TOPIC_MATCHER_SIZE_LIMIT`] — as the matcher a reader will build from it.
/// `None` when the lists are admissible.
///
/// Together those bound the work and the memory a stored flow can cost: the
/// patterns of one list are at most `MAX_TOPIC_PATTERNS_PER_FLOW` strings of
/// `MAX_TOPIC_PATTERN_LENGTH` characters, and the matcher compiled from them at
/// most `TOPIC_MATCHER_SIZE_LIMIT` bytes, whatever the request asked for.
pub fn topic_patterns_violation(
    topics_to_replicate: &[String],
    topics_to_exclude: &[String],
) -> Option<TopicPatternsViolation> {
    for (member, patterns) in [
        ("TopicsToReplicate", topics_to_replicate),
        ("TopicsToExclude", topics_to_exclude),
    ] {
        if patterns.len() > MAX_TOPIC_PATTERNS_PER_FLOW {
            return Some(TopicPatternsViolation {
                member,
                message: format!(
                    "{member} may hold at most {MAX_TOPIC_PATTERNS_PER_FLOW} patterns, not {}.",
                    patterns.len()
                ),
            });
        }
        if let Some(message) = patterns.iter().find_map(|pattern| topic_pattern_error(pattern)) {
            return Some(TopicPatternsViolation { member, message });
        }
        // Admitted through the SAME construction the readers match topics with, so
        // a list this service could not turn into a matcher — including one whose
        // compiled program would pass `TOPIC_MATCHER_SIZE_LIMIT` — is refused here
        // rather than stored and selecting nothing.
        if topic_matcher(patterns).is_none() {
            return Some(TopicPatternsViolation {
                member,
                message: format!(
                    "{member} holds patterns too large to be used together as a topic filter."
                ),
            });
        }
    }
    None
}

/// The matcher the patterns `patterns` describe, or `None` when they cannot be
/// compiled into one together.
///
/// The ONE construction a flow's patterns go through: the admission check builds
/// it to decide whether to accept them, and the store builds it to keep, so a
/// stored configuration can never hold patterns the readers would match nothing
/// with. Bounded by [`MAX_TOPIC_PATTERNS_PER_FLOW`] for the reason that constant
/// gives, and every pattern anchored because it describes a whole topic name.
fn topic_matcher(patterns: &[String]) -> Option<RegexSet> {
    ::regex::RegexSetBuilder::new(
        patterns
            .iter()
            .take(MAX_TOPIC_PATTERNS_PER_FLOW)
            .map(|pattern| anchored_topic_pattern(pattern)),
    )
    // The compiled program's own ceiling, set here rather than left to the regex
    // crate's default: the patterns come from a request, and a stored flow keeps
    // the matcher compiled from them, so the memory one create can leave behind
    // is bounded by this rather than by what the caller wrote.
    .size_limit(TOPIC_MATCHER_SIZE_LIMIT)
    .build()
    .ok()
}

/// The patterns `patterns` holds, compiled into the matcher a reader selects
/// topics with.
///
/// Called where a flow's configuration is STORED, never on a read path. An
/// unbuildable list cannot reach here — `CreateReplicator` and
/// `UpdateReplicationInfo` refuse one through [`topic_patterns_violation`], which
/// builds the same matcher — so the empty fallback is an unreachable backstop
/// rather than a filter anybody can be given.
fn compile_topic_patterns(patterns: &[String]) -> RegexSet {
    topic_matcher(patterns).unwrap_or_else(RegexSet::empty)
}

/// The name a replication flow gives the source topic `source_topic` on its
/// target cluster: the source cluster's alias and the topic's own name when the
/// flow prefixes replicated names, and the source topic's name itself when it
/// asks for identical ones.
pub fn replicated_topic_name(source_alias: &str, source_topic: &str, prefixed: bool) -> String {
    if prefixed {
        format!("{source_alias}{REPLICATED_TOPIC_SEPARATOR}{source_topic}")
    } else {
        source_topic.to_string()
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
    /// Which revision of its configuration the replicator is at.
    /// [`Replicator::current_version`] is this number's token, so the version
    /// changes exactly when an accepted update changes the replicator.
    pub revision: u64,
    /// `ReplicatorDescription` — the summary description the request carried, if
    /// any.
    pub description: Option<String>,
    /// The IAM role the replicator uses to reach the source and target clusters.
    pub service_execution_role_arn: String,
    pub creation_time: SystemTime,
    /// The instant the clock owes this replicator its transition into
    /// `RUNNING`: from `CREATING` after the create, and from `UPDATING` after an
    /// accepted configuration change. Stored, never slept on.
    pub running_due: SystemTime,
    /// A `ReplicatorState` value.
    pub state: String,
    /// Why provisioning this replicator failed, as the `ReplicationStateInfo` a
    /// reader reports, or `None` when every cluster it names resolved.
    ///
    /// Decided when the create is ACCEPTED and kept here, because MSK resolves a
    /// replicator's clusters while it provisions it rather than while it admits
    /// the request: the clock then owes the replicator `FAILED` instead of
    /// `RUNNING`, and this is the detail that says why.
    pub failure: Option<ReplicationStateInfo>,
    /// Whether this record is a reference to a replicator created in another
    /// region (`IsReplicatorReference`).
    pub is_replicator_reference: bool,
    /// The clusters replication runs between, in submission order.
    pub kafka_clusters: Vec<ReplicatorCluster>,
    /// The replications configured between those clusters, in submission order.
    pub replication_info: Vec<ReplicationInfo>,
    /// The compiled topic filter of each of those replications, at the same
    /// index: the one DERIVED thing on this record, built from
    /// [`Self::replication_info`] whenever that is set so no reader has to compile
    /// a caller's patterns. Set by [`Self::provisioned`] and refreshed by
    /// [`Self::apply_replication_update`], the only two places the configuration
    /// changes.
    pub topic_filters: Vec<TopicFilter>,
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
    /// whole identity), `now` the creation instant, `input` the submitted
    /// configuration as `CreateReplicator` deserialized it, and `failure` the
    /// provisioning failure the setup already carries (see [`Self::failure`]),
    /// which the clock turns into [`REPLICATOR_STATE_FAILED`].
    ///
    /// The configuration is stored with the service's own defaults APPLIED
    /// ([`applied_kafka_cluster`], [`applied_replication_info`]), so a member the
    /// request left out is reported from the record like any other instead of
    /// being absent from every reader.
    pub fn provisioned(
        region: &str,
        suffix: u64,
        now: SystemTime,
        input: CreateReplicatorInput,
        failure: Option<ReplicationStateInfo>,
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
                cluster: applied_kafka_cluster(cluster),
            })
            .collect();
        // Capped for the same reason as the cluster list above.
        let replication_info: Vec<ReplicationInfo> = input
            .replication_info_list
            .into_iter()
            .take(MAX_FLOWS_PER_REPLICATOR)
            .map(applied_replication_info)
            .collect();
        Self {
            current_version: current_version_token(&arn, 1),
            revision: 1,
            resource_arn: arn.clone(),
            arn,
            name: input.replicator_name,
            region: region.to_string(),
            description: input.description,
            service_execution_role_arn: input.service_execution_role_arn,
            creation_time: now,
            running_due: now + REPLICATOR_ACTIVATION_DELAY,
            state: REPLICATOR_STATE_CREATING.to_string(),
            failure,
            // A replicator created here is the replicator itself, never the
            // read-only reference AWS surfaces in the OTHER region of a
            // cross-region setup.
            is_replicator_reference: false,
            kafka_clusters,
            // Each flow's patterns are compiled HERE, once, so no later read of
            // this record compiles a pattern a request supplied.
            topic_filters: replication_info
                .iter()
                .map(|info| TopicFilter::of(&info.topic_replication))
                .collect(),
            replication_info,
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

    /// Apply the transition the clock owes this replicator once `now` has reached
    /// [`Self::running_due`] — out of `CREATING` after the create and out of
    /// `UPDATING` after an accepted change, the two non-terminal states a
    /// replicator of this registry is reported in. A replicator whose
    /// provisioning failed ([`Self::failure`]) is owed `FAILED` rather than
    /// `RUNNING`: the clusters it was created over did not resolve, so it never
    /// starts replicating. Idempotent, and a no-op for every other state: only a
    /// transition the clock already owed is materialised here.
    pub fn settle(&mut self, now: SystemTime) {
        let settling = self.state == REPLICATOR_STATE_CREATING
            || self.state == REPLICATOR_STATE_UPDATING;
        if settling && now >= self.running_due {
            self.state = match self.failure {
                Some(_) => REPLICATOR_STATE_FAILED.to_string(),
                None => REPLICATOR_STATE_RUNNING.to_string(),
            };
        }
    }

    /// The `StateInfo` a reader reports for this replicator: the provisioning
    /// failure, once the clock has moved the replicator into
    /// [`REPLICATOR_STATE_FAILED`]. A state that explains itself — `CREATING`,
    /// `RUNNING`, `UPDATING`, `DELETING` — carries none.
    pub fn state_info(&self) -> Option<ReplicationStateInfo> {
        if self.state == REPLICATOR_STATE_FAILED {
            return self.failure.clone();
        }
        None
    }

    /// The instant this replicator's flows start replicating: the instant the
    /// clock first owes the replicator `RUNNING`.
    ///
    /// Read off the creation instant rather than [`Self::running_due`], which a
    /// later accepted update moves forward: an update rolls out while the flows
    /// the replicator already runs keep running, so a configuration change does
    /// not withdraw what has already been replicated.
    pub fn replication_start(&self) -> SystemTime {
        self.creation_time + REPLICATOR_ACTIVATION_DELAY
    }

    /// Whether this replicator is replicating at `now`: from
    /// [`Self::replication_start`] on, which is exactly when a reader reports it
    /// `RUNNING`. A replicator whose provisioning failed never replicates
    /// anything, whatever the clock says. A deleted replicator leaves the
    /// registry, so no caller can ask this about one.
    pub fn replicates_at(&self, now: SystemTime) -> bool {
        self.failure.is_none() && now >= self.replication_start()
    }

    /// The Amazon MSK cluster ARNs this replicator replicates INTO, once each:
    /// what the per-target index maps to this replicator, so a reader of one
    /// cluster's topics finds the replicators that feed it.
    ///
    /// A flow whose target is a self-managed Apache Kafka cluster names no ARN
    /// here: that cluster's topics live on the cluster itself, which this
    /// emulator does not hold.
    pub fn target_cluster_arns(&self) -> BTreeSet<String> {
        self.replication_info
            .iter()
            .filter_map(|info| info.target_kafka_cluster_arn.clone())
            .collect()
    }

    /// The position in [`Self::replication_info`] of the replication running
    /// between the clusters the aliases `source` and `target` name, or `None`
    /// when this replicator configures no replication for that ordered pair.
    ///
    /// A replication is addressed by the ALIASES of its two ends — the same way
    /// every reader reports it — so an end given as an Amazon MSK cluster ARN and
    /// the same end given as an Apache Kafka cluster id resolve through one rule,
    /// [`flow_cluster_alias`]. The pair is ORDERED: the aliases of a configured
    /// replication named the other way round address no replication.
    pub fn replication_index_of(&self, source: &str, target: &str) -> Option<usize> {
        self.replication_info.iter().position(|info| {
            let configured_source = flow_cluster_alias(
                info.source_kafka_cluster_arn.as_deref(),
                info.source_kafka_cluster_id.as_deref(),
            );
            let configured_target = flow_cluster_alias(
                info.target_kafka_cluster_arn.as_deref(),
                info.target_kafka_cluster_id.as_deref(),
            );
            configured_source.is_some_and(|alias| alias == source)
                && configured_target.is_some_and(|alias| alias == target)
        })
    }

    /// Fold an accepted replication-info update into the stored record: replace
    /// the members the update shapes carry on the replication at `index`, take
    /// the submitted log delivery when the request carried one, bump the
    /// replicator's revision — and so its `CurrentVersion` token — and move it to
    /// `UPDATING` with the instant the clock owes it `RUNNING` again.
    ///
    /// `TopicReplicationUpdate` and `ConsumerGroupReplicationUpdate` model every
    /// member they carry as required, so each one SETS that member of the stored
    /// configuration; the members those shapes do not model (a topic
    /// replication's starting position and topic-name configuration, a consumer
    /// group replication's offset sync mode) are not part of the update and keep
    /// the values the record already holds.
    ///
    /// A no-op for an `index` the record does not hold, so a caller of this
    /// module cannot change a replicator's version without changing its
    /// configuration.
    pub fn apply_replication_update(
        &mut self,
        index: usize,
        now: SystemTime,
        topic_replication: Option<TopicReplicationUpdate>,
        consumer_group_replication: Option<ConsumerGroupReplicationUpdate>,
        log_delivery: Option<LogDelivery>,
    ) {
        let Some(info) = self.replication_info.get_mut(index) else {
            return;
        };
        if let Some(update) = topic_replication {
            let stored = &mut info.topic_replication;
            stored.copy_access_control_lists_for_topics =
                Some(update.copy_access_control_lists_for_topics);
            stored.copy_topic_configurations = Some(update.copy_topic_configurations);
            stored.detect_and_copy_new_topics = Some(update.detect_and_copy_new_topics);
            stored.topics_to_exclude = Some(update.topics_to_exclude);
            stored.topics_to_replicate = update.topics_to_replicate;
        }
        if let Some(update) = consumer_group_replication {
            let stored = &mut info.consumer_group_replication;
            stored.consumer_groups_to_exclude = Some(update.consumer_groups_to_exclude);
            stored.consumer_groups_to_replicate = update.consumer_groups_to_replicate;
            stored.detect_and_copy_new_consumer_groups =
                Some(update.detect_and_copy_new_consumer_groups);
            stored.synchronise_consumer_group_offsets =
                Some(update.synchronise_consumer_group_offsets);
        }
        if log_delivery.is_some() {
            self.log_delivery = log_delivery;
        }
        // The filter is DERIVED from the configuration just written, so it is
        // rebuilt here rather than left reporting the patterns the update
        // replaced — and rebuilt for the one replication that changed.
        if let Some(info) = self.replication_info.get(index) {
            let filter = TopicFilter::of(&info.topic_replication);
            match self.topic_filters.get_mut(index) {
                Some(stored) => *stored = filter,
                // Unreachable: the filter list is built from the replication list
                // and the two are only ever set together. Grown rather than
                // dropped, so the replication at `index` keeps a filter.
                None => {
                    self.topic_filters.resize_with(index, TopicFilter::default);
                    self.topic_filters.push(filter);
                }
            }
        }
        self.revision += 1;
        self.current_version = current_version_token(&self.arn, self.revision);
        self.state = REPLICATOR_STATE_UPDATING.to_string();
        self.running_due = now + REPLICATOR_ACTIVATION_DELAY;
    }

    /// The compiled topic filter of the replication at `index`, or `None` when
    /// the record holds no such replication.
    pub fn topic_filter(&self, index: usize) -> Option<&TopicFilter> {
        self.topic_filters.get(index)
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

/// Why [`ComAmazonawsKafkaKafkaState::register_cluster`],
/// [`ComAmazonawsKafkaKafkaState::register_configuration`] or
/// [`ComAmazonawsKafkaKafkaState::register_replicator`] refused to register a
/// resource. Every variant leaves the registry exactly as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterRejected {
    /// The region is at its per-region quota for this kind of resource
    /// ([`MAX_CLUSTERS_PER_REGION`], [`MAX_CONFIGURATIONS_PER_REGION`]).
    RegionQuota,
    /// The registry already holds its ceiling of resources of this kind
    /// ([`MAX_CLUSTERS_IN_REGISTRY`], [`MAX_CONFIGURATIONS_IN_REGISTRY`],
    /// [`MAX_REPLICATORS_IN_REGISTRY`]).
    RegistryCeiling,
    /// A resource with this ARN is already registered. The registry's monotonic
    /// ordinal makes this unreachable; it is refused rather than overwriting an
    /// existing resource.
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
    /// The instant this topic was created on its cluster. Not reported by any
    /// topic shape; it is what decides whether a replication flow that does not
    /// detect new topics covers this one, since such a flow replicates the topics
    /// its source cluster already held when the replicator was created.
    pub created_at: SystemTime,
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
            created_at: now,
            active_due: now + TOPIC_ACTIVATION_DELAY,
            removal_due: None,
        }
    }

    /// The topic a replication flow carries onto its TARGET cluster, as that
    /// cluster's readers report it: `target` is the cluster the flow replicates
    /// INTO, `name` the name the flow gives the topic there, `source` the source
    /// cluster's topic it is carried from, `configs` the topic-level
    /// configuration the flow copies over (if any), and `visible_at` the instant
    /// the clock owed its appearance on the target.
    ///
    /// Every member is DERIVED from that source topic and that target cluster: a
    /// replicated topic carries the source topic's partitions, and its replication
    /// factor is the source topic's bounded by the brokers the TARGET cluster has
    /// to place replicas on — the same bound `CreateTopic` enforces for a topic of
    /// that cluster, since Apache Kafka cannot place more replicas of a partition
    /// than there are brokers.
    pub fn replicated(
        target: &Cluster,
        name: String,
        source: &Topic,
        configs: Option<String>,
        visible_at: SystemTime,
    ) -> Self {
        Self {
            arn: target.topic_arn(&name),
            name,
            cluster_arn: target.arn.clone(),
            partition_count: source.partition_count,
            replication_factor: source
                .replication_factor
                .clamp(1, target.number_of_broker_nodes.max(1)),
            configs,
            // The flow has finished carrying the topic over by the instant the
            // target reports it at all, so the target reports the created topic it
            // is: before that instant the target holds no such topic.
            state: TOPIC_STATE_ACTIVE.to_string(),
            created_at: visible_at,
            active_due: visible_at,
            removal_due: None,
        }
    }

    /// Whether a reader at `now` still sees this topic in its cluster: a topic
    /// whose removal the clock already owes is gone, whether or not anything has
    /// pruned the record yet.
    pub fn is_present(&self, now: SystemTime) -> bool {
        self.removal_due.is_none_or(|due| now < due)
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
    /// Every stored configuration, keyed by ARN. `BTreeMap` for the same reason
    /// the clusters are: `ListConfigurations` needs one total order to page
    /// through.
    pub configurations: BTreeMap<String, ConfigurationRecord>,
    /// (region, configuration name) -> ARN. A configuration name is unique
    /// within an account and region, so the key carries the region too.
    pub configuration_arn_by_name: HashMap<(String, String), String>,
    /// region -> how many configurations it holds, maintained by
    /// [`Self::register_configuration`] so the per-region quota costs a lookup
    /// rather than a walk of the registry.
    pub configurations_per_region: HashMap<String, usize>,
    /// Monotonic counter feeding the configuration ARNs, so each configuration
    /// gets a distinct identity.
    pub next_configuration_suffix: u64,
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
    /// Target Amazon MSK cluster ARN -> the ARNs of the replicators that run a
    /// replication flow INTO that cluster. Maintained on insert and remove by
    /// [`Self::register_replicator`] / [`Self::unregister_replicator`], so
    /// deriving one cluster's replicated topics costs a lookup rather than a walk
    /// of the whole replicator registry. The endpoints of a stored flow never
    /// change — `UpdateReplicationInfo` changes a replication's topic and
    /// consumer-group settings, not which clusters it runs between — so the index
    /// cannot drift from the records it points at.
    pub replicator_arns_by_target_cluster: HashMap<String, BTreeSet<String>>,
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

    /// The ARN registered for the configuration `name` in `region`, if any. This
    /// is the lookup that makes a repeated `CreateConfiguration` a conflict.
    ///
    /// A configuration whose removal the clock already owes holds its name no
    /// longer, so the name is free again even before anything pruned the record;
    /// one that is `DELETING` with its removal still ahead DOES still hold it,
    /// because it is still a configuration of this region.
    pub fn configuration_arn_for_name(
        &self,
        region: &str,
        name: &str,
        now: SystemTime,
    ) -> Option<&String> {
        let arn = self
            .configuration_arn_by_name
            .get(&(region.to_string(), name.to_string()))?;
        self.configurations
            .get(arn)
            .filter(|configuration| configuration.is_live(now))
            .map(|_| arn)
    }

    /// How many configurations `region` currently holds — what the per-region
    /// quota is checked against. O(1): the count is maintained as they come and
    /// go.
    pub fn configuration_count_in(&self, region: &str) -> usize {
        self.configurations_per_region
            .get(region)
            .copied()
            .unwrap_or(0)
    }

    /// The configuration `arn` names, as an endpoint for `region` sees it at
    /// `now`.
    ///
    /// A regional endpoint serves only its own region's configurations, so a
    /// well-formed ARN belonging to another region resolves to `None` exactly as
    /// an absent one does — the same rule [`Self::cluster_in`] follows, so the
    /// readers and `ListConfigurations` agree on which configurations this
    /// endpoint holds. A configuration whose removal the clock already owes is
    /// gone for the same reason, whether or not the record has been pruned yet;
    /// a `DELETING` one whose removal is still ahead resolves, since the
    /// asynchronous delete has not taken it yet.
    pub fn configuration_in(
        &self,
        region: &str,
        arn: &str,
        now: SystemTime,
    ) -> Option<&ConfigurationRecord> {
        self.configurations
            .get(arn)
            .filter(|configuration| configuration.region == region && configuration.is_live(now))
    }

    /// The same lookup for a call that WRITES, with the removal the clock
    /// already owed THIS configuration materialised: a configuration whose
    /// removal is due leaves the registry here (so this answers `None` and its
    /// name is free again), and any other resolves mutably.
    ///
    /// Scoped to the one configuration being addressed, for the same reason
    /// [`Self::settled_cluster_in`] is: no other configuration's removal is this
    /// request's business.
    pub fn settled_configuration_in_mut(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
    ) -> Option<&mut ConfigurationRecord> {
        let configuration = self.configurations.get(arn)?;
        if configuration.region != region {
            return None;
        }
        if !configuration.is_live(now) {
            self.unregister_configuration(region, arn);
            return None;
        }
        self.configurations.get_mut(arn)
    }

    /// Accept the deletion of the configuration `arn` names, as an endpoint for
    /// `region` sees it, handing back the record as the caller must now see it:
    /// `DELETING`, with the instant the clock owes its removal set.
    ///
    /// Answers `None` when this region holds no such configuration — including
    /// one whose removal the clock already owed, which the settling lookup
    /// materialises first, so a configuration is never deleted twice. Idempotent
    /// through [`ConfigurationRecord::begin_deletion`].
    pub fn begin_configuration_deletion(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
    ) -> Option<&ConfigurationRecord> {
        let configuration = self.settled_configuration_in_mut(region, arn, now)?;
        configuration.begin_deletion(now);
        Some(configuration)
    }

    /// Drop the configurations of `region` whose removal the clock owed by
    /// `now`.
    ///
    /// Every one of them was due at that same instant whether or not anybody
    /// addressed it, so this materialises nothing a scoped lookup would not:
    /// it is what lets the registry's ceilings bound the records actually held,
    /// rather than letting a caller looping create-and-delete under fresh names
    /// grow it past them.
    fn prune_removed_configurations(&mut self, region: &str, now: SystemTime) {
        let removed: Vec<String> = self
            .configurations
            .values()
            .filter(|configuration| configuration.region == region && !configuration.is_live(now))
            .map(|configuration| configuration.arn.clone())
            .collect();
        for arn in removed {
            self.unregister_configuration(region, &arn);
        }
    }

    /// Add `configuration` to every index that must know about it: the one
    /// insertion path, so the ARN registry, the (region, name) index and the
    /// per-region count cannot drift apart, and the one place the registry's
    /// ceilings are enforced.
    ///
    /// Refuses, changing nothing, when the region is at
    /// [`MAX_CONFIGURATIONS_PER_REGION`], when the whole registry is at
    /// [`MAX_CONFIGURATIONS_IN_REGISTRY`], or when the ARN is already registered.
    pub fn register_configuration(
        &mut self,
        configuration: ConfigurationRecord,
        now: SystemTime,
    ) -> Result<(), RegisterRejected> {
        // Materialise every removal this region's configurations are already
        // owed before the ceilings are checked, so they bound the records the
        // registry really holds.
        self.prune_removed_configurations(&configuration.region, now);
        if self.configurations.contains_key(&configuration.arn) {
            return Err(RegisterRejected::DuplicateArn);
        }
        if self.configurations.len() >= MAX_CONFIGURATIONS_IN_REGISTRY {
            return Err(RegisterRejected::RegistryCeiling);
        }
        if self.configuration_count_in(&configuration.region) >= MAX_CONFIGURATIONS_PER_REGION {
            return Err(RegisterRejected::RegionQuota);
        }
        *self
            .configurations_per_region
            .entry(configuration.region.clone())
            .or_insert(0) += 1;
        self.configuration_arn_by_name.insert(
            (
                configuration.region.clone(),
                configuration.name.clone(),
            ),
            configuration.arn.clone(),
        );
        self.configurations
            .insert(configuration.arn.clone(), configuration);
        Ok(())
    }

    /// Remove the configuration `arn` names from every index, as an endpoint for
    /// `region` sees it, handing the record back. The one removal path, so the
    /// ARN registry, the (region, name) index and the per-region count cannot
    /// drift apart — the counterpart of [`Self::register_configuration`], and
    /// what the settling lookups call once the clock owes a removal.
    ///
    /// Follows the same regional rule as [`Self::configuration_in`]: a
    /// well-formed ARN belonging to another region removes nothing and resolves
    /// to `None`, exactly as an absent one does, so a delete and a read agree on
    /// which configurations this endpoint holds. The name it held is free again,
    /// which is what lets a later create reuse it.
    pub fn unregister_configuration(
        &mut self,
        region: &str,
        arn: &str,
    ) -> Option<ConfigurationRecord> {
        if self
            .configurations
            .get(arn)
            .is_none_or(|configuration| configuration.region != region)
        {
            return None;
        }
        let configuration = self.configurations.remove(arn)?;
        self.configuration_arn_by_name
            .remove(&(configuration.region.clone(), configuration.name.clone()));
        if let Some(count) = self.configurations_per_region.get_mut(&configuration.region) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.configurations_per_region.remove(&configuration.region);
            }
        }
        Some(configuration)
    }

    /// The ARNs of the clusters in `region` whose stored `ConfigurationInfo`
    /// names the configuration `arn`, in registry (ARN) order.
    ///
    /// A cluster keeps the configuration it was created with, so this is the
    /// association that decides whether a configuration is still being used:
    /// computed from the stored clusters rather than from a counter, so a cluster
    /// that has since left the registry stops holding its configuration down.
    pub fn clusters_using_configuration(&self, region: &str, arn: &str) -> Vec<String> {
        self.clusters
            .values()
            .filter(|cluster| cluster.region == region)
            .filter(|cluster| {
                cluster
                    .configuration_info
                    .as_ref()
                    .is_some_and(|info| info.arn == arn)
            })
            .map(|cluster| cluster.arn.clone())
            .collect()
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

    /// The stored tag set of the resource `arn` names in `region`, as a caller at
    /// `now` sees it.
    ///
    /// This is the ONE resolution the tagging operations share, so
    /// `TagResource`, `UntagResource` and `ListTagsForResource` cannot disagree
    /// about which ARNs name a taggable resource or about which failure a given
    /// ARN earns. Tags live on the resource record itself, which is why a tag
    /// written here is visible to every other reader of that resource (a
    /// cluster's tags are reported by `DescribeCluster` and `ListClusters`, a
    /// channel's by `DescribeChannel` and `ListChannels`) — and why `now` is
    /// needed: a channel whose removal the clock already owes is gone for the
    /// tagging operations exactly as it is for its own reader.
    pub fn resource_tags_in(
        &self,
        region: &str,
        arn: &str,
        now: SystemTime,
    ) -> Result<&BTreeMap<String, String>, TagTargetRejected> {
        match resource_type_of_arn(arn)? {
            CLUSTER_RESOURCE_TYPE => self
                .cluster_in(region, arn)
                .map(|cluster| &cluster.tags)
                .ok_or(TagTargetRejected::NoSuchResource),
            CHANNEL_RESOURCE_TYPE => self
                .channel_by_arn_in(region, arn, now)
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
        now: SystemTime,
    ) -> Result<&mut BTreeMap<String, String>, TagTargetRejected> {
        match resource_type_of_arn(arn)? {
            CLUSTER_RESOURCE_TYPE => self
                .cluster_mut_in(region, arn)
                .map(|cluster| &mut cluster.tags)
                .ok_or(TagTargetRejected::NoSuchResource),
            CHANNEL_RESOURCE_TYPE => self
                .channel_by_arn_mut_in(region, arn, now)
                .map(|channel| &mut channel.tags)
                .ok_or(TagTargetRejected::NoSuchResource),
            REPLICATOR_RESOURCE_TYPE => self
                .replicator_mut_in(region, arn)
                .map(|replicator| &mut replicator.tags)
                .ok_or(TagTargetRejected::NoSuchResource),
            _ => Err(TagTargetRejected::NoSuchResource),
        }
    }

    /// The channel `arn` names, as an endpoint for `region` sees it at `now`,
    /// resolved by its OWN ARN alone.
    ///
    /// That is the form an operation that takes any resource ARN addresses a
    /// channel in: the cluster is not a separate request member there, it is a
    /// segment of the channel ARN itself (`channel/<cluster>/<cluster-id>/...`),
    /// so a channel ARN already names the cluster that holds it.
    /// [`Self::channel_in`] stays the resolution for the paths that carry the
    /// cluster separately and must agree with it. A channel whose removal the
    /// clock already owes resolves to `None`, exactly as it does there.
    pub fn channel_by_arn_in(&self, region: &str, arn: &str, now: SystemTime) -> Option<&Channel> {
        self.channels
            .get(arn)
            .filter(|channel| channel.region == region && channel.is_live(now))
    }

    /// The same lookup, mutably — the write counterpart of
    /// [`Self::channel_by_arn_in`], with the same regional scoping.
    pub fn channel_by_arn_mut_in(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
    ) -> Option<&mut Channel> {
        self.channels
            .get_mut(arn)
            .filter(|channel| channel.region == region && channel.is_live(now))
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

    /// The operations `cluster` has recorded, latest first.
    ///
    /// The cluster's own index into the operation registry, resolved through it
    /// and ordered by [`ClusterOperation::history_key`]: ONE derivation of a
    /// cluster's operation history, so a listing of it reports exactly the
    /// records [`Self::cluster_operation_in`] resolves for the same ARNs, in
    /// one order. An index entry whose record the history bound has already
    /// evicted is skipped rather than reported as a gap.
    pub fn cluster_operation_history(&self, cluster: &Cluster) -> Vec<&ClusterOperation> {
        let mut history: Vec<&ClusterOperation> = cluster
            .operation_arns
            .iter()
            .filter_map(|arn| self.cluster_operations.get(arn))
            .collect();
        history.sort_by(|left, right| left.history_key().cmp(&right.history_key()));
        history
    }

    /// Start an update on the cluster `arn` names, as an endpoint for `region`
    /// sees it: record the cluster operation, move the cluster to `UPDATING`,
    /// and hand it the change the clock will apply at [`CLUSTER_UPDATE_DELAY`].
    /// Answers the new operation's ARN, or `None` when this region holds no
    /// cluster for `arn`.
    ///
    /// The cluster is resolved through [`Self::settled_cluster_in_mut`], so a
    /// writer is scoped to its own region exactly as every reader is, and it
    /// updates a record whose clock-owed transitions have landed.
    ///
    /// `plan` turns the stored cluster into what the operation reports and what
    /// it changes: the attributes it touches as they are now, the same
    /// attributes as the update leaves them, and the change itself.
    ///
    /// The ONE path that starts a cluster update, so an operation ARN an update
    /// call hands back always names a record this registry holds — whichever
    /// update began it — and the operation record, the cluster's pending change,
    /// its active operation ARN and its operation history cannot drift apart.
    /// The history is bounded where every update's record is filed, in
    /// [`Self::file_cluster_operation`].
    fn start_cluster_update<Plan>(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
        operation_type: &str,
        plan: Plan,
    ) -> Option<String>
    where
        Plan: FnOnce(&Cluster) -> (MutableClusterInfo, MutableClusterInfo, ClusterChange),
    {
        let cluster = self.settled_cluster_in_mut(region, arn, now)?;
        let cluster_arn = cluster.arn.clone();
        let operation_region = cluster.region.clone();
        let cluster_type = cluster.cluster_type.clone();
        let (source_cluster_info, target_cluster_info, change) = plan(cluster);
        // The cluster mints the operation's ARN and holds the pending change:
        // one in-flight update per cluster, whatever began it.
        let operation_arn = cluster.begin_update(now, change);
        let operation = ClusterOperation {
            arn: operation_arn.clone(),
            cluster_arn,
            region: operation_region,
            cluster_type,
            operation_type: operation_type.to_string(),
            creation_time: now,
            end_due: now + CLUSTER_UPDATE_DELAY,
            source_cluster_info,
            target_cluster_info,
        };
        self.file_cluster_operation(operation);
        Some(operation_arn)
    }

    /// Start a storage update: the volume size, provisioned throughput and
    /// storage mode `change` names, reported by an operation of
    /// `operation_type` — `UpdateStorage` and `UpdateBrokerStorage` both come
    /// through here, each naming its own type.
    pub fn start_storage_update(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
        operation_type: &str,
        change: StorageChange,
    ) -> Option<String> {
        self.start_cluster_update(region, arn, now, operation_type, |cluster| {
            let source_cluster_info = cluster.storage_cluster_info();
            let target_cluster_info = cluster.storage_cluster_info_after(&change);
            (
                source_cluster_info,
                target_cluster_info,
                ClusterChange::Storage(change),
            )
        })
    }

    /// Start a broker-type update: every broker of the cluster moves to
    /// `instance_type` once the clock owes the operation its completion.
    pub fn start_broker_type_update(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
        instance_type: String,
    ) -> Option<String> {
        self.start_cluster_update(
            region,
            arn,
            now,
            OPERATION_TYPE_UPDATE_BROKER_TYPE,
            |cluster| {
                let source_cluster_info = cluster.broker_type_cluster_info();
                let target_cluster_info = broker_type_cluster_info(&instance_type);
                (
                    source_cluster_info,
                    target_cluster_info,
                    ClusterChange::BrokerType { instance_type },
                )
            },
        )
    }

    /// Start a broker-count update: the cluster holds
    /// `target_number_of_broker_nodes` broker nodes once the clock owes the
    /// operation its completion.
    pub fn start_broker_count_update(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
        target_number_of_broker_nodes: i32,
    ) -> Option<String> {
        // Bounded where the request value ENTERS the registry, so neither the
        // operation record it files nor the pending change it stores can carry a
        // count the model's range does not admit.
        let target = admissible_broker_count(target_number_of_broker_nodes);
        self.start_cluster_update(
            region,
            arn,
            now,
            OPERATION_TYPE_UPDATE_BROKER_COUNT,
            |cluster| {
                let source_cluster_info = cluster.broker_count_cluster_info();
                let target_cluster_info = cluster.broker_count_cluster_info_after(target);
                (
                    source_cluster_info,
                    target_cluster_info,
                    ClusterChange::BrokerCount {
                        number_of_broker_nodes: target,
                    },
                )
            },
        )
    }

    /// Start an Apache Kafka version update: the brokers move to
    /// `kafka_version`, together with the custom configuration the request
    /// named for that version, once the clock owes the operation its
    /// completion.
    pub fn start_kafka_version_update(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
        kafka_version: String,
        configuration_info: Option<ConfigurationInfo>,
    ) -> Option<String> {
        self.start_cluster_update(
            region,
            arn,
            now,
            OPERATION_TYPE_UPDATE_CLUSTER_KAFKA_VERSION,
            |cluster| {
                let source_cluster_info = cluster.kafka_version_cluster_info();
                let target_cluster_info = kafka_version_cluster_info(
                    &kafka_version,
                    configuration_info
                        .clone()
                        .or_else(|| cluster.configuration_info.clone()),
                );
                (
                    source_cluster_info,
                    target_cluster_info,
                    ClusterChange::KafkaVersion {
                        kafka_version,
                        configuration_info,
                    },
                )
            },
        )
    }

    /// Start a configuration update: the brokers move to the MSK configuration
    /// revision `configuration_info` names once the clock owes the operation its
    /// completion.
    pub fn start_configuration_update(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
        configuration_info: ConfigurationInfo,
    ) -> Option<String> {
        self.start_cluster_update(
            region,
            arn,
            now,
            OPERATION_TYPE_UPDATE_CLUSTER_CONFIGURATION,
            |cluster| {
                let source_cluster_info = cluster.configuration_cluster_info();
                let target_cluster_info =
                    configuration_cluster_info(Some(configuration_info.clone()));
                (
                    source_cluster_info,
                    target_cluster_info,
                    ClusterChange::Configuration { configuration_info },
                )
            },
        )
    }

    /// Start a connectivity update: the broker access configuration, the
    /// ZooKeeper access setting, or both, as the request named them.
    pub fn start_connectivity_update(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
        connectivity_info: Option<ConnectivityInfo>,
        zookeeper_access: Option<ZookeeperAccess>,
    ) -> Option<String> {
        self.start_cluster_update(
            region,
            arn,
            now,
            OPERATION_TYPE_UPDATE_CONNECTIVITY,
            |cluster| {
                let source_cluster_info = cluster.connectivity_cluster_info();
                let target_cluster_info = cluster.connectivity_cluster_info_after(
                    connectivity_info.as_ref(),
                    zookeeper_access.as_ref(),
                );
                (
                    source_cluster_info,
                    target_cluster_info,
                    ClusterChange::Connectivity {
                        connectivity_info,
                        zookeeper_access,
                    },
                )
            },
        )
    }

    /// Start a security update: the client authentication the cluster accepts,
    /// the encryption it runs with, or both, as the request named them.
    pub fn start_security_update(
        &mut self,
        region: &str,
        arn: &str,
        now: SystemTime,
        client_authentication: Option<ClientAuthentication>,
        encryption_info: Option<EncryptionInfo>,
    ) -> Option<String> {
        self.start_cluster_update(region, arn, now, OPERATION_TYPE_UPDATE_SECURITY, |cluster| {
            let source_cluster_info = cluster.security_cluster_info();
            let target_cluster_info = cluster.security_cluster_info_after(
                client_authentication.as_ref(),
                encryption_info.as_ref(),
            );
            (
                source_cluster_info,
                target_cluster_info,
                ClusterChange::Security {
                    client_authentication,
                    encryption_info,
                },
            )
        })
    }

    /// File `operation` in the registry and in its own cluster's history, so the
    /// ARN a reader resolves and the history a cluster lists cannot drift apart.
    ///
    /// The history is bounded here: the records past
    /// [`MAX_OPERATIONS_PER_CLUSTER`] are that cluster's own oldest, so the trim
    /// costs one walk of THAT cluster's history rather than of the registry. The
    /// one filing path, so every update operation bounds its history the same way.
    fn file_cluster_operation(&mut self, operation: ClusterOperation) {
        let operation_arn = operation.arn.clone();
        let evicted = match self.clusters.get_mut(&operation.cluster_arn) {
            Some(cluster) => {
                cluster.operation_arns.push(operation_arn.clone());
                let excess = cluster
                    .operation_arns
                    .len()
                    .saturating_sub(MAX_OPERATIONS_PER_CLUSTER);
                cluster.operation_arns.drain(..excess).collect::<Vec<_>>()
            }
            // Unreachable: an operation is filed for the cluster it was just
            // started on, under the same lock. Filed anyway rather than dropped,
            // so the ARN the caller was handed still resolves.
            None => Vec::new(),
        };
        self.cluster_operations
            .insert(operation_arn, operation);
        for stale in evicted {
            self.cluster_operations.remove(&stale);
        }
    }

    /// How many topics the cluster `cluster_arn` names holds as a reader at
    /// `now` sees it — what the per-cluster ceiling is checked against. A topic
    /// whose removal the clock already owes is NOT counted, so the ceiling
    /// agrees with what every reader reports rather than reserving room for
    /// records that are gone. The registry key leads with the cluster ARN, so
    /// one cluster's topics are a contiguous range.
    pub fn topic_count_in(&self, cluster_arn: &str, now: SystemTime) -> usize {
        self.topics_in(cluster_arn)
            .filter(|topic| topic.is_present(now))
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
            .filter(|topic| !topic.is_present(now))
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

    /// The provisioning failure a replicator over `clusters` carries, or `None`
    /// when every Amazon MSK cluster its setup names resolves.
    ///
    /// Amazon MSK ADMITS a `CreateReplicator` request on what the request itself
    /// says and resolves the clusters while it provisions the replicator, so a
    /// setup naming a cluster that does not exist is not refused: the replicator
    /// is registered and the clock converges it to [`REPLICATOR_STATE_FAILED`]
    /// carrying this detail. The FIRST entry that does not resolve is the one
    /// reported, in submission order, so one setup yields one reason.
    ///
    /// Resolved against the whole registry rather than one region's clusters: a
    /// replicator replicates BETWEEN regions, so a cluster of another region is a
    /// cluster that exists. A `KafkaClusters` entry naming a self-managed Apache
    /// Kafka cluster resolves to nothing here — that cluster is not a resource
    /// this service holds.
    pub fn replicator_provisioning_failure(
        &self,
        clusters: &[KafkaCluster],
    ) -> Option<ReplicationStateInfo> {
        let unresolved = clusters
            .iter()
            .filter_map(|cluster| cluster.amazon_msk_cluster.as_ref())
            .map(|msk| msk.msk_cluster_arn.as_str())
            .find(|arn| !self.clusters.contains_key(*arn))?;
        Some(ReplicationStateInfo {
            code: Some(REPLICATION_CODE_INVALID_KAFKA_CLUSTER.to_string()),
            message: Some(invalid_kafka_cluster_message(unresolved)),
        })
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
        // One entry per Amazon MSK cluster this replicator replicates INTO, which
        // is what a reader of that cluster's topics looks the replicator up by. A
        // flow whose target is a self-managed Apache Kafka cluster is indexed
        // under nothing: that cluster's topics are not this service's to report.
        for target in replicator.target_cluster_arns() {
            self.replicator_arns_by_target_cluster
                .entry(target)
                .or_default()
                .insert(replicator.arn.clone());
        }
        self.replicators.insert(replicator.arn.clone(), replicator);
        Ok(())
    }

    /// Remove the replicator `arn` names from every index, handing it back. The
    /// one removal path, for the same reason.
    pub fn unregister_replicator(&mut self, arn: &str) -> Option<Replicator> {
        let replicator = self.replicators.remove(arn)?;
        self.replicator_arn_by_name
            .remove(&(replicator.region.clone(), replicator.name.clone()));
        for target in replicator.target_cluster_arns() {
            if let Some(arns) = self.replicator_arns_by_target_cluster.get_mut(&target) {
                arns.remove(arn);
                if arns.is_empty() {
                    self.replicator_arns_by_target_cluster.remove(&target);
                }
            }
        }
        Some(replicator)
    }

    /// Every topic the cluster `cluster_arn` names reports to a reader at `now`:
    /// the topics it holds itself, plus the topics a running replicator
    /// replicates onto it. Keyed, and so ordered, by the name the cluster reports
    /// each topic under — the order `ListTopics` pages through.
    ///
    /// The ONE derivation the topic readers answer from, so a listing and a
    /// describe of the same cluster cannot disagree about which topics are on it.
    /// A name the cluster holds a topic of its own under is that topic: a
    /// replicated name never shadows a stored record.
    pub fn observable_topics_in(
        &self,
        cluster_arn: &str,
        now: SystemTime,
    ) -> BTreeMap<String, Topic> {
        let mut topics = self.replicated_topics_into(cluster_arn, None, now);
        for topic in self
            .topics_in(cluster_arn)
            .filter(|topic| topic.is_present(now))
        {
            topics.insert(topic.name.clone(), topic.clone());
        }
        topics
    }

    /// The same derivation for the ONE name a reader addresses: the cluster's own
    /// topic of that name, with the transitions the clock already owed it
    /// materialised, or else the topic a running replicator replicates onto this
    /// cluster under it.
    pub fn observable_topic_in(
        &mut self,
        cluster_arn: &str,
        name: &str,
        now: SystemTime,
    ) -> Option<Topic> {
        if let Some(topic) = self.settled_topic_in(cluster_arn, name, now) {
            return Some(topic.clone());
        }
        self.replicated_topics_into(cluster_arn, Some(name), now)
            .remove(name)
    }

    /// The topics the registry's replicators replicate onto the cluster
    /// `target_cluster_arn` names, as a reader at `now` sees them, keyed by the
    /// name the target reports each one under; `wanted` narrows the derivation to
    /// one such name.
    ///
    /// A replicated topic is DERIVED, never a second stored record: it is
    /// reported exactly while a replicator that is replicating has a flow into
    /// this cluster whose filter covers a topic its source cluster still holds,
    /// so deleting that topic — or the replicator — withdraws it with nothing
    /// having to remember to. A flow's own source topics are reported from the
    /// moment the clock owes the propagation ([`TOPIC_REPLICATION_DELAY`] after
    /// the flow and the topic are both in place), and a flow that does not detect
    /// new topics carries only the ones its source cluster already held when the
    /// replicator was created.
    ///
    /// The work is bounded for every reader: the per-target index narrows it to
    /// the flows INTO this cluster, [`MAX_REPLICATION_FLOWS_PER_TARGET`] bounds how
    /// many of those one reader derives from, [`MAX_TOPICS_PER_CLUSTER`] bounds
    /// what it reports — the same ceiling the cluster's own topics obey — and the
    /// patterns were compiled when they were stored, so nothing here compiles a
    /// caller's pattern.
    fn replicated_topics_into(
        &self,
        target_cluster_arn: &str,
        wanted: Option<&str>,
        now: SystemTime,
    ) -> BTreeMap<String, Topic> {
        let mut replicated = BTreeMap::new();
        // A cluster this endpoint does not hold reports nothing at all, and a
        // replicated topic's identity is derived from the target cluster's own.
        let Some(target) = self.clusters.get(target_cluster_arn) else {
            return replicated;
        };
        let mut flows_derived = 0usize;
        for replicator in self
            .replicator_arns_by_target_cluster
            .get(target_cluster_arn)
            .into_iter()
            .flatten()
            .filter_map(|arn| self.replicators.get(arn))
            .filter(|replicator| replicator.replicates_at(now))
        {
            for (index, info) in replicator.replication_info.iter().enumerate().filter(|(_, info)| {
                info.target_kafka_cluster_arn.as_deref() == Some(target_cluster_arn)
            }) {
                // The two bounds this derivation is capped by, checked before each
                // flow is walked: the flows one reader derives from, and the topics
                // one cluster reports.
                if flows_derived >= MAX_REPLICATION_FLOWS_PER_TARGET
                    || replicated.len() >= MAX_TOPICS_PER_CLUSTER
                {
                    return replicated;
                }
                flows_derived += 1;
                // Only an Amazon MSK source cluster's topics are this service's to
                // read: a self-managed Apache Kafka cluster's topics live on that
                // cluster, which this emulator does not hold.
                let Some(source_arn) = info.source_kafka_cluster_arn.as_deref() else {
                    continue;
                };
                // Compiled when the flow was stored, so a listing matches topics
                // rather than compiling what a request supplied.
                let Some(filter) = replicator.topic_filter(index) else {
                    continue;
                };
                let settings = &info.topic_replication;
                // Replicated names are prefixed with the source cluster's alias
                // unless the flow asks for names identical to the source's, which
                // is the one thing `ReplicationTopicNameConfiguration` configures.
                let prefixed = settings
                    .topic_name_configuration
                    .as_ref()
                    .and_then(|configuration| configuration.r#type.as_deref())
                    .is_none_or(|kind| kind != TOPIC_NAME_IDENTICAL);
                let alias = replicator
                    .alias_of(Some(source_arn), None)
                    .unwrap_or_default();
                // A flow that does not say periodically detects new topics and
                // copies the source topics' configurations: those are the
                // replicator's own defaults, so only an explicit `false` turns
                // either off.
                let detects_new_topics = settings.detect_and_copy_new_topics != Some(false);
                let copies_configurations = settings.copy_topic_configurations != Some(false);
                let flow_start = replicator.replication_start();
                for source in self
                    .topics_in(source_arn)
                    .filter(|topic| topic.is_present(now))
                    .filter(|topic| filter.covers(&topic.name))
                    .filter(|topic| {
                        detects_new_topics || topic.created_at <= replicator.creation_time
                    })
                {
                    if replicated.len() >= MAX_TOPICS_PER_CLUSTER {
                        return replicated;
                    }
                    let name = replicated_topic_name(&alias, &source.name, prefixed);
                    // An Apache Kafka topic name is at most
                    // `MAX_TOPIC_NAME_LENGTH` characters, so a prefixed name past
                    // that length is one no cluster could hold.
                    if name.chars().count() > MAX_TOPIC_NAME_LENGTH {
                        continue;
                    }
                    if wanted.is_some_and(|wanted| wanted != name) {
                        continue;
                    }
                    let visible_at = source.active_due.max(flow_start) + TOPIC_REPLICATION_DELAY;
                    if now < visible_at {
                        continue;
                    }
                    let configs = if copies_configurations {
                        source.configs.clone()
                    } else {
                        None
                    };
                    // The first flow to replicate a name keeps it: two flows that
                    // carry one name onto a cluster are carrying the same topic
                    // there, and the cluster holds one topic per name.
                    replicated.entry(name.clone()).or_insert_with(|| {
                        Topic::replicated(target, name, source, configs, visible_at)
                    });
                }
            }
        }
        replicated
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

    /// The ARN registered for channel `name` under the cluster `cluster_arn` as a
    /// caller at `now` sees it — the lookup that makes a repeated `CreateChannel`
    /// a conflict.
    ///
    /// A channel whose removal the clock already owes has freed its name, so this
    /// answers `None` for it: the create that follows must see the same registry
    /// the insertion path does.
    pub fn channel_arn_for_name(
        &self,
        cluster_arn: &str,
        name: &str,
        now: SystemTime,
    ) -> Option<&String> {
        let arn = self
            .channel_arn_by_name
            .get(&(cluster_arn.to_string(), name.to_string()))?;
        self.channels
            .get(arn)
            .filter(|channel| channel.is_live(now))
            .map(|_| arn)
    }

    /// How many channels `cluster_arn` currently holds — what the per-cluster
    /// ceiling is checked against. O(1): the index is maintained as channels come
    /// and go.
    pub fn channel_count_in_cluster(&self, cluster_arn: &str) -> usize {
        self.channel_arns_by_cluster
            .get(cluster_arn)
            .map_or(0, BTreeSet::len)
    }

    /// Drop the channels of `cluster_arn` whose removal the clock owed by `now`.
    /// Scoped to the one cluster through the per-cluster index, so this never
    /// walks the whole registry.
    fn prune_removed_channels(&mut self, cluster_arn: &str, now: SystemTime) {
        let removed: Vec<String> = self
            .channel_arns_by_cluster
            .get(cluster_arn)
            .into_iter()
            .flatten()
            .filter(|arn| {
                self.channels
                    .get(*arn)
                    .is_some_and(|channel| !channel.is_live(now))
            })
            .cloned()
            .collect();
        for arn in removed {
            self.unregister_channel(&arn);
        }
    }

    /// Add `channel` to every index that must know about it. The one insertion
    /// path, so the ARN registry, the (cluster, name) index and the per-cluster
    /// index cannot drift apart — and the one place the per-cluster ceiling is
    /// enforced, so it holds for every caller of this module.
    ///
    /// `now` is the instant the create is served: the removals this cluster's
    /// channels are already owed are materialised FIRST, so a name whose channel
    /// is gone as far as every reader is concerned is free here too (whether or
    /// not anybody read it in between) and the ceiling bounds what the registry
    /// actually holds.
    ///
    /// Refuses, changing nothing, when the cluster is at
    /// [`MAX_CHANNELS_PER_CLUSTER`] or when the ARN is already registered.
    pub fn register_channel(
        &mut self,
        channel: Channel,
        now: SystemTime,
    ) -> Result<(), ChannelRejected> {
        self.prune_removed_channels(&channel.cluster_arn, now);
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
    /// the same way [`Self::cluster_in`] refuses another region's cluster. A
    /// channel whose removal the clock already owes is absent too.
    pub fn channel_in(
        &self,
        region: &str,
        cluster_arn: &str,
        channel_arn: &str,
        now: SystemTime,
    ) -> Option<&Channel> {
        self.channels.get(channel_arn).filter(|channel| {
            channel.region == region && channel.cluster_arn == cluster_arn && channel.is_live(now)
        })
    }

    /// The same lookup, with the transitions the clock already owed THIS channel
    /// materialised: a channel whose removal is due is gone (so this answers
    /// `None`), and one whose activation is due is `ACTIVE`.
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
        let channel = self.channels.get(channel_arn)?;
        if channel.region != region || channel.cluster_arn != cluster_arn {
            return None;
        }
        if !channel.is_live(now) {
            self.unregister_channel(channel_arn);
            return None;
        }
        let channel = self.channels.get_mut(channel_arn)?;
        channel.settle(now);
        Some(channel)
    }

    /// Accept the deletion of the channel `channel_arn` names under
    /// `cluster_arn`, as an endpoint for `region` sees it, handing back the record
    /// as the caller must now see it: `DELETING`, with the instant the clock owes
    /// its removal and the delete's own cluster operation set.
    ///
    /// Answers `None` when this region holds no such channel under that cluster —
    /// including one whose removal the clock already owed, which the settling
    /// lookup materialises first, so a channel is never deleted twice. Idempotent
    /// through [`Channel::begin_deletion`].
    pub fn begin_channel_deletion(
        &mut self,
        region: &str,
        cluster_arn: &str,
        channel_arn: &str,
        now: SystemTime,
    ) -> Option<&Channel> {
        self.settled_channel_in(region, cluster_arn, channel_arn, now)?;
        let channel = self.channels.get_mut(channel_arn)?;
        channel.begin_deletion(now);
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
            owner: account.to_string(),
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

    /// The connection as the `VpcConnection` shape `ListVpcConnections`
    /// reports: the subset of this record that listing carries. Every member is
    /// read off the stored record, which is what keeps a listed connection and
    /// a described one from ever disagreeing.
    pub fn to_vpc_connection(&self) -> crate::ops::types::VpcConnection {
        crate::ops::types::VpcConnection {
            vpc_connection_arn: self.arn.clone(),
            target_cluster_arn: self.target_cluster_arn.clone(),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            authentication: Some(self.authentication.clone()),
            vpc_id: Some(self.vpc_id.clone()),
            state: Some(self.state.clone()),
        }
    }

    /// The connection as the `ClientVpcConnection` shape
    /// `ListClientVpcConnections` reports: the view a CLUSTER has of one client
    /// connection to it, which names the account that owns the connection
    /// instead of the cluster it targets — the caller named that cluster to get
    /// this listing. Every member is read off the same stored record
    /// [`Self::to_vpc_connection`] renders, so the two listings and
    /// `DescribeVpcConnection` cannot disagree about a connection.
    pub fn to_client_vpc_connection(&self) -> ClientVpcConnection {
        ClientVpcConnection {
            vpc_connection_arn: self.arn.clone(),
            authentication: Some(self.authentication.clone()),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            state: Some(self.state.clone()),
            owner: Some(self.owner.clone()),
        }
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

/// The sentence Amazon MSK answers a tag key that starts with the reserved
/// prefix with: fixed text end to end — the offending key is NOT named — and
/// stating that the test is applied without regard to case, which is how the
/// service applies it ([`starts_with_reserved_tag_prefix`]).
pub const RESERVED_TAG_KEY_MESSAGE: &str =
    "Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)";

/// Whether `key` starts with the reserved prefix, compared WITHOUT regard to
/// case: `AWS:`, `Aws:` and `aws:` are one prefix, which is what the refusal
/// says ([`RESERVED_TAG_KEY_MESSAGE`]) and what keeps a caller from writing a
/// key the service reserves by spelling it differently.
pub fn starts_with_reserved_tag_prefix(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// Why `key` is not a tag key a caller may set or remove, phrased as the message
/// a `BadRequestException` carries — or `None` when the key is acceptable.
///
/// The length bound and the reserved prefix are stated by the model's own
/// `UntagResourceRequest$TagKeys` documentation: 1..128 Unicode characters and no
/// `aws:` prefix. The character set is AWS's own tagging rule (AWS General
/// Reference, "Tagging AWS resources"): Unicode letters, digits, white space and
/// the characters `+ - = . _ : / @` — the colon included, which is what makes a
/// namespaced key (`<namespace>:<name>`) a legal key everywhere tags are written.
///
/// The reserved-prefix clause is the service's own fixed sentence
/// ([`RESERVED_TAG_KEY_MESSAGE`]), which names neither the key nor the prefix;
/// the other two state the constraint and the offending key, for which no
/// wording of the real service has been observed.
pub fn tag_key_violation(key: &str) -> Option<String> {
    let length = key.chars().count();
    if !(1..=MAX_TAG_KEY_LENGTH).contains(&length) {
        return Some(format!(
            "Tag key '{key}' is {length} Unicode characters long; a tag key must be between 1 and {MAX_TAG_KEY_LENGTH} Unicode characters in length."
        ));
    }
    if starts_with_reserved_tag_prefix(key) {
        return Some(RESERVED_TAG_KEY_MESSAGE.to_string());
    }
    if let Some(offending) = key.chars().find(|c| !is_tag_key_character(*c)) {
        return Some(format!(
            "Tag key '{key}' contains the unsupported character '{offending}'; a tag key must consist of Unicode letters, digits, white space and the special characters + - = . _ : / @."
        ));
    }
    None
}

/// Whether `c` is one of the characters AWS's tagging rule admits in a tag key:
/// a Unicode letter, a digit, white space, or one of `+ - = . _ : / @`.
fn is_tag_key_character(c: char) -> bool {
    c.is_alphabetic()
        || c.is_numeric()
        || c.is_whitespace()
        || matches!(c, '+' | '-' | '=' | '.' | '_' | ':' | '/' | '@')
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

/// Encode the ARN a page stopped at as a token, through the one page-token
/// codec of this service ([`encode_page_token`]) under the cluster listings' own
/// marker. The token is ENCODED, not authenticated, so a caller could decode it
/// or mint one. That is harmless here — the ARN inside is only used as a `>`
/// comparison against the keys of the caller's own registry, so a forged resume
/// point can only skip or repeat entries the caller may already list.
pub fn encode_cluster_page_token(last_arn: &str) -> String {
    encode_page_token(CLUSTER_PAGE_TOKEN_PREFIX, last_arn)
}

/// The ARN a token resumes after, or `None` when the token is not one a cluster
/// listing minted — which every listing refuses as a bad request rather than
/// serving as a first page.
pub fn decode_cluster_page_token(token: &str) -> Option<String> {
    decode_page_token(CLUSTER_PAGE_TOKEN_PREFIX, token)
}

impl ConfigurationRecord {
    /// Mint a configuration from the submitted request: derive its identity from
    /// `suffix` (the registry's monotonic counter) and store the submitted body
    /// as revision 1.
    ///
    /// `region` is the caller's signing region, `now` the creation instant. The
    /// ARN takes the form the real service mints,
    /// `arn:<partition>:kafka:<region>:<account>:configuration/<name>/<id>-<ordinal>`.
    pub fn created(
        region: &str,
        suffix: u64,
        now: SystemTime,
        input: CreateConfigurationInput,
    ) -> Self {
        let name = input.name;
        // The registry's counter, as the 1-based ordinal AWS puts at the end of
        // a configuration ARN; the SAME value seeds the opaque id before it.
        let ordinal = suffix + 1;
        let digest = stable_digest(&format!(
            "configuration:{}:{region}:{name}:{ordinal}",
            ::engine::emulator_account_id()
        ));
        let arn = ::engine::emulator_arn(
            "kafka",
            region,
            &format!(
                "configuration/{name}/{}-{ordinal}",
                uuid_v4_shaped_from_digest(&digest)
            ),
        );
        // `Description` is optional on the request and on the readers that
        // report it, and an empty description is no description, so the one
        // normalisation decides what is stored.
        let description = stored_description(input.description);
        Self {
            arn,
            name,
            region: region.to_string(),
            kafka_versions: input.kafka_versions.unwrap_or_default(),
            creation_time: now,
            state: CONFIGURATION_STATE_ACTIVE.to_string(),
            removal_due: None,
            revisions: vec![ConfigurationRevisionRecord {
                revision: 1,
                creation_time: now,
                description: description.clone(),
                server_properties: input.server_properties,
            }],
            description,
        }
    }

    /// Whether this configuration is still part of the registry a caller at
    /// `now` sees: one whose removal the clock already owes is gone, whether or
    /// not the record has been pruned yet.
    pub fn is_live(&self, now: SystemTime) -> bool {
        self.removal_due.is_none_or(|due| now < due)
    }

    /// Accept this configuration's deletion at `now`: move it to `DELETING` and
    /// record the instant the clock owes its removal from the registry.
    ///
    /// Idempotent — a configuration already `DELETING` keeps the removal instant
    /// it was given, so repeating the call cannot postpone the removal.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        if self.removal_due.is_some() {
            return;
        }
        self.state = CONFIGURATION_STATE_DELETING.to_string();
        self.removal_due = Some(now + CONFIGURATION_DELETION_DELAY);
    }

    /// The newest stored revision. Infallible on a registered configuration:
    /// [`Self::created`] stores revision 1 and an update only ever appends.
    pub fn latest_revision(&self) -> &ConfigurationRevisionRecord {
        self.revisions
            .last()
            .expect("a configuration always holds at least revision 1")
    }

    /// Store `server_properties` as this configuration's next revision and hand
    /// the stored revision back.
    ///
    /// A configuration's revision history is append-only: the submitted body
    /// becomes the revision one past the current latest and no stored revision
    /// is ever rewritten, which is what lets a reader of an earlier revision see
    /// the body that revision was created with. `description` is that revision's
    /// own description, normalised through [`stored_description`] — `None` when
    /// the request named none or named an empty one, since
    /// `ConfigurationRevision$Description` is optional and reports a revision
    /// with no description by omitting the member. The configuration's own
    /// description is the one it was created with.
    ///
    /// This is the one append path, so it is also the one place the history's
    /// ceilings are enforced: it refuses, storing nothing, a body past
    /// [`MAX_SERVER_PROPERTIES_BYTES`] and an append past
    /// [`MAX_REVISIONS_PER_CONFIGURATION`].
    pub fn append_revision(
        &mut self,
        now: SystemTime,
        description: Option<String>,
        server_properties: Vec<u8>,
    ) -> Result<&ConfigurationRevisionRecord, RevisionRejected> {
        if server_properties_too_large(&server_properties) {
            return Err(RevisionRejected::BodyTooLarge);
        }
        if self.revisions.len() >= MAX_REVISIONS_PER_CONFIGURATION {
            return Err(RevisionRejected::HistoryFull);
        }
        let revision = self.latest_revision().revision + 1;
        self.revisions.push(ConfigurationRevisionRecord {
            revision,
            creation_time: now,
            description: stored_description(description),
            server_properties,
        });
        Ok(self.latest_revision())
    }

    /// The stored revision numbered `revision`, or `None` when this
    /// configuration holds no such revision — the distinction between a revision
    /// number a reader reports and one it must refuse as absent.
    pub fn revision(&self, revision: i64) -> Option<&ConfigurationRevisionRecord> {
        self.revisions
            .iter()
            .find(|stored| stored.revision == revision)
    }

    /// The configuration as the `Configuration` shape `ListConfigurations` and
    /// `DescribeConfiguration` report, so every reader of this record renders it
    /// the same way.
    pub fn to_configuration(&self) -> Configuration {
        let latest = self.latest_revision();
        Configuration {
            arn: self.arn.clone(),
            creation_time: ::aws_smithy_types::DateTime::from(self.creation_time),
            // `Configuration$Description` is `@required` on this shape, so the
            // member is always written and a configuration that has no
            // description can only be rendered with the empty string here. The
            // readers whose own member is OPTIONAL
            // (`DescribeConfigurationResponse$Description`) omit it instead —
            // which is also why the empty string a request may submit is stored
            // as no description at all.
            description: self.description.clone().unwrap_or_default(),
            kafka_versions: self.kafka_versions.clone(),
            latest_revision: latest.to_configuration_revision(),
            name: self.name.clone(),
            state: self.state.clone(),
        }
    }
}

impl ConfigurationRevisionRecord {
    /// This revision as the modelled `ConfigurationRevision` shape, so the
    /// writer and every reader of a stored revision render it the same way.
    pub fn to_configuration_revision(&self) -> ConfigurationRevision {
        ConfigurationRevision {
            creation_time: ::aws_smithy_types::DateTime::from(self.creation_time),
            // Optional on the shape, so a revision that has no description is
            // reported by leaving the member out.
            description: self.description.clone(),
            revision: self.revision,
        }
    }
}

/// How many characters of a cluster's NAME the alias MSK derives for it carries.
///
/// The alias is the name truncated to this many characters, a hyphen and
/// [`KAFKA_CLUSTER_ALIAS_ID_CHARS`] characters of the cluster's id, so it stays
/// the same length whatever the name and tells two clusters whose names share a
/// prefix apart.
const KAFKA_CLUSTER_ALIAS_NAME_CHARS: usize = 28;

/// How many hexadecimal digits of a cluster's own id (the uuid segment of its
/// ARN) the alias carries, after the truncated name: the uuid's last group.
const KAFKA_CLUSTER_ALIAS_ID_CHARS: usize = 12;

/// The alias of one `KafkaCluster` entry: an Amazon MSK cluster is aliased by the
/// alias MSK DERIVES from its ARN ([`msk_cluster_alias`]), and an Apache Kafka
/// cluster by the id the caller gave it. The alias is what prefixes the names of
/// replicated topics, per `KafkaClusterDescription$KafkaClusterAlias`.
pub fn kafka_cluster_alias(cluster: &KafkaCluster) -> Option<String> {
    if let Some(msk) = cluster.amazon_msk_cluster.as_ref() {
        return flow_cluster_alias(Some(msk.msk_cluster_arn.as_str()), None);
    }
    cluster
        .apache_kafka_cluster
        .as_ref()
        .map(|apache| apache.apache_kafka_cluster_id.clone())
}

/// WHICH Kafka cluster one `KafkaClusters` entry names, as the request names it:
/// the Amazon MSK cluster's ARN, or the Apache Kafka cluster's id — the value
/// the entry identifies its cluster by, and `None` for an entry that identifies
/// none.
///
/// This is what makes the list a set: two entries are the same cluster when
/// they name the same one ([`DUPLICATE_KAFKA_CLUSTERS_MESSAGE`]). Read from the
/// entry itself rather than from the alias derived for it, so the test is over
/// the identifier the caller sent.
pub fn kafka_cluster_identity(cluster: &KafkaCluster) -> Option<&str> {
    if let Some(msk) = cluster.amazon_msk_cluster.as_ref() {
        return Some(msk.msk_cluster_arn.as_str());
    }
    cluster
        .apache_kafka_cluster
        .as_ref()
        .map(|apache| apache.apache_kafka_cluster_id.as_str())
}

/// Whether the `KafkaClusters` list `clusters` names any one Kafka cluster more
/// than once ([`kafka_cluster_identity`]) — the condition
/// [`DUPLICATE_KAFKA_CLUSTERS_MESSAGE`] is answered for.
///
/// An entry that identifies no cluster is not compared: that request is refused
/// on the entry itself, which is a different condition.
pub fn names_a_kafka_cluster_twice(clusters: &[KafkaCluster]) -> bool {
    let named: Vec<&str> = clusters.iter().filter_map(kafka_cluster_identity).collect();
    let distinct: std::collections::BTreeSet<&str> = named.iter().copied().collect();
    distinct.len() != named.len()
}

/// The alias of the cluster a replication flow names at one end, from the ARN or
/// the cluster id the flow carries. Same rule as [`kafka_cluster_alias`]: the
/// derived alias for an MSK ARN, the id itself for an Apache Kafka cluster.
pub fn flow_cluster_alias(arn: Option<&str>, id: Option<&str>) -> Option<String> {
    if let Some(arn) = arn.filter(|value| !value.is_empty()) {
        return Some(msk_cluster_alias(arn).unwrap_or_else(|| arn.to_string()));
    }
    id.filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

/// One END of a replication, as a request names it: the pair of members that can
/// carry the Kafka cluster at that end, spelled as the wire spells them, and the
/// word the service's refusals call that end by.
///
/// Both members of each pair are optional on every shape that takes a replication
/// (`ReplicationInfo`, and the `UpdateReplicationInfo` request itself) because
/// EITHER one identifies the cluster — not because an end may be left unnamed or
/// named twice. That rule is not something the model can state, so the service
/// states it at runtime, and it states it from here so every operation that takes
/// a replication's ends enforces it in the same words.
#[derive(Debug, Clone, Copy)]
pub struct ReplicationEnd {
    /// Which end this is, as the refusal names it.
    end: &'static str,
    /// The member carrying the end's Amazon MSK cluster ARN.
    arn_member: &'static str,
    /// The member carrying the end's Apache Kafka cluster id.
    id_member: &'static str,
}

/// The end a replication runs FROM.
pub const SOURCE_REPLICATION_END: ReplicationEnd = ReplicationEnd {
    end: "source",
    arn_member: "sourceKafkaClusterArn",
    id_member: "sourceKafkaClusterId",
};

/// The end a replication runs TO.
pub const TARGET_REPLICATION_END: ReplicationEnd = ReplicationEnd {
    end: "target",
    arn_member: "targetKafkaClusterArn",
    id_member: "targetKafkaClusterId",
};

impl ReplicationEnd {
    /// What the service refuses a request that does not name this end exactly
    /// once with: one fixed sentence per end, built from the two member names the
    /// wire carries and the word for the end, with NO part of it taken from the
    /// request.
    pub fn unnamed_message(&self) -> String {
        format!(
            "Must specify either {} or {} for the {} cluster.",
            self.arn_member, self.id_member, self.end
        )
    }
}

/// The alias of the cluster at `end`, from the pair of members a request can name
/// it by — or the refusal for that end when the pair does not name it exactly
/// once.
///
/// One of the two members, never both and never neither: a cluster given by both
/// its ARN and its cluster id is named twice, and one given by neither is not
/// named at all, so neither request describes an end this service can resolve.
/// A member sent EMPTY names nothing, the same reading [`flow_cluster_alias`]
/// gives it, so it counts as absent rather than as an end named by a value
/// nothing resolves. The alias itself comes from that one rule, so an end named
/// either way resolves to the alias every reader of a replicator reports it
/// under.
pub fn replication_end_alias(
    end: ReplicationEnd,
    arn: Option<&str>,
    id: Option<&str>,
) -> Result<String, String> {
    let arn = arn.filter(|value| !value.is_empty());
    let id = id.filter(|value| !value.is_empty());
    match (arn, id) {
        (Some(_), None) | (None, Some(_)) => {
            flow_cluster_alias(arn, id).ok_or_else(|| end.unnamed_message())
        }
        _ => Err(end.unnamed_message()),
    }
}

/// The alias Amazon MSK knows the cluster `arn` names a replicator by, or `None`
/// when `arn` is not an MSK cluster ARN.
///
/// DERIVED from the ARN, not read off it: MSK takes the cluster NAME, truncated
/// to [`KAFKA_CLUSTER_ALIAS_NAME_CHARS`] characters, a hyphen, and the cluster's
/// own id as [`KAFKA_CLUSTER_ALIAS_ID_CHARS`] hexadecimal digits
/// ([`cluster_alias_id`]). So the alias identifies the cluster rather than just
/// naming it: two clusters that share a name prefix, or the same name in two
/// accounts, get different aliases, and an alias is one fixed shape however long
/// the name is. One place, so the create and the describe projections of a
/// replicator cannot disagree about it.
pub fn msk_cluster_alias(arn: &str) -> Option<String> {
    let mut segments = kafka_arn_resource(arn)?.split('/');
    if segments.next()? != CLUSTER_RESOURCE_TYPE {
        return None;
    }
    let name = segments.next().filter(|name| !name.is_empty())?;
    let name: String = name.chars().take(KAFKA_CLUSTER_ALIAS_NAME_CHARS).collect();
    let id = cluster_alias_id(segments.next().unwrap_or_default());
    Some(format!("{name}-{id}"))
}

/// The [`KAFKA_CLUSTER_ALIAS_ID_CHARS`] hexadecimal digits of a cluster's id that
/// its alias carries: the FINAL GROUP of the uuid the ARN's id segment names, and
/// zeroes when that segment names no uuid — the alias keeps one shape either way,
/// and an ARN that carries no cluster id contributes none of its own.
fn cluster_alias_id(id: &str) -> String {
    /// The widths of a uuid's hyphen-separated groups, the last of which is the
    /// one an alias carries.
    const UUID_GROUP_WIDTHS: [usize; 5] = [8, 4, 4, 4, KAFKA_CLUSTER_ALIAS_ID_CHARS];

    // `splitn` with one field more than a uuid has leaves anything after the
    // uuid — the ordinal this emulator's own cluster ARNs carry — in the last
    // field, where it is not part of the uuid being read.
    let groups: Vec<&str> = id.splitn(UUID_GROUP_WIDTHS.len() + 1, '-').collect();
    let shaped = groups.len() >= UUID_GROUP_WIDTHS.len()
        && groups
            .iter()
            .zip(UUID_GROUP_WIDTHS)
            .all(|(group, width)| {
                group.len() == width && group.bytes().all(|byte| byte.is_ascii_hexdigit())
            });
    match shaped {
        true => groups[UUID_GROUP_WIDTHS.len() - 1].to_string(),
        false => "0".repeat(KAFKA_CLUSTER_ALIAS_ID_CHARS),
    }
}

/// The `ReplicationStartingPositionType` a replication starts from when the
/// request names no `StartingPosition`: the newest records, so an accepted
/// replication does not begin by copying every record the source cluster still
/// retains.
pub const DEFAULT_REPLICATION_STARTING_POSITION: &str = "LATEST";

/// The `ReplicationTopicNameConfigurationType` a replication uses when the
/// request names no `TopicNameConfiguration`: replicated topic names are prefixed
/// with the source cluster's alias, which is what
/// `KafkaClusterDescription$KafkaClusterAlias` documents the alias is for ("Used
/// to prefix names of replicated topics").
pub const DEFAULT_REPLICATION_TOPIC_NAME_CONFIGURATION: &str =
    "PREFIXED_WITH_SOURCE_CLUSTER_ALIAS";

/// The `ConsumerGroupOffsetSyncMode` a replication uses when the request names
/// none: the mode that needs nothing else in place, since the model documents
/// `ENHANCED` as requiring a corresponding replicator replicating the other way
/// (`smithy_query(query="doc",shape="ConsumerGroupOffsetSyncMode")`).
pub const DEFAULT_CONSUMER_GROUP_OFFSET_SYNC_MODE: &str = "LEGACY";

/// One submitted `KafkaCluster` as the service APPLIES it: the client VPC
/// configuration reports the security groups attached to the replicator's network
/// interfaces, which is an EMPTY list — not an absent member — when the request
/// attached none.
///
/// Applied where the configuration is stored, so every reader of the record
/// reports the same thing and no projection has to re-derive it.
pub fn applied_kafka_cluster(mut cluster: KafkaCluster) -> KafkaCluster {
    if let Some(vpc_config) = cluster.vpc_config.as_mut() {
        vpc_config.security_group_ids.get_or_insert_with(Vec::new);
    }
    cluster
}

/// One submitted `ReplicationInfo` as the service APPLIES it: every member of the
/// topic and consumer-group replication that the request left out is filled in
/// with the value the service runs the replication with — a selection the request
/// did not narrow excludes nothing, the two configuration shapes carry the
/// service's default type, and a feature switch the request did not set is ON,
/// because a replication MSK sets up keeps the target's topics, their
/// configurations, their ACLs and their consumer groups matching the source
/// unless it is told otherwise.
///
/// So a reader reports the whole configuration the replication RUNS under rather
/// than only the subset the request happened to spell out.
pub fn applied_replication_info(mut info: ReplicationInfo) -> ReplicationInfo {
    let topics = &mut info.topic_replication;
    topics.copy_access_control_lists_for_topics.get_or_insert(true);
    topics.copy_topic_configurations.get_or_insert(true);
    topics.detect_and_copy_new_topics.get_or_insert(true);
    topics.topics_to_exclude.get_or_insert_with(Vec::new);
    topics
        .starting_position
        .get_or_insert_with(|| ReplicationStartingPosition {
            r#type: Some(DEFAULT_REPLICATION_STARTING_POSITION.to_string()),
        })
        .r#type
        .get_or_insert_with(|| DEFAULT_REPLICATION_STARTING_POSITION.to_string());
    topics
        .topic_name_configuration
        .get_or_insert_with(|| ReplicationTopicNameConfiguration {
            r#type: Some(DEFAULT_REPLICATION_TOPIC_NAME_CONFIGURATION.to_string()),
        })
        .r#type
        .get_or_insert_with(|| DEFAULT_REPLICATION_TOPIC_NAME_CONFIGURATION.to_string());
    let groups = &mut info.consumer_group_replication;
    groups.detect_and_copy_new_consumer_groups.get_or_insert(true);
    groups.synchronise_consumer_group_offsets.get_or_insert(true);
    groups.consumer_groups_to_exclude.get_or_insert_with(Vec::new);
    groups
        .consumer_group_offset_sync_mode
        .get_or_insert_with(|| DEFAULT_CONSUMER_GROUP_OFFSET_SYNC_MODE.to_string());
    info
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
            status: CHANNEL_STATUS_CREATING.to_string(),
            removal_due: None,
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
            deletion_operation_arn: None,
        }
    }

    /// Apply the `CREATING` -> `ACTIVE` transition once `now` has reached
    /// [`Self::active_due`]. Idempotent, and a no-op for every other status: only
    /// a transition the clock already owed is materialised here. A channel that is
    /// `DELETING` never goes back to `ACTIVE`.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == CHANNEL_STATUS_CREATING && now >= self.active_due {
            self.status = CHANNEL_STATUS_ACTIVE.to_string();
        }
    }

    /// Whether this channel is still part of the registry a caller at `now` sees:
    /// a channel whose removal the clock already owes is gone, whether or not the
    /// record has been dropped yet.
    pub fn is_live(&self, now: SystemTime) -> bool {
        self.removal_due.is_none_or(|due| now < due)
    }

    /// Accept this channel's deletion at `now`: move it to `DELETING`, record the
    /// instant the clock owes its removal, and mint the cluster operation carrying
    /// the delete out.
    ///
    /// Idempotent — a channel already `DELETING` keeps the removal instant and the
    /// operation ARN it was given, so repeating the call cannot postpone the
    /// removal or re-identify the operation.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        if self.removal_due.is_some() {
            return;
        }
        // The delete's operation id is derived from the channel's own identity, so
        // it is distinct from the create's (a different digest input) and stable
        // across repeats of this call. It lives under the same cluster as the
        // create operation, so the ARN keeps that prefix and replaces only the
        // operation id.
        let operation_id = uuid_v4_shaped_from_digest(&stable_digest(&format!(
            "{}:{}:channel-delete-operation",
            ::engine::emulator_account_id(),
            self.arn
        )));
        let prefix = self
            .cluster_operation_arn
            .rsplit_once('/')
            .map_or(self.cluster_operation_arn.as_str(), |(head, _)| head);
        self.deletion_operation_arn = Some(format!("{prefix}/{operation_id}"));
        self.status = CHANNEL_STATUS_DELETING.to_string();
        self.removal_due = Some(now + CHANNEL_DELETION_DELAY);
    }

    /// The cluster operation a reader reports as this channel's in-flight one
    /// (`DescribeChannelResponse$ClusterOperationArn`, "the ARN of the in-flight
    /// cluster operation"): the delete once one has been accepted, otherwise the
    /// create that minted the channel.
    pub fn in_flight_cluster_operation_arn(&self) -> &str {
        self.deletion_operation_arn
            .as_deref()
            .unwrap_or(&self.cluster_operation_arn)
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
            cluster_operation_arn: Some(self.in_flight_cluster_operation_arn().to_string()),
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
        let ClusterIdentity { arn, dns_suffix } = cluster_identity(region, name, suffix);

        // BOUNDED, not trusted: the model's own 1..15 range already refuses an
        // out-of-range request at the boundary, but this constructor must not let
        // any caller size a heap allocation from an unchecked value.
        let broker_count = admissible_broker_count(input.number_of_broker_nodes);
        let mut brokers = Vec::with_capacity(usize::try_from(broker_count).unwrap_or(0));
        for id in 1..=broker_count {
            brokers.push(BrokerNode::placed(
                region,
                name,
                &dns_suffix,
                id,
                &input.broker_node_group_info.client_subnets,
                now,
            ));
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
            dns_suffix,
            brokers,
            operation_arns: Vec::new(),
            // A provisioned cluster comes up with ZooKeeper access on; only
            // `UpdateConnectivity` turns it off.
            zookeeper_access_enabled: true,
            // Revision 1 is the configuration the create submitted, and
            // `current_version` above is that revision's token.
            revision: 1,
            pending_update: None,
            // A provisioned cluster carries no serverless configuration: the
            // per-kind detail is stored for the kind the record IS.
            serverless: None,
            // A cluster is created with no resource-based policy; only
            // `PutClusterPolicy` attaches one.
            policy: None,
            // A cluster is created holding no SASL/SCRAM secrets; only
            // `BatchAssociateScramSecret` associates one.
            scram_secret_arns: BTreeSet::new(),
        }
    }

    /// Mint a serverless cluster: derive the cluster's identity from `suffix` the
    /// same way [`Self::provisioned`] does, and store the submitted VPC and
    /// client-authentication configuration as the [`ServerlessConfig`] every
    /// reader derives the `Serverless` shape from.
    ///
    /// `region` is the caller's signing region, `suffix` the registry's monotonic
    /// counter, `now` the creation instant, `name` the submitted cluster name,
    /// `tags` the submitted tags and `request` the `Serverless` body of the
    /// create.
    ///
    /// MSK Serverless provisions, scales and patches the brokers itself, so this
    /// record holds NO broker-node configuration: the broker list is empty, the
    /// broker count is zero, there is no ZooKeeper ensemble (nothing to connect
    /// to) and no Apache Kafka version — this API models none for a serverless
    /// cluster. Nothing reports those members for a serverless record: the
    /// broker-node readers refuse this kind ([`PROVISIONED_ONLY_MESSAGE`]) and
    /// the V2 readers report [`Self::serverless`] instead of the provisioned
    /// detail.
    pub fn serverless(
        region: &str,
        suffix: u64,
        now: SystemTime,
        name: String,
        tags: Option<HashMap<String, String>>,
        request: ServerlessRequest,
    ) -> Self {
        let ClusterIdentity { arn, dns_suffix } = cluster_identity(region, &name, suffix);
        let config = ServerlessConfig {
            vpc_configs: request.vpc_configs,
            client_authentication: request.client_authentication,
            bootstrap_host: format!("boot-{dns_suffix}.kafka-serverless.{region}.amazonaws.com"),
        };

        Self {
            current_version: current_version_token(&arn, 1),
            arn,
            name,
            region: region.to_string(),
            cluster_type: CLUSTER_TYPE_SERVERLESS.to_string(),
            // Not a member of this cluster kind: MSK Serverless runs the Apache
            // Kafka version, and neither the create nor the `Serverless` shape
            // names one.
            kafka_version: String::new(),
            creation_time: now,
            active_due: now + CLUSTER_ACTIVATION_DELAY,
            state: CLUSTER_STATE_CREATING.to_string(),
            // No customer broker nodes, hence no broker-node configuration and no
            // broker or ZooKeeper endpoints.
            number_of_broker_nodes: 0,
            broker_node_group_info: BrokerNodeGroupInfo::default(),
            client_authentication: None,
            // MSK Serverless encrypts data in transit between clients and the
            // cluster, so the stored record says TLS rather than the
            // provisioned-cluster default; the data volumes are encrypted with
            // the service's own key for this region, as they are for a
            // provisioned cluster that names none.
            encryption_info: resolve_encryption_info(
                region,
                Some(EncryptionInfo {
                    encryption_at_rest: None,
                    encryption_in_transit: Some(EncryptionInTransit {
                        client_broker: Some("TLS".to_string()),
                        in_cluster: Some(true),
                    }),
                }),
            ),
            enhanced_monitoring: DEFAULT_ENHANCED_MONITORING.to_string(),
            open_monitoring: OpenMonitoringInfo::default(),
            logging_info: None,
            storage_mode: DEFAULT_STORAGE_MODE.to_string(),
            configuration_info: None,
            rebalancing: None,
            tags: tags.unwrap_or_default().into_iter().collect(),
            zookeeper_connect_string: String::new(),
            zookeeper_connect_string_tls: String::new(),
            dns_suffix,
            brokers: Vec::new(),
            operation_arns: Vec::new(),
            // There is no ZooKeeper ensemble to reach.
            zookeeper_access_enabled: false,
            revision: 1,
            pending_update: None,
            serverless: Some(config),
            // Created with no resource-based policy, exactly as a provisioned
            // cluster is.
            policy: None,
            // And holding no SASL/SCRAM secrets, exactly as a provisioned
            // cluster is.
            scram_secret_arns: BTreeSet::new(),
        }
    }

    /// Whether this record is a serverless cluster — what an operation whose
    /// subject is the broker nodes tests before it reports or reconfigures them.
    pub fn is_serverless(&self) -> bool {
        self.cluster_type == CLUSTER_TYPE_SERVERLESS
    }

    /// Attach `document` as this cluster's resource-based policy, replacing any
    /// policy it already carries, and hand back the stored record.
    ///
    /// Each attachment is the next revision of the cluster's policy, so the
    /// `CurrentVersion` the writer answers with and every later reader reports
    /// changes exactly when the stored document does.
    pub fn put_policy(&mut self, document: String) -> &ClusterPolicy {
        let revision = self
            .policy
            .as_ref()
            .map_or(1, |policy| policy.revision.saturating_add(1));
        // The token is minted over the policy's own address rather than the
        // cluster's, so a policy version is never a token the cluster itself
        // reports as its `CurrentVersion`.
        let current_version = current_version_token(&format!("{}/policy", self.arn), revision);
        self.policy.insert(ClusterPolicy {
            document,
            revision,
            current_version,
        })
    }

    /// Associate `secret_arns` with this cluster, so its SASL/SCRAM clients can
    /// authenticate with those secrets and `ListScramSecrets` reports them.
    ///
    /// The set is what makes this idempotent: a secret the cluster already
    /// holds is not a second entry, so associating one twice leaves the stored
    /// set — and therefore every listing of it — unchanged.
    ///
    /// ALL OR NOTHING, and the one association path, so the ceiling
    /// [`MAX_SCRAM_SECRETS_PER_CLUSTER`] states holds for every caller rather
    /// than only for a handler that remembers to check first: a batch that
    /// would take the cluster past it is refused with the counts that explain
    /// the refusal, and stores none of its secrets.
    pub fn associate_scram_secrets(
        &mut self,
        secret_arns: &[String],
    ) -> Result<(), ScramSecretsRejected> {
        let held = self.scram_secret_arns.len();
        // What the batch ADDS is the distinct ARNs the cluster does not hold
        // yet, since it may repeat one and may name one already stored. Staged
        // in one pass and then inserted, so the count the bound is read against
        // is exactly what goes in.
        let mut staged: BTreeSet<&String> = BTreeSet::new();
        for arn in secret_arns {
            if !self.scram_secret_arns.contains(arn) {
                staged.insert(arn);
            }
        }
        let added = staged.len();
        let resulting = held.saturating_add(added);
        if resulting > MAX_SCRAM_SECRETS_PER_CLUSTER {
            return Err(ScramSecretsRejected {
                held,
                added,
                resulting,
            });
        }
        for arn in staged {
            self.scram_secret_arns.insert(arn.clone());
        }
        Ok(())
    }

    /// Disassociate `secret_arns` from this cluster, so its SASL/SCRAM clients
    /// can no longer authenticate with those secrets.
    ///
    /// The counterpart of [`Self::associate_scram_secrets`] on the same stored
    /// set, so a secret the cluster does not hold is removed from nothing and
    /// the call is idempotent in the same way the association is.
    pub fn disassociate_scram_secrets(&mut self, secret_arns: &[String]) {
        for arn in secret_arns {
            self.scram_secret_arns.remove(arn);
        }
    }

    /// Take this cluster's resource-based policy off the record, handing back
    /// what was removed — `None` when the cluster carried none, which is what
    /// makes a delete of an absent policy a not-found rather than a silent
    /// success.
    ///
    /// The revision history goes with it: a policy attached after this one is
    /// the first revision of a new policy, as it is on a cluster that never had
    /// one.
    pub fn take_policy(&mut self) -> Option<ClusterPolicy> {
        self.policy.take()
    }

    /// The one bootstrap string a serverless cluster has, or `None` for a
    /// provisioned one (whose strings come from [`Self::broker_endpoints`], one
    /// per enabled access path).
    pub fn serverless_bootstrap_brokers(&self) -> Option<String> {
        self.serverless.as_ref().map(|config| {
            join_endpoints(
                std::iter::once(config.bootstrap_host.as_str()),
                PORT_SASL_IAM,
            )
        })
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
                // The operation finished when the clock owed it, not when this
                // caller happened to look, so that instant is what a change
                // which records a time of its own (a broker node joining the
                // cluster) carries.
                let applied_at = update.due;
                self.apply_change(update.change, applied_at);
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

    /// The cluster's broker type as the `MutableClusterInfo` a cluster operation
    /// reports: the one attribute an `UpdateBrokerType` call changes, since an
    /// operation describes what it touches rather than the whole cluster.
    pub fn broker_type_cluster_info(&self) -> MutableClusterInfo {
        broker_type_cluster_info(&self.broker_node_group_info.instance_type)
    }

    /// The cluster's broker count as the `MutableClusterInfo` a cluster
    /// operation reports: the one attribute an `UpdateBrokerCount` call
    /// changes, since an operation describes what it touches rather than the
    /// whole cluster — the same rule [`Self::storage_cluster_info`] follows.
    pub fn broker_count_cluster_info(&self) -> MutableClusterInfo {
        MutableClusterInfo {
            number_of_broker_nodes: Some(self.number_of_broker_nodes),
            ..MutableClusterInfo::default()
        }
    }

    /// The same attribute as the update leaves it, plus the ids of the broker
    /// nodes the operation has to create or delete to get there, which is what
    /// `MutableClusterInfo$BrokerCountUpdateInfo` reports. Both lists are
    /// DERIVED from the stored nodes and the target through
    /// [`Self::broker_count_change`], the same derivation
    /// [`Self::apply_broker_count_change`] then applies, so the ids the
    /// operation reports are the nodes the cluster really gains or loses — and
    /// the count reported is the one that will land, since both go through
    /// [`admissible_broker_count`].
    pub fn broker_count_cluster_info_after(&self, target: i32) -> MutableClusterInfo {
        let target = admissible_broker_count(target);
        let (created_broker_ids, deleted_broker_ids) = self.broker_count_change(target);
        MutableClusterInfo {
            number_of_broker_nodes: Some(target),
            broker_count_update_info: Some(BrokerCountUpdateInfo {
                created_broker_ids: Some(created_broker_ids),
                deleted_broker_ids: Some(deleted_broker_ids),
            }),
            ..MutableClusterInfo::default()
        }
    }

    /// Which broker ids a move to `target` creates and which it deletes:
    /// growing the cluster adds the ids past the ones it holds, shrinking it
    /// takes the highest-numbered ones away, and a target the cluster is
    /// already at does neither. Both `BrokerCountUpdateInfo` members target
    /// `__listOf__double`, so a 1-based broker id travels as the number it is.
    ///
    /// `target` is bounded by [`admissible_broker_count`] before it reaches
    /// here, so each list holds at most [`MAX_BROKER_NODES_PER_CLUSTER`]
    /// entries and neither bound can overflow.
    fn broker_count_change(&self, target: i32) -> (Vec<f64>, Vec<f64>) {
        let target = admissible_broker_count(target);
        let current = admissible_broker_count(self.number_of_broker_nodes);
        (
            (current + 1..=target).map(f64::from).collect(),
            (target + 1..=current).map(f64::from).collect(),
        )
    }

    /// Resize the broker list to `target` nodes, each node added carrying
    /// `joined_at` as the instant it joined the cluster.
    ///
    /// The nodes themselves are the record, so every reader derived from them —
    /// the bootstrap strings, the node listing and the reported broker count —
    /// follows from this one write. A node added later is laid out by the SAME
    /// [`BrokerNode::placed`] the create lays its brokers out with, so it
    /// continues the cluster's own rotation through its client subnets and sits
    /// under the cluster's own DNS suffix; shrinking the cluster removes the
    /// highest-numbered nodes, the ones
    /// [`Self::broker_count_change`] reports as deleted.
    fn apply_broker_count_change(&mut self, target: i32, joined_at: SystemTime) {
        // BOUNDED for the same reason the create bounds its own count: the
        // target sizes an allocation, so no path through this module can size
        // it from a value the model's range did not admit.
        let target = admissible_broker_count(target);
        let highest = self.brokers.iter().map(|broker| broker.id).max().unwrap_or(0);
        let mut added = Vec::new();
        for id in highest + 1..=target {
            added.push(BrokerNode::placed(
                &self.region,
                &self.name,
                &self.dns_suffix,
                id,
                &self.broker_node_group_info.client_subnets,
                joined_at,
            ));
        }
        self.brokers.extend(added);
        self.brokers.retain(|broker| broker.id <= target);
        self.number_of_broker_nodes = target;
    }

    /// The Apache Kafka version the brokers run and the configuration revision
    /// they run it with, as the `MutableClusterInfo` a cluster operation
    /// reports: the attributes an `UpdateClusterKafkaVersion` call changes, and
    /// only those — the same rule [`Self::storage_cluster_info`] follows.
    pub fn kafka_version_cluster_info(&self) -> MutableClusterInfo {
        kafka_version_cluster_info(&self.kafka_version, self.configuration_info.clone())
    }

    /// The configuration revision the brokers run, as the `MutableClusterInfo`
    /// a cluster operation reports: the one attribute an
    /// `UpdateClusterConfiguration` call changes, and only that — the same rule
    /// [`Self::storage_cluster_info`] follows. `None` where the cluster runs no
    /// custom configuration at all, which is what a cluster created without one
    /// does.
    pub fn configuration_cluster_info(&self) -> MutableClusterInfo {
        configuration_cluster_info(self.configuration_info.clone())
    }

    /// The cluster's broker access configuration and ZooKeeper access setting
    /// as the `MutableClusterInfo` a cluster operation reports: the attributes
    /// an `UpdateConnectivity` call changes, and only those — the same rule
    /// [`Self::storage_cluster_info`] follows.
    pub fn connectivity_cluster_info(&self) -> MutableClusterInfo {
        connectivity_cluster_info(
            self.broker_node_group_info.connectivity_info.clone(),
            self.zookeeper_access_enabled,
        )
    }

    /// The same attributes as a submitted connectivity update leaves them.
    ///
    /// A submitted `ConnectivityInfo` REPLACES the cluster's broker access
    /// configuration while an omitted one leaves it alone, and
    /// `ZookeeperAccess$Enabled` is optional, so a request naming the shape
    /// without it leaves access on — the resolution [`Self::apply_change`]
    /// performs when the operation comes due, so what the operation reports as
    /// its target is what the cluster then runs.
    pub fn connectivity_cluster_info_after(
        &self,
        connectivity_info: Option<&ConnectivityInfo>,
        zookeeper_access: Option<&ZookeeperAccess>,
    ) -> MutableClusterInfo {
        connectivity_cluster_info(
            connectivity_info
                .cloned()
                .or_else(|| self.broker_node_group_info.connectivity_info.clone()),
            zookeeper_access.map_or(self.zookeeper_access_enabled, |access| {
                access.enabled.unwrap_or(true)
            }),
        )
    }

    /// The cluster's security settings as the `MutableClusterInfo` a cluster
    /// operation reports: the client authentication it accepts and the
    /// encryption it runs with, which are the attributes a security update
    /// changes and the only ones set — the same rule
    /// [`Self::storage_cluster_info`] follows.
    pub fn security_cluster_info(&self) -> MutableClusterInfo {
        security_cluster_info(
            self.client_authentication.clone(),
            self.encryption_info.clone(),
        )
    }

    /// The same attributes as a submitted security update leaves them, so what
    /// the operation reports as its target is what the cluster then runs.
    pub fn security_cluster_info_after(
        &self,
        client_authentication: Option<&ClientAuthentication>,
        encryption_info: Option<&EncryptionInfo>,
    ) -> MutableClusterInfo {
        security_cluster_info(
            self.client_authentication_after(client_authentication),
            self.encryption_info_after(encryption_info),
        )
    }

    /// The client authentication the cluster accepts once a submitted one has
    /// landed.
    ///
    /// `ClientAuthentication` "includes ALL client authentication information"
    /// (its model documentation), so a submitted shape REPLACES the cluster's
    /// own — the mechanisms it does not turn on are off — while an omitted one
    /// leaves the stored authentication exactly as it is. The same rule
    /// [`Self::connectivity_cluster_info_after`] applies to a submitted
    /// `ConnectivityInfo`.
    fn client_authentication_after(
        &self,
        submitted: Option<&ClientAuthentication>,
    ) -> Option<ClientAuthentication> {
        submitted
            .cloned()
            .or_else(|| self.client_authentication.clone())
    }

    /// The encryption settings the cluster runs with once a submitted
    /// `EncryptionInfo` has landed.
    ///
    /// The shape carries two independently configured settings, so each one a
    /// request names replaces the stored one and each one it omits is kept —
    /// a request that names only the in-transit setting leaves the cluster's
    /// data-volume key alone. A replaced setting is resolved exactly as a
    /// create resolves it, so the stored record stays the effective settings
    /// every reader reports.
    fn encryption_info_after(&self, submitted: Option<&EncryptionInfo>) -> EncryptionInfo {
        match submitted {
            None => self.encryption_info.clone(),
            Some(submitted) => resolve_encryption_info(
                &self.region,
                Some(EncryptionInfo {
                    encryption_at_rest: submitted
                        .encryption_at_rest
                        .clone()
                        .or_else(|| self.encryption_info.encryption_at_rest.clone()),
                    encryption_in_transit: submitted
                        .encryption_in_transit
                        .clone()
                        .or_else(|| self.encryption_info.encryption_in_transit.clone()),
                }),
            ),
        }
    }

    /// Record `change` as the cluster's one in-flight update and move it to
    /// `UPDATING`, handing back the ARN of the operation that will apply it.
    ///
    /// Reached only through
    /// [`ComAmazonawsKafkaKafkaState::start_cluster_update`], which files the
    /// operation this ARN names in the registry: a cluster is `UPDATING` with an
    /// active operation ARN exactly while it holds a [`PendingUpdate`], and that
    /// ARN always resolves for a cluster-operation reader, whichever update
    /// operation recorded it.
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
    /// reader derives the cluster's new behaviour from one place. `applied_at`
    /// is the instant the operation was due, which a change that records a time
    /// of its own uses.
    fn apply_change(&mut self, change: ClusterChange, applied_at: SystemTime) {
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
            ClusterChange::BrokerType { instance_type } => {
                // Every broker of the cluster moves to the one type the update
                // named, which is what `BrokerNodeGroupInfo$InstanceType` is: the
                // type of the group's nodes, not of one node.
                self.broker_node_group_info.instance_type = instance_type;
            }
            ClusterChange::BrokerCount {
                number_of_broker_nodes,
            } => {
                // The brokers ARE the record: the count the cluster reports and
                // the nodes its endpoints and its node listing are built from
                // move together.
                self.apply_broker_count_change(number_of_broker_nodes, applied_at);
            }
            ClusterChange::KafkaVersion {
                kafka_version,
                configuration_info,
            } => {
                // The brokers now run the target version, which is what every
                // reader derives `BrokerSoftwareInfo$KafkaVersion` from.
                self.kafka_version = kafka_version;
                // A custom configuration is applied ON the new version, so a
                // request that named one leaves the cluster running it; one that
                // named none keeps the configuration the cluster already had.
                if let Some(info) = configuration_info {
                    self.configuration_info = Some(info);
                }
            }
            ClusterChange::Configuration { configuration_info } => {
                // The brokers now run the revision the update named, which is
                // what every reader derives the cluster's `ConfigurationArn` and
                // `ConfigurationRevision` from.
                self.configuration_info = Some(configuration_info);
            }
            ClusterChange::Security {
                client_authentication,
                encryption_info,
            } => {
                // Resolved through the SAME pair the operation's target
                // `MutableClusterInfo` was derived from, so the settings the
                // cluster ends up running are the ones the operation reported
                // it would apply.
                let authentication =
                    self.client_authentication_after(client_authentication.as_ref());
                let encryption = self.encryption_info_after(encryption_info.as_ref());
                self.client_authentication = authentication;
                self.encryption_info = encryption;
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

    /// The ARN of the cluster operation in flight on this cluster, while one is:
    /// the ARN the update call that started it answered with, and `None` once the
    /// clock has made that update due.
    ///
    /// The in-flight operation is part of the STORED cluster record, so every
    /// reader of a cluster reports it off this one accessor: the `ClusterInfo`
    /// view (`DescribeCluster`, `ListClusters`) and the `Cluster` view
    /// (`DescribeClusterV2`, `ListClustersV2`) cannot disagree about which
    /// operation a cluster has in flight, and a client that tracks an update
    /// through either reader sees the same ARN.
    pub fn active_operation_arn(&self) -> Option<String> {
        self.pending_update
            .as_ref()
            .map(|update| update.operation_arn.clone())
    }

    /// The cluster as `ClusterInfo`, the shape both `DescribeCluster` and
    /// `ListClusters` report.
    pub fn to_cluster_info(&self) -> ClusterInfo {
        ClusterInfo {
            active_operation_arn: self.active_operation_arn(),
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
            active_operation_arn: self.active_operation_arn(),
            cluster_type: Some(self.cluster_type.clone()),
            cluster_arn: Some(self.arn.clone()),
            cluster_name: Some(self.name.clone()),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            current_version: Some(self.current_version.clone()),
            state: Some(self.state.clone()),
            state_info: None,
            tags: Some(self.tags.clone().into_iter().collect()),
            // Exactly one of the per-kind members is reported, the one this
            // cluster's `ClusterType` names: the provisioned detail is derived
            // from the broker configuration the record holds, the serverless one
            // from its [`ServerlessConfig`].
            provisioned: (self.cluster_type == CLUSTER_TYPE_PROVISIONED)
                .then(|| self.to_provisioned()),
            serverless: self.serverless.as_ref().map(ServerlessConfig::to_serverless),
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

    /// One stored broker node as the `NodeInfo` `ListNodes` reports.
    ///
    /// Every member is derived from the record: the node's own placement and
    /// endpoint from [`BrokerNode`], and the attributes that belong to the whole
    /// broker group — the instance type and the deployed software — from the
    /// cluster, so an applied `UpdateBrokerType` or configuration change moves
    /// what every node reports at once. `ZookeeperNodeInfo` and
    /// `ControllerNodeInfo` are absent: a broker node is neither, and the
    /// model's `NodeType` names `BROKER` alone.
    pub fn to_node_info(&self, broker: &BrokerNode) -> NodeInfo {
        NodeInfo {
            added_to_cluster_time: rendered_instant(broker.added_to_cluster_time),
            broker_node_info: Some(BrokerNodeInfo {
                attached_eni_id: Some(broker.attached_eni_id.clone()),
                // `BrokerNodeInfo$BrokerId` is a `__double` in the model, so the
                // 1-based id travels as the number it is.
                broker_id: Some(f64::from(broker.id)),
                client_subnet: Some(broker.client_subnet.clone()),
                client_vpc_ip_address: Some(broker.client_vpc_ip_address.clone()),
                current_broker_software_info: Some(self.broker_software_info()),
                // One endpoint per node: the host every access path's bootstrap
                // string carries for this broker, which is what the cluster's
                // endpoint list is built from too.
                endpoints: Some(vec![broker.endpoint_host.clone()]),
            }),
            controller_node_info: None,
            instance_type: Some(self.broker_node_group_info.instance_type.clone()),
            node_arn: Some(self.node_arn(broker.id)),
            node_type: Some(NODE_TYPE_BROKER.to_string()),
            zookeeper_node_info: None,
        }
    }

    /// The ARN of broker node `broker_id` on this cluster.
    ///
    /// A node is a sub-resource of the cluster it belongs to, so it is named by
    /// the cluster's own ARN followed by the broker id — derived from the stored
    /// cluster ARN, which keeps a node's ARN in the same partition, region and
    /// account as the cluster that holds it, the same way [`Self::topic_arn`]
    /// derives a topic's.
    pub fn node_arn(&self, broker_id: i32) -> String {
        format!("{}/{broker_id}", self.arn)
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

/// How many broker nodes a cluster may be left holding: `target` bounded to the
/// range the model admits for a broker count
/// (`CreateClusterRequest$NumberOfBrokerNodes` and
/// `UpdateBrokerCountRequest$TargetNumberOfBrokerNodes` are both
/// `__integerMin1Max15`), i.e. 1..=[`MAX_BROKER_NODES_PER_CLUSTER`].
///
/// Every path that sizes a broker list, or derives the ids of the nodes a change
/// adds or removes, goes through this ONE bound: a request value is what those
/// paths are driven by, and the handlers that refuse an out-of-range value are
/// what make such a value a bad request rather than an allocation. Bounding it
/// first is also what keeps the id arithmetic those paths do inside `i32`.
fn admissible_broker_count(target: i32) -> i32 {
    target.clamp(1, MAX_BROKER_NODES_PER_CLUSTER)
}

/// What one cluster ordinal seeds: the cluster's ARN, and the DNS suffix its
/// endpoints sit under.
struct ClusterIdentity {
    arn: String,
    dns_suffix: String,
}

/// Derive a cluster's identity from the registry's monotonic counter, so one
/// number seeds its uuid, its ARN tail and its endpoint host names — and both
/// create paths mint an ARN the same way, whichever kind of cluster they
/// register.
fn cluster_identity(region: &str, name: &str, suffix: u64) -> ClusterIdentity {
    // The counter, as the 1-based ordinal AWS puts at the end of a cluster ARN.
    // The SAME value seeds the digest, so a cluster's uuid, DNS suffix and ARN
    // tail all follow from one number.
    let ordinal = suffix + 1;
    let digest = stable_digest(&format!(
        "{}:{region}:{name}:{ordinal}",
        ::engine::emulator_account_id()
    ));
    let cluster_uuid = uuid_v4_shaped_from_digest(&digest);
    ClusterIdentity {
        arn: ::engine::emulator_arn(
            "kafka",
            region,
            &format!("cluster/{name}/{cluster_uuid}-{ordinal}"),
        ),
        // The DNS suffix of an MSK endpoint is a short opaque cluster id plus the
        // cell the cluster landed in: `b-1.demo.a1b2c3.c4.kafka.<region>...`.
        dns_suffix: format!(
            "{:02x}{:02x}{:02x}.c{}",
            digest[16],
            digest[17],
            digest[18],
            1 + u32::from(digest[19] % 9)
        ),
    }
}

/// The `Provisioned` body of a `CreateClusterV2`, as the `CreateClusterInput`
/// [`Cluster::provisioned`] mints from.
///
/// The two requests carry the SAME provisioned configuration under different
/// shape names, so regrouping the v2 body here is what makes a v2 provisioned
/// create and a v1 create register one kind of record instead of two near-copies
/// that could drift apart.
pub fn provisioned_create_input(
    cluster_name: String,
    tags: Option<HashMap<String, String>>,
    request: ProvisionedRequest,
) -> CreateClusterInput {
    CreateClusterInput {
        broker_node_group_info: request.broker_node_group_info,
        rebalancing: request.rebalancing,
        client_authentication: request.client_authentication,
        cluster_name,
        configuration_info: request.configuration_info,
        encryption_info: request.encryption_info,
        enhanced_monitoring: request.enhanced_monitoring,
        open_monitoring: request.open_monitoring,
        kafka_version: request.kafka_version,
        logging_info: request.logging_info,
        number_of_broker_nodes: request.number_of_broker_nodes,
        tags,
        storage_mode: request.storage_mode,
    }
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

/// The broker type a cluster operation reports, as `MutableClusterInfo`.
///
/// `InstanceType` is the only member a broker-type update touches, so it is the
/// only one set: an operation describes the attributes it changes, not the whole
/// cluster — the same rule [`storage_cluster_info`] follows.
fn broker_type_cluster_info(instance_type: &str) -> MutableClusterInfo {
    MutableClusterInfo {
        instance_type: Some(instance_type.to_string()),
        ..MutableClusterInfo::default()
    }
}

/// The software attributes a cluster operation reports, as `MutableClusterInfo`:
/// the Apache Kafka version the brokers run and the configuration revision they
/// run it with.
///
/// Those are what an Apache Kafka version update touches, so they are the only
/// members set — the same rule [`storage_cluster_info`] follows.
fn kafka_version_cluster_info(
    kafka_version: &str,
    configuration_info: Option<ConfigurationInfo>,
) -> MutableClusterInfo {
    MutableClusterInfo {
        kafka_version: Some(kafka_version.to_string()),
        configuration_info,
        ..MutableClusterInfo::default()
    }
}

/// The configuration revision a cluster operation reports, as
/// `MutableClusterInfo`.
///
/// `ConfigurationInfo` is the only member an `UpdateClusterConfiguration` call
/// touches, so it is the only one set — the same rule [`storage_cluster_info`]
/// follows.
fn configuration_cluster_info(configuration_info: Option<ConfigurationInfo>) -> MutableClusterInfo {
    MutableClusterInfo {
        configuration_info,
        ..MutableClusterInfo::default()
    }
}

/// The security attributes a cluster operation reports, as `MutableClusterInfo`:
/// the client authentication the cluster accepts and the encryption it runs
/// with.
///
/// Those are what a security update touches, so they are the only members set,
/// and the encryption is reported resolved rather than as submitted — the same
/// rule [`storage_cluster_info`] follows.
fn security_cluster_info(
    client_authentication: Option<ClientAuthentication>,
    encryption_info: EncryptionInfo,
) -> MutableClusterInfo {
    MutableClusterInfo {
        client_authentication,
        encryption_info: Some(encryption_info),
        ..MutableClusterInfo::default()
    }
}

/// The connectivity attributes a cluster operation reports, as
/// `MutableClusterInfo`.
///
/// The broker access configuration and the ZooKeeper access setting are what a
/// connectivity update touches, so they are the only members set, and the
/// ZooKeeper setting is reported resolved rather than absent — the same rule
/// [`storage_cluster_info`] follows.
fn connectivity_cluster_info(
    connectivity_info: Option<ConnectivityInfo>,
    zookeeper_access_enabled: bool,
) -> MutableClusterInfo {
    MutableClusterInfo {
        connectivity_info,
        zookeeper_access: Some(ZookeeperAccess {
            enabled: Some(zookeeper_access_enabled),
        }),
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

/// An instant as the string a `__string` timestamp member carries: ISO-8601
/// (RFC 3339), the form every modelled timestamp of this service travels in.
/// `None` only for an instant no date-time can represent, which no stored record
/// holds.
fn rendered_instant(at: SystemTime) -> Option<String> {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
}

/// An EC2 long resource id for `kind` (`"eni"`): the prefix, a hyphen and 17
/// lowercase hexadecimal digits — the longer of the two published EC2
/// resource-id forms, which is what [`is_ec2_resource_id`] accepts. A pure
/// function of `seed`, so the resource it names keeps one id.
fn ec2_long_id(kind: &str, seed: &str) -> String {
    use std::fmt::Write;

    let digest = stable_digest(seed);
    let mut id = format!("{kind}-");
    // Nine bytes render 18 digits; the id takes the first 17 of them.
    for byte in digest.iter().take(9) {
        // Infallible: `{:02x}` raises no formatting error, and a `String` is the
        // one `fmt::Write` that cannot fail for any other reason.
        let _ = write!(id, "{byte:02x}");
    }
    id.truncate(kind.len() + 1 + 17);
    id
}

/// The private IPv4 address a network interface this service creates answers on,
/// as a pure function of `seed` so the interface keeps one address.
///
/// CHOSEN, not documented: the real service allocates from the client subnet's
/// CIDR, and this emulator holds no VPC inventory to read a CIDR from. The
/// address is drawn from 10.0.0.0/8 — a private range (RFC 1918) — with a host
/// octet in 1..=254, so it is never a network or broadcast address.
fn private_ipv4(seed: &str) -> String {
    let digest = stable_digest(seed);
    let host = 1 + u32::from(digest[2]) % 254;
    format!("10.{}.{}.{host}", digest[0], digest[1])
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

/// The id of the call this service made to ANOTHER service on the caller's behalf,
/// as it is reported back to that caller ([`network`]).
///
/// A request identifier is generated per call — it is what correlates one answer
/// with the call behind it — so it is drawn afresh rather than derived from
/// anything in the request: the seed is the system clock in nanoseconds together
/// with the OS-seeded randomness a `RandomState` draws, and it is rendered by the
/// one minter behind every identifier this module gives that shape to
/// ([`uuid_v4_shaped_from_digest`]).
fn new_peer_request_id() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::BuildHasher;

    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();
    let entropy = RandomState::new().hash_one(nanos);
    uuid_v4_shaped_from_digest(&stable_digest(&format!("{nanos}#{entropy}")))
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

/// Encode the registry key a page stopped at as a pagination token, marked with
/// `prefix` so a token minted for one list cannot be mistaken for another's.
///
/// Base64 of a marked plaintext: the token is ENCODED, not authenticated, so a
/// caller could decode it or mint one. That is harmless here — the key inside is
/// only used as a `>` comparison against the keys of the caller's own registry,
/// so a forged resume point can only skip or repeat entries the caller may
/// already list.
pub fn encode_page_token(prefix: &str, last_key: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{prefix}{last_key}"))
}

/// The registry key a token resumes after, or `None` when the token is not one
/// `prefix` was minted with.
pub fn decode_page_token(prefix: &str, token: &str) -> Option<String> {
    let decoded = ::aws_smithy_types::base64::decode(token).ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    decoded.strip_prefix(prefix).map(ToString::to_string)
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

/// The input member the cluster-scoped operations carry their cluster's ARN in,
/// named as the API documents it on the wire — the camelCase spelling the
/// request itself uses, not the Smithy structure member's identifier. What those
/// operations' refusals report as the parameter at fault, beside
/// [`REPLICATOR_ARN_MEMBER`].
pub const CLUSTER_ARN_MEMBER: &str = "clusterArn";

/// The member `CreateVpcConnection` carries the cluster's ARN in, under the same
/// rule: that operation names the cluster it connects to `TargetClusterArn`, so
/// its refusal reports that member's wire name rather than the one every
/// cluster-scoped operation binds.
pub const TARGET_CLUSTER_ARN_MEMBER: &str = "targetClusterArn";

/// The member the readers of one cluster OPERATION carry their target's ARN in,
/// named the same way.
pub const CLUSTER_OPERATION_ARN_MEMBER: &str = "clusterOperationArn";

/// The cluster-ARN-SPECIFIC refusal wording, which the SCRAM-secret surface and
/// the client-VPC-connection listing answer a cluster ARN they cannot parse with
/// instead of the generic [`BAD_REQUEST_MESSAGE`] (the apostrophe is U+2019, as
/// the service writes it).
///
/// Fixed text end to end, like the generic one: the offending value is not part
/// of it. Which of the two an operation answers is the operation's, so the two
/// are named separately here rather than evened out — a caller of one surface is
/// told what that surface tells callers.
pub const CLUSTER_ARN_NOT_VALID_MESSAGE: &str =
    "The specified cluster ARN isn\u{2019}t valid. Specify a valid cluster ARN, and try again.";

/// The refusal an operation answers a value that is not shaped like the kind of
/// ARN the member naming it carries ([`is_cluster_arn`],
/// [`is_cluster_operation_arn`]): a `BadRequestException` (HTTP 400) whose
/// message is the service's generic parameter sentence
/// ([`BAD_REQUEST_MESSAGE`]) — the WHOLE text, with no part taken from the
/// request, so two calls carrying different unusable values are told the same
/// thing and the offending value is never echoed back — and which names the
/// member at fault in `invalidParameter`, by its wire name
/// ([`CLUSTER_ARN_MEMBER`], [`TARGET_CLUSTER_ARN_MEMBER`],
/// [`CLUSTER_OPERATION_ARN_MEMBER`]).
///
/// Built HERE rather than beside each shape check, so no handler composes a
/// sentence of its own around the ARN it was handed: one condition, one
/// envelope, whichever operation met it.
pub fn malformed_arn(member: &str) -> BadRequestException {
    parameter_refusal(member, BAD_REQUEST_MESSAGE)
}

/// The same refusal for the surfaces that answer the cluster-ARN-specific
/// wording ([`CLUSTER_ARN_NOT_VALID_MESSAGE`]): the member at fault is the
/// cluster's own ARN, which is what that sentence is about.
pub fn invalid_cluster_arn() -> BadRequestException {
    parameter_refusal(CLUSTER_ARN_MEMBER, CLUSTER_ARN_NOT_VALID_MESSAGE)
}

/// The sentence an operation that models NO `NotFoundException` answers a
/// well-formed resource ARN this endpoint's region holds nothing for with.
///
/// The SAME fixed, request-independent sentence [`malformed_arn`] carries
/// ([`BAD_REQUEST_MESSAGE`]): both are the one thing this service says about a
/// parameter it cannot act on, and neither takes any part of its text from the
/// request — not the value, not the region the endpoint serves. Operations that
/// DO model `NotFoundException` answer [`RESOURCE_NOT_FOUND_MESSAGE`] instead,
/// which is that error's own sentence.
///
/// Named here, beside the shape refusal, so no handler composes a sentence of
/// its own around the reference it was handed.
pub const UNRESOLVED_REFERENCE_MESSAGE: &str = BAD_REQUEST_MESSAGE;

/// One `BadRequestException` naming a request member and carrying one of the
/// service's fixed sentences, so the two refusals above cannot drift apart on
/// how a parameter is named or on the fact that the message carries nothing of
/// the request.
fn parameter_refusal(member: &str, message: &str) -> BadRequestException {
    BadRequestException {
        invalid_parameter: Some(member.to_string()),
        message: Some(message.to_string()),
    }
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

/// Whether `value` is an AWS Secrets Manager secret ARN —
/// `arn:<partition>:secretsmanager:<region>:<account>:secret:<name>`, the form
/// `SecretArnList` carries ("List of AWS Secrets Manager secret ARNs") and the
/// same requirement the shared ARN vocabulary states for such a member: the
/// vendor field is `secretsmanager` and the resource names a `secret`.
///
/// Read through [`arn_resource_of`], the one ARN split of this module, so this
/// predicate and the ones over this service's own ARNs cannot disagree about the
/// fields in front of the resource. The same distinction between the two client
/// errors as [`is_cluster_arn`]: a value that is not a secret ARN at all is a
/// request the service cannot act on (`BadRequestException`), not a resource it
/// cannot find.
pub fn is_secrets_manager_secret_arn(value: &str) -> bool {
    let Some(resource) = arn_resource_of(value, "secretsmanager") else {
        return false;
    };
    // A secret's resource field is `secret:<name>`; a name may itself carry ':'
    // and '/', so only the first separator ends the type and the rest is the
    // name, which must not be empty — an ARN naming no secret names none.
    resource
        .strip_prefix("secret:")
        .is_some_and(|name| !name.is_empty())
}

/// The sentence Amazon MSK answers a `SecretArnList` entry that is not an AWS
/// Secrets Manager secret ARN at all with: fixed text end to end, with the
/// offending value NOT echoed back, like this service's other parameter
/// refusals. Shared by the two SCRAM batch operations, so one condition is
/// stated one way whichever of them met it.
pub const SECRET_ARN_INVALID_MESSAGE: &str = "The provided secret ARN is invalid.";

/// Whether `value`'s resource part is `<kind>/<name>/<id>` — the shape a
/// cluster, a configuration and a replicator ARN all take, so the predicates
/// above cannot disagree about what a well-formed ARN of their kind is.
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
/// `arn:<partition>:kafka:<region>:<account>:channel/<cluster>/<cluster-id>/<name>`,
/// optionally followed by the channel's own id: a channel is a sub-resource of
/// the cluster it reads from, so its ARN names that cluster and then the channel.
///
/// The cluster name, the cluster id and the channel name are what identify a
/// channel — a channel name is unique within its cluster — and
/// [`Channel::streaming`] appends the channel's own id after them. BOTH forms are
/// channel ARNs here: the behavioural corpus records the real service answering
/// `NotFoundException`, not `BadRequestException`, for one that stops at the
/// channel name, so that trailing id is not part of what a caller must supply.
///
/// Same division of labour as [`is_cluster_arn`]: a value that is not an ARN of
/// this kind is a `BadRequestException`, and `NotFoundException` is reserved for
/// a well-formed ARN no live channel holds.
pub fn is_channel_arn(value: &str) -> bool {
    let Some(resource) = kafka_arn_resource(value) else {
        return false;
    };
    let mut segments = resource.split('/');
    if segments.next() != Some(CHANNEL_RESOURCE_TYPE) {
        return false;
    }
    let identity: Vec<&str> = segments.collect();
    matches!(identity.len(), 3 | 4) && identity.iter().all(|segment| !segment.is_empty())
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
    arn_resource_of(value, "kafka")
}

/// The resource part of `value` when it is an ARN of `vendor` in some partition,
/// region and account, and `None` otherwise.
///
/// The ONE ARN split of this module, shared by the predicates over this
/// service's own ARNs and by the one over the Secrets Manager secret ARNs a
/// member of this API carries, so they cannot disagree about the five fields in
/// front of the resource. `splitn` with the field count leaves the whole
/// resource — colons included — as the last field, so an ARN of the
/// `<type>:<id>` form splits exactly like one of the `<type>/<id>` form, and the
/// vendor comparison is exact because an ARN namespace is a fixed lowercase
/// token.
fn arn_resource_of<'a>(value: &'a str, vendor: &str) -> Option<&'a str> {
    let mut fields = value.splitn(6, ':');
    let (Some("arn"), Some(partition), Some(found), Some(region), Some(account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    if found != vendor || partition.is_empty() || region.is_empty() || account.is_empty() {
        return None;
    }
    Some(resource)
}

/// Whether `c` may appear in a topic or configuration name this service admits.
///
/// ONE character set for the two names a caller supplies and this service turns
/// into a resource identity: ASCII alphanumerics and the three separators `.`,
/// `_` and `-`. It is Apache Kafka's own name character set
/// (`org.apache.kafka.common.internals.Topic.validate`), and the one Amazon MSK
/// admits a configuration name from, so topic names and configuration names are
/// tested against the same predicate rather than two copies of the same list. A
/// cluster name is NOT governed by it: the model bounds `ClusterName`'s length
/// and declares no character set for it.
pub fn is_resource_name_character(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')
}

/// The first character of `name` that is outside the service's name character
/// set, or `None` when every one of them is in it.
///
/// A name reaches the store only through this test: it becomes the resource's
/// identity — it keys a name index and goes into the ARN — so a name carrying
/// anything else names no resource this service could have issued.
pub fn name_character_outside_set(name: &str) -> Option<char> {
    name.chars().find(|c| !is_resource_name_character(*c))
}

/// What the service answers a name built from characters outside its name
/// character set with.
///
/// The register the service's name validator refuses in: it reports that the
/// name cannot be used and echoes no part of the request — not the name, not the
/// offending character — exactly as the same validator's refusal of a name of
/// the wrong length does ([`RESOURCE_NAME_LENGTH_MESSAGE`]). The same sentence
/// for every name the set rejects, since one rule was broken.
pub const RESOURCE_NAME_INVALID_CHARACTERS_MESSAGE: &str =
    "The resource name contains invalid characters";

/// What the service answers a name whose length is outside the bound with.
///
/// ONE sentence for both ends of the one constraint — a name too long and a name
/// too short broke the same rule — and it echoes no part of the request, so a
/// caller is not handed back a name the service refused to register.
pub const RESOURCE_NAME_LENGTH_MESSAGE: &str =
    "The resource name contains too many or too few characters";

/// The shortest and the longest configuration name Amazon MSK registers: a
/// 128-character name is created and read back byte-identical, while 129
/// characters and the empty string are both refused, so the bound is closed at
/// both ends.
pub const MIN_CONFIGURATION_NAME_LENGTH: usize = 1;
pub const MAX_CONFIGURATION_NAME_LENGTH: usize = 128;

/// Whether `name` is of a length this service registers a configuration under.
///
/// Counted in CHARACTERS, not bytes: the bound is on the name a caller reads
/// back, so a multi-byte character counts once — and a name outside it is
/// refused rather than truncated into a resource nobody asked for.
pub fn is_configuration_name_length(name: &str) -> bool {
    (MIN_CONFIGURATION_NAME_LENGTH..=MAX_CONFIGURATION_NAME_LENGTH).contains(&name.chars().count())
}

/// Why `name` is not a legal Apache Kafka topic name, or `None` when it is.
///
/// Apache Kafka's own rule (`org.apache.kafka.common.internals.Topic.validate`),
/// which the MSK topic operations sit in front of: a name is at most
/// [`MAX_TOPIC_NAME_LENGTH`] characters, is neither `.` nor `..`, and is made up
/// only of the service's name characters ([`is_resource_name_character`]). The
/// returned sentence names the member and the constraint it broke, so a refusal
/// says which part of the request was wrong.
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
    if let Some(illegal) = name_character_outside_set(name) {
        return Some(format!(
            "A topic name may contain only ASCII alphanumerics, '.', '_' and '-', but this one contains {illegal:?}."
        ));
    }
    None
}

/// Whether `value` could be an Amazon MSK broker type.
///
/// Two conditions, both from what the model says such a value is:
///
/// * its length is within [`INSTANCE_TYPE_MIN_LEN`]..=[`INSTANCE_TYPE_MAX_LEN`],
///   the `length` trait on the member that STORES it
///   (`BrokerNodeGroupInfo$InstanceType`), which an update's own
///   `TargetInstanceType` (a plain `__string`) does not carry; and
/// * it has the `<prefix>.<family>.<size>` form every MSK broker type takes —
///   `kafka.m5.large` … `kafka.m5.24xlarge` as `BrokerNodeGroupInfo$InstanceType`'s
///   documentation lists them, and the families AWS added after that prose
///   (`kafka.m7g.*`, `express.m7g.*`) alike.
///
/// The FORM is checked rather than the model's prose list, which no longer
/// enumerates every type the service accepts; a value that cannot be a broker
/// type at all is refused with `BadRequestException`.
pub fn is_broker_instance_type(value: &str) -> bool {
    let length = value.chars().count();
    if !(INSTANCE_TYPE_MIN_LEN..=INSTANCE_TYPE_MAX_LEN).contains(&length) {
        return false;
    }
    let mut segments = value.split('.');
    matches!(
        (
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next()
        ),
        (Some(prefix), Some(family), Some(size), None)
            if !prefix.is_empty() && !family.is_empty() && !size.is_empty()
    )
}
