// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-eks-awswesleyfrontend service.
//!
//! This holds the cluster lifecycle slice: `CreateCluster` and `RegisterCluster`
//! write a [`ClusterRecord`] here, and `DescribeCluster`, `ListClusters`,
//! `TagResource`, `ListTagsForResource` and `DeleteCluster` all compute their
//! responses from it. Nothing in `src/ops/` keeps a second copy of a cluster.
//!
//! It also holds the managed node group slice: `CreateNodegroup` writes a
//! [`NodegroupRecord`] here and `DescribeNodegroup` computes its whole response
//! from that record, so a node group's server-generated parts (its ARN,
//! `createdAt`, defaults) are written once and read back unchanged.
//!
//! One record covers both kinds of cluster this API knows: an Amazon EKS cluster
//! `CreateCluster` provisions a control plane for, and an external Kubernetes
//! cluster `RegisterCluster` connects. The members only a provisioned control
//! plane has (`endpoint`, `roleArn`, `resourcesVpcConfig`, `version`,
//! `platformVersion`) are optional on the record, because a connected cluster
//! reports none of them — it reports [`ClusterRecord::connector_config`] instead.
//!
//! It also holds the add-on slice: `CreateAddon` writes an [`AddonRecord`] here
//! and `DescribeAddon` reads it back. An add-on lives INSIDE a cluster, so its
//! key carries the cluster's key and `DeleteCluster` drops the add-ons of the
//! cluster it removes.
//!
//! It also holds the managed-capability slice that hangs off a cluster:
//! `CreateCapability` writes a [`CapabilityRecord`], and `DescribeCapability`,
//! `ListCapabilities` and `DeleteCapability` compute their responses from it.
//!
//! It also holds the access-entry slice, as part of the cluster record rather than
//! beside it: an access entry belongs to exactly one cluster, so
//! [`ClusterRecord::access_entries`] is where `CreateAccessEntry` writes and
//! `DescribeAccessEntry`, `ListAccessEntries`, `AssociateAccessPolicy` and
//! `ListAssociatedAccessPolicies` read — and deleting the cluster takes its access
//! entries with it, without a second map to keep in step.
//!
//! It also holds the EKS Pod Identity association slice:
//! `CreatePodIdentityAssociation` writes a [`PodIdentityAssociationRecord`] here
//! and `DescribePodIdentityAssociation`, `UpdatePodIdentityAssociation`,
//! `DeletePodIdentityAssociation` and `ListPodIdentityAssociations` read, mutate
//! and remove exactly that record.
//!
//! It also holds the Kubernetes version calendar `DescribeClusterVersions`
//! reports ([`ClusterVersionRecord`]), which is seeded once because it is what
//! Amazon EKS publishes rather than per-account data.
//!
//! An asynchronous cluster update lives ON the cluster it updates
//! ([`ClusterRecord::updates`]), so `AssociateEncryptionConfig` and
//! `AssociateIdentityProviderConfig` write one there, `DescribeUpdate` reads it
//! back and `CancelUpdate` records its cancellation on it; deleting the cluster
//! drops its updates with it rather than leaving them behind in a second map.
//!
//! The identity-provider slice lives on the same record:
//! `AssociateIdentityProviderConfig` writes an [`IdentityProviderConfigRecord`]
//! into the owning cluster's [`ClusterRecord::identity_provider_configs`], and
//! `DescribeIdentityProviderConfig` reads it back. Holding them on the cluster —
//! rather than in a second map beside it — is what makes a config a part of its
//! cluster: deleting the cluster takes its configs with it, and a config name is
//! scoped to one cluster, which is exactly how the API addresses it
//! (`/clusters/{clusterName}/identity-provider-configs/...`).

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    AccessConfigResponse, AccessEntry, AccessScope, Addon, AddonHealth,
    AddonNamespaceConfigResponse, AssociatedAccessPolicy, Cancellation, Capability,
    CapabilityConfigurationResponse, CapabilityHealth, CapabilityIssue, CapabilitySummary,
    Certificate, Cluster, ClusterVersionInformation, ComputeConfigResponse,
    ConnectorConfigResponse,
    ControlPlaneScalingConfig, EncryptionConfig, Identity, KubeApiServerConfigResponse,
    KubeControllerManagerConfigResponse, KubeSchedulerConfigResponse,
    KubernetesNetworkConfigResponse, LaunchTemplateSpecification, Logging, NodeRepairConfig,
    Nodegroup, NodegroupHealth, NodegroupScalingConfig, NodegroupUpdateConfig,
    OidcIdentityProviderConfig, OutpostConfigResponse, PodIdentityAssociation,
    PodIdentityAssociationSummary, RemoteAccessConfig, RemoteNetworkConfigResponse,
    StorageConfigResponse, Taint, Update, UpdateParam, UpgradePolicyResponse, VpcConfigResponse,
    WarmPoolConfig, ZonalShiftConfigResponse,
};
use crate::ops::types::{
    AllowedValuesConstraint, ControlPlaneConfigInfo, ControlPlaneScalingTierInfo,
    DurationConstraints, DurationParameterConfig,
    HorizontalPodAutoscalerControllerVersionConfig, IntegerConstraints, IntegerParameterConfig,
    IntegerRangeConstraint, KubeApiServerVersionConfig, KubeControllerManagerVersionConfig,
    KubeSchedulerVersionConfig, NodeResourcesFitVersionConfig, PodGcControllerVersionConfig,
    PortRangeConstraints, PortRangeParameterConfig, ResourceConstraints, ResourceWeight,
    ScoringStrategy, ScoringStrategyConfig, ScoringStrategyConstraints, ServiceNodePortRange,
};

/// The `ClusterStatus` members this slice uses. The model's enum
/// (`smithy_query query="shape" shape="ClusterStatus"`) is
/// `CREATING | ACTIVE | DELETING | FAILED | UPDATING | PENDING`; `Cluster$status`
/// is a plain string member, so the status is stored as the wire token.
///
/// `NodegroupStatus` (`CREATING | ACTIVE | UPDATING | DELETING | CREATE_FAILED |
/// DELETE_FAILED | DEGRADED`) spells its first two members with the same tokens,
/// so the node group slice reuses [`STATUS_CREATING`] and [`STATUS_ACTIVE`]
/// rather than declaring a second pair of identical constants.
pub const STATUS_CREATING: &str = "CREATING";
/// See [`STATUS_CREATING`].
pub const STATUS_ACTIVE: &str = "ACTIVE";
/// See [`STATUS_CREATING`].
pub const STATUS_DELETING: &str = "DELETING";
/// See [`STATUS_CREATING`]. The status a cluster `RegisterCluster` connected
/// reports until the connector manifest is applied in the external cluster.
pub const STATUS_PENDING: &str = "PENDING";
/// See [`STATUS_CREATING`].
pub const STATUS_FAILED: &str = "FAILED";
/// See [`STATUS_CREATING`] — the status a cluster reports while one of its
/// asynchronous updates is in flight.
pub const STATUS_UPDATING: &str = "UPDATING";

/// The `UpdateStatus` members this slice uses. The model's enum
/// (`smithy_query query="shape" shape="UpdateStatus"`) is
/// `InProgress | Failed | Cancelled | Successful`; `Update$status` deserialises
/// to a plain string, so the status is stored as the wire token.
pub const UPDATE_IN_PROGRESS: &str = "InProgress";
/// See [`UPDATE_IN_PROGRESS`].
pub const UPDATE_SUCCESSFUL: &str = "Successful";
/// See [`UPDATE_IN_PROGRESS`] — the status `CancelUpdate` moves an update to
/// once it has stopped it.
pub const UPDATE_CANCELLED: &str = "Cancelled";
/// See [`UPDATE_IN_PROGRESS`] — the fourth member of the enum, which an update
/// reaches when the change it carries could not be made.
pub const UPDATE_FAILED: &str = "Failed";

/// The `UpdateType` member an encryption association reports
/// (`smithy_query query="shape" shape="UpdateType"`).
pub const UPDATE_TYPE_ASSOCIATE_ENCRYPTION_CONFIG: &str = "AssociateEncryptionConfig";

/// The `UpdateParamType` key an encryption association reports its requested
/// configuration under (`smithy_query query="shape" shape="UpdateParamType"`).
pub const UPDATE_PARAM_ENCRYPTION_CONFIG: &str = "EncryptionConfig";

/// How long an asynchronous cluster update reports `InProgress` before the clock
/// owes it `Successful` and the change it carries is applied to the cluster.
///
/// No duration is published for an EKS update — `DescribeUpdate` says only "When
/// the status of the update is `Successful`, the update is complete" — so this
/// span is CHOSEN rather than cited: non-zero, so a caller that describes the
/// update straight after starting it still observes `InProgress` (and the
/// `UPDATING` cluster it goes with), and short enough to be worth waiting for.
/// The due instant is stored on the update record; nothing sleeps for it.
pub const UPDATE_COMPLETION_DELAY: Duration = Duration::from_millis(500);

/// How long a newly created cluster reports `CREATING` before the clock owes it
/// `ACTIVE`.
///
/// CreateCluster's own documentation says only "In most cases, it takes several
/// minutes to create a cluster" — no duration is published, so this span is
/// CHOSEN rather than cited: long enough that a caller which describes the
/// cluster straight after creating it still observes `CREATING` (and the state
/// errors that go with it), short enough to be worth waiting for. The due
/// instant is stored on the record; nothing sleeps for it.
pub const CLUSTER_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// The `CapabilityStatus` members this slice uses. The model's enum
/// (`smithy_query query="shape" shape="CapabilityStatus"`) is
/// `CREATING | CREATE_FAILED | UPDATING | DELETING | DELETE_FAILED | ACTIVE |
/// DEGRADED`; `Capability$status` is stored as the wire token, as the cluster
/// status is.
pub const CAPABILITY_STATUS_CREATING: &str = "CREATING";
/// See [`CAPABILITY_STATUS_CREATING`].
pub const CAPABILITY_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`CAPABILITY_STATUS_CREATING`].
pub const CAPABILITY_STATUS_DELETING: &str = "DELETING";

/// How long a newly created capability reports `CREATING` before the clock owes it
/// `ACTIVE`.
///
/// `CreateCapability` publishes no duration for the provisioning it starts ("Amazon
/// EKS provisions and manages the infrastructure required to run the capability
/// outside of your cluster"), so this span is CHOSEN rather than cited, and matches
/// [`CLUSTER_ACTIVATION_DELAY`]: long enough that a caller which describes the
/// capability straight after creating it still observes `CREATING`, short enough to
/// be worth waiting for. The due instant is stored on the record; nothing sleeps for
/// it.
pub const CAPABILITY_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// The `CapabilityType` members whose `configuration` this service maps from the
/// request shape to the response shape. The model's enum
/// (`smithy_query query="shape" shape="CapabilityType"`) is `ACK | ARGOCD | KRO`,
/// and `CapabilityConfigurationRequest` carries a member for exactly these two:
/// `KRO` takes no configuration.
pub const CAPABILITY_TYPE_ACK: &str = "ACK";
/// See [`CAPABILITY_TYPE_ACK`].
pub const CAPABILITY_TYPE_ARGOCD: &str = "ARGOCD";

/// How long a connected cluster has to apply its connector manifest before the
/// registration lapses.
///
/// CITED, not chosen: "If the manifest isn't applied within three days, the
/// connected cluster will no longer be visible and must be deregistered using
/// DeregisterCluster" (`RegisterCluster`'s own documentation). It is reported to
/// the caller as `connectorConfig.activationExpiry`, and it is the instant at
/// which the clock owes a still-`PENDING` connected cluster the move to
/// `FAILED`.
pub const CONNECTOR_ACTIVATION_WINDOW: Duration = Duration::from_secs(3 * 24 * 60 * 60);

/// The Kubernetes version a cluster gets when `CreateCluster` is called without
/// `version`: "If you don't specify a value here, the default version available
/// in Amazon EKS is used" (`CreateCluster$version`), which the model does not
/// name — so this emulator pins one recent minor version rather than inventing a
/// value per request.
///
/// It is the row the version calendar marks `defaultVersion` (see
/// [`seed_cluster_versions`]), so the version `DescribeClusterVersions` reports
/// with `defaultVersion: true` is exactly the version a create without `version`
/// uses. The unit tests at the bottom of this module pin the two together.
///
/// The version pinned here is the NEWEST published row but one: the version
/// Amazon EKS has most recently added is available to a caller that asks for it
/// by name, while the version a create lands on when it names none still lags it
/// by one release — which is why the newest row of [`VERSION_CALENDAR`] is not
/// the default one.
pub const DEFAULT_KUBERNETES_VERSION: &str = "1.36";

/// The platform version a freshly created cluster carries. EKS platform versions
/// are `eks.<n>`, numbered from 1 within each Kubernetes minor version, so a
/// cluster created now starts at the first one.
pub const INITIAL_PLATFORM_VERSION: &str = "eks.1";

/// The `ipFamily` a cluster gets when `kubernetesNetworkConfig.ipFamily` is
/// absent: "If you don't specify a value, `ipv4` is used by default"
/// (`KubernetesNetworkConfigRequest$ipFamily`).
pub const IP_FAMILY_IPV4: &str = "ipv4";
/// The other `IpFamily` member (`smithy_query query="shape" shape="IpFamily"`).
pub const IP_FAMILY_IPV6: &str = "ipv6";

/// The service CIDR Kubernetes assigns when `serviceIpv4Cidr` is absent on an
/// `ipv4` cluster: "If you don't specify a block, Kubernetes assigns addresses
/// from either the `10.100.0.0/16` or `172.20.0.0/16` CIDR blocks"
/// (`KubernetesNetworkConfigRequest$serviceIpv4Cidr`). One of the two documented
/// blocks is pinned so a read-back is stable.
pub const DEFAULT_SERVICE_IPV4_CIDR: &str = "10.100.0.0/16";

/// The `authenticationMode` a cluster gets when `accessConfig` does not name one:
/// "If you create a cluster by using the EKS API, Amazon Web Services SDKs, or
/// CloudFormation, the default is `CONFIG_MAP`"
/// (`CreateAccessConfigRequest$authenticationMode`) — and this emulator is
/// reached through the API.
pub const DEFAULT_AUTHENTICATION_MODE: &str = "CONFIG_MAP";

/// The `supportType` a cluster gets when `upgradePolicy` is absent: "New
/// clusters, by default, have extended support enabled"
/// (`CreateCluster$upgradePolicy`).
pub const DEFAULT_SUPPORT_TYPE: &str = "EXTENDED";

/// The CIDR allowed to reach the public endpoint when `publicAccessCidrs` is
/// absent: "The default value is `0.0.0.0/0` and additionally `::/0` for
/// dual-stack `IPv6` clusters" (`VpcConfigRequest$publicAccessCidrs`).
pub const DEFAULT_PUBLIC_ACCESS_CIDR_V4: &str = "0.0.0.0/0";
/// See [`DEFAULT_PUBLIC_ACCESS_CIDR_V4`] — added for an `ipv6` cluster.
pub const DEFAULT_PUBLIC_ACCESS_CIDR_V6: &str = "::/0";

/// The region used when a request carries no SigV4 credential scope to read one
/// from. It matches the region the emulator writes into the shared AWS config it
/// generates for clients.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The number of clusters this store holds before `CreateCluster` answers
/// `ResourceLimitExceededException`.
///
/// AWS publishes an adjustable EKS service quota of 100 clusters per Region per
/// account, and exceeding a service limit is exactly what that error reports. It
/// also bounds the store, so a long-lived emulator cannot grow without limit on
/// repeated creates.
pub const MAX_CLUSTERS: usize = 100;

/// The number of clusters this store holds ACROSS EVERY REGION before
/// `CreateCluster` answers `ResourceLimitExceededException`.
///
/// [`MAX_CLUSTERS`] is the published per-Region quota, but the region a record
/// is filed under comes from the caller's own SigV4 credential scope (see
/// [`request_region`]), so the caller decides how many regions exist and a
/// per-region cap alone does not bound the store. This ceiling does: it is the
/// per-region quota times a small number of regions — enough for any realistic
/// multi-region use, and a hard stop on how much a repeated create can retain.
pub const MAX_STORED_CLUSTERS: usize = MAX_CLUSTERS * 10;

/// The number of tags one resource may carry.
///
/// The model bounds a single request's map at 1..50 (`TagMap`), and the same
/// 50-tag ceiling applies to the resource's ACCUMULATED map — so a merge that
/// would push a record past it is refused rather than letting repeated
/// `TagResource` calls grow the record without limit.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The `CancellationStatus` member a cancellation this emulator performed
/// carries. The model's enum is `InProgress | Failed | Successful`
/// (`smithy_query query="shape" shape="CancellationStatus"`); the other two
/// describe a cancellation still running or one that could not stop the
/// rollback, and this emulator stops the rollback within the call, so the
/// recorded outcome is always the terminal success.
pub const CANCELLATION_STATUS_SUCCESSFUL: &str = "Successful";

/// The one `UpdateType` whose cancellation EKS performs: "Currently, this is
/// supported for `VersionRollback` update types on EKS Auto Mode clusters when
/// nodes are rolling back" (`CancelUpdate`). The full type catalog is the
/// model's `UpdateType` enum (`smithy_query query="shape" shape="UpdateType"`).
pub const UPDATE_TYPE_VERSION_ROLLBACK: &str = "VersionRollback";

/// How long a `VersionRollback` update spends rolling nodes back — the phase in
/// which `CancelUpdate` can still stop it — before the control plane rollback
/// begins.
///
/// `CancelUpdate` documents both ends of this window: cancellation "is
/// supported for VersionRollback update types on EKS Auto Mode clusters when
/// nodes are rolling back", and "if the control plane rollback has already
/// begun, the cancellation request fails". AWS publishes no length for the node
/// phase, so the span is CHOSEN rather than cited: a rollback rolls its nodes
/// back first and its control plane after, so this is the first HALF of
/// [`UPDATE_COMPLETION_DELAY`] — non-zero, so a caller that cancels straight
/// after the rollback starts still lands inside the window, and closed well
/// before the update completes. It is derived from the update's stored
/// `createdAt`; nothing sleeps for it.
pub const VERSION_ROLLBACK_NODE_PHASE: Duration = Duration::from_millis(250);

/// The number of clusters `ListClusters` returns when the caller gives no
/// `maxResults`. The model bounds `maxResults` at 1..100, so the full page is the
/// natural default. `ListCapabilities` bounds `maxResults` the same way and shares
/// this default.
pub const DEFAULT_LIST_PAGE_SIZE: i32 = 100;

/// How long a newly created node group reports `CREATING` before the clock owes
/// it `ACTIVE`.
///
/// `CreateNodegroup` publishes no duration for the transition, so this span is
/// CHOSEN rather than cited, on the same grounds as
/// [`CLUSTER_ACTIVATION_DELAY`]: a caller that describes the node group straight
/// after creating it still observes `CREATING`. The due instant is stored on the
/// record; nothing sleeps for it.
pub const NODEGROUP_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// The instance type a node group gets when neither `instanceTypes` nor a launch
/// template names one: "If you don't specify an instance type in a launch
/// template or for `instanceTypes`, then `t3.medium` is used, by default"
/// (`CreateNodegroupRequest$instanceTypes`).
pub const DEFAULT_INSTANCE_TYPE: &str = "t3.medium";

/// The number of `instanceTypes` one node group may name: "you can specify 0-20
/// instance types for `instanceTypes`" (`CreateNodegroupRequest$instanceTypes`).
pub const MAX_INSTANCE_TYPES: usize = 20;

/// The root device disk size a Linux or Bottlerocket node group gets when
/// `diskSize` is absent: "The default disk size is 20 GiB for Linux and
/// Bottlerocket" (`CreateNodegroupRequest$diskSize`).
pub const DEFAULT_DISK_SIZE_GIB: i32 = 20;

/// The smallest `diskSize` a node group may ask for. "The root device disk size
/// (in GiB) for your node group instances"
/// (`CreateNodegroupRequest$diskSize`) — a disk is at least one GiB. The member
/// is a `BoxedInteger` with no modelled range, so `CreateNodegroup` is what
/// refuses a zero or negative size.
pub const MIN_DISK_SIZE_GIB: i32 = 1;

/// The root device disk size a Windows node group gets when `diskSize` is
/// absent: "The default disk size is 50 GiB for Windows"
/// (`CreateNodegroupRequest$diskSize`). Which of the two applies is decided by
/// the node group's `amiType` — see [`default_disk_size_gib`].
pub const DEFAULT_WINDOWS_DISK_SIZE_GIB: i32 = 50;

/// The `amiType` prefix that marks a Windows AMI family. The `AMITypes` enum
/// (`smithy_query query="shape" shape="AMITypes"`) spells every Windows member
/// `WINDOWS_<edition>_<year>_x86_64`, so the family is the prefix rather than a
/// hand-kept list of the six members.
pub const WINDOWS_AMI_TYPE_PREFIX: &str = "WINDOWS_";

/// The `amiType` a node group gets when the request names none and uses no
/// launch template.
///
/// The model documents no default for `CreateNodegroupRequest$amiType`, so this
/// emulator pins one member of the modelled `AMITypes` enum — the current
/// Amazon Linux 2023 x86-64 standard family — instead of inventing a value per
/// request, exactly as [`DEFAULT_KUBERNETES_VERSION`] pins a version.
pub const DEFAULT_AMI_TYPE: &str = "AL2023_x86_64_STANDARD";

/// The `amiType` a node group reports when it was deployed with a launch
/// template and named no `amiType`: "If the node group was deployed using a
/// launch template with a custom AMI, then this is `CUSTOM`"
/// (`Nodegroup$amiType`). This emulator holds no launch templates, so a launch
/// template the caller did not pair with an `amiType` is taken to carry a custom
/// AMI.
pub const CUSTOM_AMI_TYPE: &str = "CUSTOM";

/// The `capacityType` a node group gets when the request names none.
///
/// The model documents no default for `CreateNodegroupRequest$capacityType`, so
/// this emulator pins the on-demand member of the modelled `CapacityTypes` enum
/// (`ON_DEMAND | SPOT | CAPACITY_BLOCK`) rather than leaving the member unset on
/// a resource AWS always reports one for.
pub const DEFAULT_CAPACITY_TYPE: &str = "ON_DEMAND";

/// The `scalingConfig` a node group gets when the request carries none.
///
/// `NodegroupScalingConfig` documents no defaults ("When creating a node group,
/// you must specify all or none of the properties"), so this emulator pins one
/// triple that satisfies `minSize <= desiredSize <= maxSize` instead of
/// inventing one per request.
pub const DEFAULT_SCALING_MIN_SIZE: i32 = 1;
/// See [`DEFAULT_SCALING_MIN_SIZE`].
pub const DEFAULT_SCALING_MAX_SIZE: i32 = 2;
/// See [`DEFAULT_SCALING_MIN_SIZE`].
pub const DEFAULT_SCALING_DESIRED_SIZE: i32 = 2;

/// The `maxUnavailable` a node group's `updateConfig` gets when the request
/// carries none.
///
/// `NodegroupUpdateConfig$maxUnavailable` states the shape of the rule — "This
/// value or `maxUnavailablePercentage` is required to have a value" — but no
/// default, so this emulator pins the most conservative count the member allows
/// (its modelled range is 1..): one node at a time.
pub const DEFAULT_MAX_UNAVAILABLE: i32 = 1;

/// The number of managed node groups one cluster holds before `CreateNodegroup`
/// answers `ResourceLimitExceededException`.
///
/// AWS publishes an adjustable EKS service quota of 30 managed node groups per
/// cluster, and exceeding a service limit is exactly what that error reports.
pub const MAX_NODEGROUPS_PER_CLUSTER: usize = 30;

/// The number of node groups this store holds ACROSS EVERY CLUSTER AND REGION
/// before `CreateNodegroup` answers `ResourceLimitExceededException`.
///
/// [`MAX_NODEGROUPS_PER_CLUSTER`] is the published per-cluster quota, but the
/// caller decides how many clusters and regions exist, so a per-cluster cap
/// alone does not bound the store. This ceiling does: it is the per-cluster
/// quota times a realistic number of clusters, and a hard stop on how much
/// repeated creates can retain.
pub const MAX_STORED_NODEGROUPS: usize = MAX_NODEGROUPS_PER_CLUSTER * 100;

/// The root device disk size a node group with `ami_type` gets when `diskSize`
/// is absent — [`DEFAULT_WINDOWS_DISK_SIZE_GIB`] for the Windows AMI families,
/// [`DEFAULT_DISK_SIZE_GIB`] for every other member.
pub fn default_disk_size_gib(ami_type: &str) -> i32 {
    if ami_type.starts_with(WINDOWS_AMI_TYPE_PREFIX) {
        DEFAULT_WINDOWS_DISK_SIZE_GIB
    } else {
        DEFAULT_DISK_SIZE_GIB
    }
}

/// The number of capabilities one cluster can hold.
///
/// It is not a separate quota: `CreateCapability` documents "You can only create one
/// Capability of each type on a given Amazon EKS cluster", so the per-type
/// uniqueness rule the handler enforces caps a cluster at one capability per
/// `CapabilityType`. Named here so the bound on the store is stated where the store
/// is, and so a test can assert it: with clusters bounded by
/// [`MAX_STORED_CLUSTERS`], the capability map is bounded too.
pub const MAX_CAPABILITIES_PER_CLUSTER: usize = crate::ops::types::CAPABILITY_TYPE_VALUES.len();

/// The message the access-entry operations report when the cluster or the access
/// entry a request names does not exist.
///
/// The access-entry APIs report a GENERIC not-found sentence rather than naming
/// the resource the way the older cluster APIs do (`DescribeCluster` answers "No
/// cluster found for name: <name>."), so this slice uses one constant across
/// `CreateAccessEntry`, `DescribeAccessEntry`, `AssociateAccessPolicy` and
/// `ListAssociatedAccessPolicies` instead of a sentence built per operation. The
/// model documents no message text for `ResourceNotFoundException`, so a client
/// branches on the error CODE (and on the `clusterName` the exception carries),
/// never on this string.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "The requested resource does not exist.";

/// The `type` an access entry gets when `CreateAccessEntry` is called without one:
/// "If you don't specify a value, Amazon EKS sets the value to `STANDARD`"
/// (`CreateAccessEntryRequest$type`).
pub const ACCESS_ENTRY_TYPE_STANDARD: &str = "STANDARD";
/// The access-entry type for a self-managed Linux Amazon EC2 node role.
pub const ACCESS_ENTRY_TYPE_EC2_LINUX: &str = "EC2_LINUX";
/// The access-entry type for a self-managed Windows Amazon EC2 node role.
pub const ACCESS_ENTRY_TYPE_EC2_WINDOWS: &str = "EC2_WINDOWS";
/// The access-entry type for a Fargate profile pod execution role.
pub const ACCESS_ENTRY_TYPE_FARGATE_LINUX: &str = "FARGATE_LINUX";

/// Every access-entry `type` this service accepts.
///
/// `AccessEntry$type` is a plain string member, so the model refuses nothing before
/// a handler runs; the set is the one `CreateAccessEntryRequest$type` documents:
/// "Valid values are `STANDARD`, `FARGATE_LINUX`, `EC2_LINUX`, `EC2_WINDOWS`,
/// `EC2` (for EKS Auto Mode), `HYBRID_LINUX`, and `HYPERPOD_LINUX`."
pub const ACCESS_ENTRY_TYPES: &[&str] = &[
    ACCESS_ENTRY_TYPE_STANDARD,
    ACCESS_ENTRY_TYPE_FARGATE_LINUX,
    ACCESS_ENTRY_TYPE_EC2_LINUX,
    ACCESS_ENTRY_TYPE_EC2_WINDOWS,
    "EC2",
    "HYBRID_LINUX",
    "HYPERPOD_LINUX",
];

/// Whether an access entry of this `type` is a NODE entry, which can only name an
/// IAM role: "For `STANDARD` access entries, you can use every IAM principal type.
/// For nodes (`EC2` (for EKS Auto Mode), `EC2_LINUX`, `EC2_WINDOWS`,
/// `FARGATE_LINUX`, and `HYBRID_LINUX`), the only valid ARN is IAM roles"
/// (`CreateAccessEntryRequest$principalArn`) — which is the list below.
pub const NODE_ACCESS_ENTRY_TYPES: &[&str] = &[
    "EC2",
    ACCESS_ENTRY_TYPE_EC2_LINUX,
    ACCESS_ENTRY_TYPE_EC2_WINDOWS,
    ACCESS_ENTRY_TYPE_FARGATE_LINUX,
    "HYBRID_LINUX",
];

/// See [`NODE_ACCESS_ENTRY_TYPES`].
pub fn is_node_access_entry_type(entry_type: &str) -> bool {
    NODE_ACCESS_ENTRY_TYPES.contains(&entry_type)
}

/// Whether an access entry of this `type` is a self-managed Amazon EC2 node entry,
/// which carries neither Kubernetes groups nor access policies: "If you set the
/// value to `EC2_LINUX` or `EC2_WINDOWS`, you can't specify values for
/// `kubernetesGroups`, or associate an `AccessPolicy` to the access entry"
/// (`CreateAccessEntryRequest$type`).
pub fn is_self_managed_node_entry_type(entry_type: &str) -> bool {
    entry_type == ACCESS_ENTRY_TYPE_EC2_LINUX || entry_type == ACCESS_ENTRY_TYPE_EC2_WINDOWS
}

/// The two `AccessScopeType` members (`smithy_query query="shape"
/// shape="AccessScopeType"`): a policy is scoped to the whole cluster or to named
/// Kubernetes namespaces. `AccessScope$type` is carried as the wire token.
pub const ACCESS_SCOPE_TYPE_CLUSTER: &str = "cluster";
/// See [`ACCESS_SCOPE_TYPE_CLUSTER`].
pub const ACCESS_SCOPE_TYPE_NAMESPACE: &str = "namespace";

/// The username EKS assigns a self-managed Amazon EC2 node access entry when the
/// caller specifies none. The model points at "Creating access entries" in the
/// Amazon EKS User Guide for "the value Amazon EKS specifies for you"
/// (`CreateAccessEntryRequest$username`); that is the `system:node:` username
/// Kubernetes resolves per node from its private DNS name.
pub const DEFAULT_USERNAME_EC2_NODE: &str = "system:node:{{EC2PrivateDNSName}}";
/// The username EKS assigns a Fargate access entry when the caller specifies none
/// — see [`DEFAULT_USERNAME_EC2_NODE`]; a Fargate pod is identified by its
/// session name rather than by a DNS name.
pub const DEFAULT_USERNAME_FARGATE: &str = "system:node:{{SessionName}}";

/// The username an access entry reports when `CreateAccessEntry` is called without
/// one.
///
/// Every type other than the node types authenticates as the IAM principal itself,
/// so the principal ARN is the username; the node types get the `system:node:`
/// username for their platform (see [`DEFAULT_USERNAME_EC2_NODE`]).
///
/// Only the types whose `system:node:` username the user guide publishes are
/// templated here — the self-managed Amazon EC2 ones and Fargate. `EC2` (EKS Auto
/// Mode), `HYBRID_LINUX` and `HYPERPOD_LINUX` are node types for the purpose of
/// [`is_node_access_entry_type`] (their principal must be a role) but no username
/// template is published for them, so rather than invent one this reports the
/// principal ARN, which is a username the principal really authenticates as.
pub fn default_username(entry_type: &str, principal_arn: &str) -> String {
    if is_self_managed_node_entry_type(entry_type) {
        DEFAULT_USERNAME_EC2_NODE.to_string()
    } else if entry_type == ACCESS_ENTRY_TYPE_FARGATE_LINUX {
        DEFAULT_USERNAME_FARGATE.to_string()
    } else {
        principal_arn.to_string()
    }
}

/// The name of the EKS-managed access policy `arn` identifies, or `None` when
/// `arn` is not the ARN of a policy this service publishes.
///
/// `AssociateAccessPolicyRequest$policyArn` is "The ARN of the AccessPolicy that
/// you're associating. For a list of ARNs, use ListAccessPolicies" — so the ARN
/// must name a policy from [`ACCESS_POLICY_NAMES`], the catalogue
/// `ListAccessPolicies` reports, which is this service's single source of truth
/// for which policies exist. The returned name is the catalogue's own `&'static`
/// entry, not a slice of the caller's string.
pub fn access_policy_name(arn: &str) -> Option<&'static str> {
    let name = arn.strip_prefix(ACCESS_POLICY_ARN_PREFIX)?;
    ACCESS_POLICY_NAMES
        .iter()
        .copied()
        .find(|published| *published == name)
}

/// The number of digits an AWS account id carries, which an IAM principal ARN's
/// account field must be ([`::engine::emulator_account_id`] is one such id).
pub const ACCOUNT_ID_DIGITS: usize = 12;

/// The parts of an IAM principal ARN an access entry is keyed and named by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IamPrincipalArn<'a> {
    /// The resource type: `role` or `user`.
    pub kind: &'a str,
    /// The [`ACCOUNT_ID_DIGITS`]-digit account the principal belongs to.
    pub account: &'a str,
    /// The principal's path and name, as the ARN spells it.
    pub name: &'a str,
}

/// The IAM principal `arn` names, or `None` when it is not an IAM user or role ARN.
///
/// `CreateAccessEntryRequest$principalArn` is "The ARN of the IAM principal for the
/// access entry", and an IAM principal ARN is
/// `arn:<partition>:iam::<account-id>:(role|user)/<name>` — global (no region) and
/// account-scoped, the account being the [`ACCOUNT_ID_DIGITS`]-digit id an AWS
/// account carries. Anything else (an assumed-role or service ARN, a bare name) is
/// not a principal this service can file an access entry under.
pub fn parse_iam_principal_arn(arn: &str) -> Option<IamPrincipalArn<'_>> {
    let mut fields = arn.splitn(6, ':');
    let (Some("arn"), Some(partition), Some("iam"), Some(""), Some(account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    if partition.is_empty()
        || account.len() != ACCOUNT_ID_DIGITS
        || !account.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let (kind, name) = resource.split_once('/')?;
    ((kind == "role" || kind == "user") && !name.is_empty()).then_some(IamPrincipalArn {
        kind,
        account,
        name,
    })
}

/// The server-generated identifier that ends an access entry's ARN, in the
/// `8-4-4-4-12` hex form EKS publishes there.
///
/// Derived from the cluster, region and principal the entry belongs to — the
/// triple that identifies it — so it is distinct per entry and stable for the
/// record's whole life. It is computed once, when the entry is created, and stored
/// on the record; no read recomputes it.
pub fn derive_access_entry_id(seed: &str) -> String {
    derive_uuid_from_seed(seed)
}

/// A UUID-shaped identifier derived from `seed`: the first 16 bytes of its
/// SHA-256, rendered by [`hyphenate_uuid`].
///
/// The ONE derivation for every identifier of this service that is UUID-shaped
/// AND derived from what identifies its resource (an access entry's triple, an
/// add-on's key), so those identifiers cannot drift apart in how they are built.
/// An identifier that must be fresh per instance is minted instead (see
/// [`new_update_id`]).
fn derive_uuid_from_seed(seed: &str) -> String {
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&Sha256::digest(seed.as_bytes())[..16]);
    // The same `8-4-4-4-12` renderer the minted ids use, so every UUID-shaped
    // identifier this service publishes is laid out in one place.
    hyphenate_uuid(&bytes)
}

/// The ARN of the access entry for `principal` on `cluster_name` in `region`, in
/// the form EKS publishes:
/// `arn:aws:eks:<region>:<account>:access-entry/<cluster>/<kind>/<principal
/// account>/<principal name>/<id>`.
pub fn access_entry_arn(
    region: &str,
    cluster_name: &str,
    principal: &IamPrincipalArn<'_>,
    principal_arn: &str,
) -> String {
    let id = derive_access_entry_id(&format!("{region}/{cluster_name}/{principal_arn}"));
    ::engine::emulator_arn(
        "eks",
        region,
        &format!(
            "access-entry/{cluster_name}/{}/{}/{}/{id}",
            principal.kind, principal.account, principal.name
        ),
    )
}

/// The number of access entries one cluster holds before `CreateAccessEntry`
/// answers `ResourceLimitExceededException`.
///
/// AWS publishes an adjustable quota for access entries per cluster but this
/// emulator does not pin its value, so the ceiling is CHOSEN rather than cited:
/// what it is for is bounding the store, so a long-lived emulator cannot grow
/// without limit on repeated creates. Exceeding a service limit is exactly what
/// that modelled error reports.
pub const MAX_ACCESS_ENTRIES_PER_CLUSTER: usize = 100;

/// One access policy associated to an access entry, as this service stores it.
#[derive(Debug, Clone)]
pub struct AssociatedPolicyRecord {
    pub policy_arn: String,
    /// The `AccessScopeType` token the association is scoped with
    /// ([`ACCESS_SCOPE_TYPE_CLUSTER`] or [`ACCESS_SCOPE_TYPE_NAMESPACE`]).
    pub scope_type: String,
    /// The namespaces a `namespace`-scoped association names; empty for a
    /// `cluster`-scoped one.
    pub namespaces: Vec<String>,
    pub associated_at: SystemTime,
    pub modified_at: SystemTime,
}

impl AssociatedPolicyRecord {
    /// This record as the modelled `AssociatedAccessPolicy` shape.
    pub fn to_associated_access_policy(&self) -> AssociatedAccessPolicy {
        AssociatedAccessPolicy {
            policy_arn: Some(self.policy_arn.clone()),
            access_scope: Some(AccessScope {
                r#type: Some(self.scope_type.clone()),
                // A `cluster`-scoped association has no namespaces, and the member
                // is optional, so it is omitted rather than reported empty.
                namespaces: (!self.namespaces.is_empty()).then(|| self.namespaces.clone()),
            }),
            associated_at: Some(::aws_smithy_types::DateTime::from(self.associated_at)),
            modified_at: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
        }
    }
}

/// One access entry, as this service stores it.
///
/// Everything an `AccessEntry` reports is held here — including the
/// server-generated parts (`accessEntryArn`, `createdAt`, `modifiedAt`, and the
/// `username` EKS fills in) — so a read returns exactly what the write stored
/// rather than recomputing anything per request.
#[derive(Debug, Clone)]
pub struct AccessEntryRecord {
    pub cluster_name: String,
    pub principal_arn: String,
    pub arn: String,
    pub created_at: SystemTime,
    pub modified_at: SystemTime,
    pub kubernetes_groups: Vec<String>,
    pub username: String,
    pub entry_type: String,
    pub tags: BTreeMap<String, String>,
    pub client_request_token: Option<String>,
    /// The access policies associated to this entry, by policy ARN. A `BTreeMap`
    /// so `ListAssociatedAccessPolicies` walks a stable, sorted order and a page
    /// token can name the policy ARN to resume after. What BOUNDS it is
    /// [`ACCESS_POLICY_NAMES`]: only a policy Amazon EKS publishes can be
    /// associated and each is keyed once, so an entry holds at most one record per
    /// published policy however often `AssociateAccessPolicy` is called.
    pub associated_policies: BTreeMap<String, AssociatedPolicyRecord>,
}

impl AccessEntryRecord {
    /// This record as the modelled `AccessEntry` shape.
    pub fn to_access_entry(&self) -> AccessEntry {
        AccessEntry {
            cluster_name: Some(self.cluster_name.clone()),
            principal_arn: Some(self.principal_arn.clone()),
            kubernetes_groups: Some(self.kubernetes_groups.clone()),
            access_entry_arn: Some(self.arn.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            modified_at: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            tags: Some(
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            username: Some(self.username.clone()),
            r#type: Some(self.entry_type.clone()),
        }
    }
}

/// The one value `ListClusters`' `include` takes: "Use 'all' to return connected
/// clusters, or blank to return only Amazon EKS clusters. 'all' must be in
/// lowercase otherwise an error occurs" (`ListClustersRequest$include`).
pub const LIST_INCLUDE_ALL: &str = "all";

/// The only `identityProviderConfig.type` this API defines: "The type of the
/// identity provider configuration. The only type available is `oidc`"
/// (`IdentityProviderConfig$type`). The comparison against it is
/// case-insensitive, because the model gives the member no enum and the
/// documented token is lower-case.
pub const IDENTITY_PROVIDER_CONFIG_TYPE_OIDC: &str = "oidc";

/// The `configStatus` members an identity provider configuration reports
/// (`smithy_query query="shape" shape="configStatus"` is
/// `CREATING | DELETING | ACTIVE`); `OidcIdentityProviderConfig$status` is a
/// plain string member, so the status is stored as the wire token.
pub const CONFIG_STATUS_CREATING: &str = "CREATING";
/// See [`CONFIG_STATUS_CREATING`].
pub const CONFIG_STATUS_ACTIVE: &str = "ACTIVE";

/// The `UpdateType` an `AssociateIdentityProviderConfig` call reports on the
/// `Update` it answers with (`smithy_query query="shape" shape="UpdateType"`).
pub const UPDATE_TYPE_ASSOCIATE_IDENTITY_PROVIDER_CONFIG: &str = "AssociateIdentityProviderConfig";

/// The `UpdateParamType` key an identity-provider association reports the
/// configuration it associates under (`smithy_query query="shape"
/// shape="UpdateParamType"`).
pub const UPDATE_PARAM_IDENTITY_PROVIDER_CONFIG: &str = "IdentityProviderConfig";

/// The number of identity provider configurations one cluster holds before
/// `AssociateIdentityProviderConfig` refuses another.
///
/// No quota is published for this resource, so the ceiling is CHOSEN: it bounds
/// the store, so repeated associates on one cluster cannot grow it without
/// limit, and it is far above the one OIDC provider a cluster realistically
/// uses. The service-limit error is not modelled on this operation (its errors
/// are `Client`, `InvalidParameter`, `InvalidRequest`, `ResourceInUse`,
/// `ResourceNotFound`, `Server` and `Throttling`), so exceeding it is reported
/// as `InvalidRequestException` — the request is invalid given the state of the
/// cluster.
pub const MAX_IDENTITY_PROVIDER_CONFIGS_PER_CLUSTER: usize = 10;

/// One OIDC identity provider configuration, as this service stores it.
///
/// It holds the caller's input and the server-generated parts (`arn`, `status`
/// and the id of the update the association was reported through), so
/// `DescribeIdentityProviderConfig` returns exactly what
/// `AssociateIdentityProviderConfig` wrote.
#[derive(Debug, Clone)]
pub struct IdentityProviderConfigRecord {
    pub name: String,
    pub arn: String,
    pub issuer_url: String,
    pub client_id: String,
    pub username_claim: Option<String>,
    pub username_prefix: Option<String>,
    pub groups_claim: Option<String>,
    pub groups_prefix: Option<String>,
    pub required_claims: BTreeMap<String, String>,
    pub tags: BTreeMap<String, String>,
    pub status: String,
    /// When set, the instant at which the clock owes this configuration the move
    /// from `CREATING` to `ACTIVE` — the same instant its association update is
    /// owed `Successful`, because that update IS this association. Cleared once
    /// the move is materialised.
    pub active_at: Option<SystemTime>,
    /// The id of the [`UpdateRecord`] in [`ClusterRecord::updates`] that reported
    /// this association, so the configuration and the update a caller tracks it
    /// through are one resource, not two copies.
    pub update_id: String,
}

impl IdentityProviderConfigRecord {
    /// Materialise the status move the clock already owes at `now`.
    ///
    /// Only the configuration's own status is moved here; the update that
    /// reported the association is moved by [`ClusterRecord`]'s own loop over
    /// [`ClusterRecord::updates`], which is owed at the same instant.
    pub fn materialize_due_transitions(&mut self, now: SystemTime) {
        if self.active_at.is_some_and(|due| now >= due) {
            self.status = CONFIG_STATUS_ACTIVE.to_string();
            self.active_at = None;
        }
    }

    /// This record as the modelled `OidcIdentityProviderConfig` shape, for the
    /// cluster it is associated to.
    pub fn to_oidc_config(&self, cluster_name: &str) -> OidcIdentityProviderConfig {
        OidcIdentityProviderConfig {
            identity_provider_config_name: Some(self.name.clone()),
            identity_provider_config_arn: Some(self.arn.clone()),
            cluster_name: Some(cluster_name.to_string()),
            issuer_url: Some(self.issuer_url.clone()),
            client_id: Some(self.client_id.clone()),
            username_claim: self.username_claim.clone(),
            username_prefix: self.username_prefix.clone(),
            groups_claim: self.groups_claim.clone(),
            groups_prefix: self.groups_prefix.clone(),
            required_claims: (!self.required_claims.is_empty()).then(|| {
                self.required_claims
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            }),
            // `TagMap` is bounded 1..50, so an empty map is not a legal value
            // for the member: a configuration with no tags reports no map.
            tags: (!self.tags.is_empty()).then(|| {
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            }),
            status: Some(self.status.clone()),
        }
    }
}

/// The access policies Amazon EKS publishes, sorted by name.
///
/// These are NOT per-account resources: "An access policy includes permissions
/// that allow Amazon EKS to authorize an IAM principal to work with Kubernetes
/// objects on your cluster. The policies are managed by Amazon EKS, but they're
/// not IAM policies" (`AccessPolicy`). No API creates, updates or deletes one, so
/// the catalogue is identical for every account and Region and is held here, as
/// the service's own data, rather than in the per-account store — and it is the
/// single source of truth for every operation that names a policy
/// (`ListAccessPolicies` reports it; `AssociateAccessPolicy` and
/// `DisassociateAccessPolicy`, still scaffolded, will check a `policyArn` against
/// it).
///
/// Source for the rows: the EKS "Access policy permissions" reference, whose table
/// of Amazon EKS access policies lists the whole managed catalogue — the four
/// policies whose permissions mirror the Kubernetes `cluster-admin`, `admin`,
/// `edit` and `view` cluster roles that `AccessPolicy`'s own documentation points
/// at, the read-only mirror of `admin`, and the policies Amazon EKS publishes for
/// the controllers and Amazon Web Services services that work on a cluster (EKS
/// Auto Mode's node lifecycle, the EKS compute, block-storage, load-balancing and
/// networking controllers, hybrid nodes, the AWS Controllers for Kubernetes, Argo
/// CD, AWS Batch, the AI operations and DevOps agents and Application Recovery
/// Controller's region switch). The catalogue is what a caller discovers through
/// `ListAccessPolicies` INSTEAD of hardcoding ARNs, so a policy missing here is a
/// policy no caller can associate.
///
/// Sorted by name, so a page token can name the entry to resume after; the unit
/// tests at the bottom of this module pin the order and the uniqueness.
pub const ACCESS_POLICY_NAMES: &[&str] = &[
    // The rows are in ONE sorted sequence (byte order by name, which is the order
    // the listing walks and pages them in), so the groups the comments name are
    // interleaved rather than listed one after another.
    //
    // Policies an Amazon Web Services service's own controller is granted on a
    // cluster it works with: the DevOps and AI operations agents, Application
    // Recovery Controller's region-switch scaling, and AWS Batch's cluster- and
    // namespace-scoped access.
    "AIDevOpsAgentAccessPolicy",
    "AWSBatchClusterPolicy",
    "AWSBatchNamespacePolicy",
    "AmazonAIOpsAssistantPolicy",
    "AmazonARCRegionSwitchScalingPolicy",
    // The AWS Controllers for Kubernetes.
    "AmazonEKSACKPolicy",
    // `admin`, and the read-only mirror of it.
    "AmazonEKSAdminPolicy",
    "AmazonEKSAdminViewPolicy",
    // Argo CD, cluster- and namespace-scoped.
    "AmazonEKSArgoCDClusterPolicy",
    "AmazonEKSArgoCDPolicy",
    // The Amazon EKS controllers that run against a cluster: EKS Auto Mode's node
    // lifecycle and the block-storage, compute, load-balancing and networking
    // controllers, plus hybrid nodes.
    "AmazonEKSAutoNodePolicy",
    "AmazonEKSBlockStorageClusterPolicy",
    // `cluster-admin`.
    "AmazonEKSClusterAdminPolicy",
    "AmazonEKSComputeClusterPolicy",
    // `edit`.
    "AmazonEKSEditPolicy",
    "AmazonEKSHybridPolicy",
    "AmazonEKSLoadBalancingClusterPolicy",
    "AmazonEKSNetworkingClusterPolicy",
    // `view`.
    "AmazonEKSViewPolicy",
];

/// The prefix every access-policy ARN carries.
///
/// An access policy is owned by the SERVICE, not by the caller's account: its ARN
/// is `arn:aws:eks::aws:cluster-access-policy/<name>` — no Region, and the literal
/// `aws` where an account-owned resource carries an account id. That is why these
/// ARNs are not built through [`::engine::emulator_arn`], which always stamps the
/// emulator's account ([`::engine::emulator_account_id`]) and a Region: an account
/// id in one of these would name a resource no account owns, and the ARN is the
/// value `AssociateAccessPolicy` takes back from a caller.
pub const ACCESS_POLICY_ARN_PREFIX: &str = "arn:aws:eks::aws:cluster-access-policy/";

/// The ARN of the access policy named `name`. See [`ACCESS_POLICY_ARN_PREFIX`].
pub fn access_policy_arn(name: &str) -> String {
    format!("{ACCESS_POLICY_ARN_PREFIX}{name}")
}

/// One cluster, as this service stores it.
///
/// Everything a `Cluster` reports is held here — including the server-generated
/// parts (`arn`, `createdAt`, `endpoint`, `platformVersion`, `identity`,
/// `certificateAuthority`) — so `DescribeCluster` returns exactly what
/// `CreateCluster` wrote rather than recomputing anything per request.
#[derive(Debug, Clone)]
pub struct ClusterRecord {
    pub name: String,
    pub arn: String,
    pub created_at: SystemTime,
    /// Unset on a connected cluster: a registration carries no Kubernetes
    /// version, and the external cluster's own version is only learned once its
    /// connector agent reports in — which nothing in this emulator does.
    pub version: Option<String>,
    /// Unset on a connected cluster: there is no Amazon EKS control plane behind
    /// it to carry a platform version.
    pub platform_version: Option<String>,
    /// Unset on a connected cluster: its API server endpoint is not published by
    /// Amazon EKS.
    pub endpoint: Option<String>,
    /// Unset on a connected cluster: the cluster role is a provisioning input of
    /// `CreateCluster`. A connected cluster's role lives in
    /// [`Self::connector_config`] instead.
    pub role_arn: Option<String>,
    /// Unset on a connected cluster: Amazon EKS provisions no VPC resources for a
    /// cluster it did not create.
    pub resources_vpc_config: Option<VpcConfigResponse>,
    pub kubernetes_network_config: Option<KubernetesNetworkConfigResponse>,
    pub logging: Option<Logging>,
    pub identity: Option<Identity>,
    pub certificate_authority: Option<Certificate>,
    pub status: String,
    /// When set, the instant at which the clock owes this cluster the move from
    /// `CREATING` to `ACTIVE`. Cleared once the move is materialised.
    pub active_at: Option<SystemTime>,
    /// Set only on a connected cluster: what `RegisterCluster` minted for it, and
    /// what `Cluster$connectorConfig` reports. `Some` is what makes a record a
    /// connected cluster rather than an Amazon EKS one (see
    /// [`ClusterRecord::is_connected`]).
    pub connector_config: Option<ConnectorConfigResponse>,
    /// When set, the instant at which the connector activation window closes (see
    /// [`CONNECTOR_ACTIVATION_WINDOW`]) and the clock owes a still-`PENDING`
    /// connected cluster the move to `FAILED`. Cleared once the move is
    /// materialised.
    pub activation_expires_at: Option<SystemTime>,
    pub tags: BTreeMap<String, String>,
    pub encryption_config: Vec<EncryptionConfig>,
    pub access_config: Option<AccessConfigResponse>,
    pub deletion_protection: bool,
    pub client_request_token: Option<String>,
    pub outpost_config: Option<OutpostConfigResponse>,
    pub upgrade_policy: Option<UpgradePolicyResponse>,
    pub zonal_shift_config: Option<ZonalShiftConfigResponse>,
    pub remote_network_config: Option<RemoteNetworkConfigResponse>,
    pub compute_config: Option<ComputeConfigResponse>,
    pub storage_config: Option<StorageConfigResponse>,
    pub control_plane_scaling_config: Option<ControlPlaneScalingConfig>,
    pub kube_api_server_config: Option<KubeApiServerConfigResponse>,
    pub kube_scheduler_config: Option<KubeSchedulerConfigResponse>,
    pub kube_controller_manager_config: Option<KubeControllerManagerConfigResponse>,
    /// This cluster's certificate authorities by id — the one Amazon EKS
    /// provisioned with the cluster plus, during a rotation, the successor a
    /// caller appended. A `BTreeMap` so the ids walk in a stable, sorted order,
    /// and bounded at [`MAX_CERTIFICATE_AUTHORITIES_PER_CLUSTER`], which
    /// `CreateCertificateAuthority` enforces.
    pub certificate_authorities: BTreeMap<String, CertificateAuthorityRecord>,
    /// How many certificate authorities this cluster has ever had, which is what
    /// the next one's id is derived from: a CA removed from the map does not free
    /// its id for reuse.
    pub certificate_authorities_created: u64,
    /// This cluster's access entries, by IAM principal ARN — the principal is what
    /// identifies an access entry within a cluster
    /// (`/clusters/{clusterName}/access-entries/{principalArn}`). They live on the
    /// cluster record because an access entry belongs to exactly one cluster, so
    /// deleting the cluster deletes them with it. A `BTreeMap` so a listing walks a
    /// stable, sorted order. Bounded by [`MAX_ACCESS_ENTRIES_PER_CLUSTER`], which
    /// `CreateAccessEntry` enforces.
    pub access_entries: BTreeMap<String, AccessEntryRecord>,
    /// The identity provider configurations associated to this cluster, by
    /// `identityProviderConfigName`.
    ///
    /// A configuration is addressed only through its cluster
    /// (`/clusters/{clusterName}/identity-provider-configs/...`) and its name is
    /// unique within that cluster, so the cluster's own record is where it
    /// lives: nothing else keeps a second copy, and deleting the cluster takes
    /// its configurations with it. A `BTreeMap` so a later listing walks a
    /// stable, sorted order. Bounded by
    /// [`MAX_IDENTITY_PROVIDER_CONFIGS_PER_CLUSTER`], which
    /// `AssociateIdentityProviderConfig` enforces.
    pub identity_provider_configs: BTreeMap<String, IdentityProviderConfigRecord>,
    /// The asynchronous updates started against this cluster, by update id.
    ///
    /// They live on the cluster because that is what scopes them: `DescribeUpdate`
    /// and `ListUpdates` both name a cluster, and deleting the cluster must drop
    /// its updates. A `BTreeMap` so a listing walks a stable, sorted order. Each
    /// operation that mints an update refuses to start a second one while one is
    /// in flight, `AssociateEncryptionConfig` is refused once encryption is
    /// enabled, `CreateCertificateAuthority` once the cluster holds
    /// [`MAX_CERTIFICATE_AUTHORITIES_PER_CLUSTER`] certificate authorities, and
    /// `AssociateIdentityProviderConfig` mints at most one update per
    /// configuration it associates — so this map holds a bounded number of
    /// entries per cluster and
    /// [`MAX_STORED_CLUSTERS`] bounds it.
    pub updates: BTreeMap<String, UpdateRecord>,
}

/// One asynchronous update of a cluster, as this service stores it.
///
/// Everything the modelled `Update` reports is held here, so `DescribeUpdate`
/// returns what the call that started the update wrote, and what a later
/// `CancelUpdate` recorded on it, rather than recomputing either. Three fields
/// are not reported: `succeeds_at` is the instant the clock owes this update its
/// `Successful` status, `pending_encryption_config` is the change applied to the
/// cluster at that same instant, and `cancellation_request_token` is what makes
/// a retried cancel request idempotent.
#[derive(Debug, Clone)]
pub struct UpdateRecord {
    pub id: String,
    pub r#type: String,
    pub status: String,
    pub created_at: SystemTime,
    pub params: Vec<UpdateParam>,
    pub client_request_token: Option<String>,
    /// When set, the instant at which the clock owes this update the move from
    /// `InProgress` to `Successful`. Cleared once the move is materialised.
    pub succeeds_at: Option<SystemTime>,
    /// The `encryptionConfig` this update puts on its cluster when it completes,
    /// for an `AssociateEncryptionConfig` update. Taken when applied.
    pub pending_encryption_config: Option<Vec<EncryptionConfig>>,
    /// "The latest cancellation information for the update. This field is
    /// present only if any cancellation is attempted for the update"
    /// (`Update$cancellation`) — written by `CancelUpdate`.
    pub cancellation: Option<Cancellation>,
    /// The `clientRequestToken` of the cancel request that produced
    /// [`Self::cancellation`], so a retry of THAT request — which an SDK sends
    /// with the same idempotency token — is answered with the cancellation it
    /// already performed instead of being refused as a second attempt. It is
    /// separate from [`Self::client_request_token`], which belongs to the call
    /// that STARTED the update.
    pub cancellation_request_token: Option<String>,
}

impl UpdateRecord {
    /// Whether this update is still in flight, i.e. in the one non-terminal
    /// `UpdateStatus`.
    pub fn is_in_progress(&self) -> bool {
        self.status == UPDATE_IN_PROGRESS
    }

    /// The instant at which a `VersionRollback` update's control plane rollback
    /// begins, after which a cancellation can no longer stop it.
    ///
    /// Derived from the stored `created_at`, so every caller computes the same
    /// instant and no read has to write anything to advance the phase. `None`
    /// only on a clock so far past the representable range that the phase can
    /// no longer be placed, which a caller reads as "already begun".
    pub fn control_plane_phase_at(&self) -> Option<SystemTime> {
        self.created_at.checked_add(VERSION_ROLLBACK_NODE_PHASE)
    }

    /// This record as the modelled `Update` shape.
    pub fn to_update(&self) -> Update {
        Update {
            id: Some(self.id.clone()),
            status: Some(self.status.clone()),
            r#type: Some(self.r#type.clone()),
            params: Some(self.params.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            // `errors` carries "any errors associated with a `Failed` update",
            // and no slice can fail an update yet (each operation that could is
            // still 501 with its own gap issue). `cancellation` "is present only
            // if any cancellation is attempted", so it is reported exactly when
            // `CancelUpdate` has attempted one.
            errors: None,
            cancellation: self.cancellation.clone(),
        }
    }
}

impl ClusterRecord {
    /// Materialise the status moves the clock already owes at `now`.
    ///
    /// This is the only write a reporting operation may make: each transition was
    /// due at its stored instant whether or not anybody called, so applying it on
    /// read does not change what a later caller sees. Four are owed here — a new
    /// cluster's `CREATING` -> `ACTIVE` move; a connected cluster's lapse to
    /// `FAILED` when its activation window closed; the `CREATING` -> `ACTIVE` move
    /// of every identity provider configuration hanging off this record; and the
    /// completion of an asynchronous update, which flips the update to
    /// `Successful`, applies the change it carries and hands the cluster back to
    /// `ACTIVE`.
    ///
    /// A certificate authority's own clock-owed move is NOT applied here: a CA is
    /// only ever reported by the certificate-authority operations, which
    /// materialise it through
    /// [`CertificateAuthorityRecord::materialize_due_transitions`] on the record
    /// they are about.
    pub fn materialize_due_transitions(&mut self, now: SystemTime) {
        if self.active_at.is_some_and(|due| now >= due) {
            self.status = STATUS_ACTIVE.to_string();
            self.active_at = None;
        }
        // A connected cluster whose activation window closed with no manifest
        // applied is no longer a usable registration, and that move was due at
        // `activation_expires_at` whether or not anybody called. Nothing in this
        // emulator applies a connector manifest, so no call takes a connected
        // cluster out of `PENDING` before that instant; the status is still
        // checked so a record that somehow left `PENDING` is not dragged back.
        if self.activation_expires_at.is_some_and(|due| now >= due) {
            if self.status == STATUS_PENDING {
                self.status = STATUS_FAILED.to_string();
            }
            self.activation_expires_at = None;
        }

        for config in self.identity_provider_configs.values_mut() {
            config.materialize_due_transitions(now);
        }

        let due: Vec<String> = self
            .updates
            .iter()
            .filter(|(_, update)| update.succeeds_at.is_some_and(|due| now >= due))
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            let Some(update) = self.updates.get_mut(&id) else {
                continue;
            };
            update.status = UPDATE_SUCCESSFUL.to_string();
            update.succeeds_at = None;
            let encryption_config = update.pending_encryption_config.take();
            if let Some(encryption_config) = encryption_config {
                self.encryption_config = encryption_config;
            }
            // An update only starts from an `ACTIVE` cluster, so completing one
            // hands the cluster back to the status it was taken from.
            self.status = STATUS_ACTIVE.to_string();
        }
    }

    /// Whether this record is an external cluster `RegisterCluster` connected,
    /// rather than an Amazon EKS cluster `CreateCluster` provisioned.
    pub fn is_connected(&self) -> bool {
        self.connector_config.is_some()
    }

    /// The update this cluster is still running, if any.
    ///
    /// A cluster runs at most one update at a time, so this is what a mutating
    /// operation checks before starting another.
    pub fn in_flight_update(&self) -> Option<&UpdateRecord> {
        self.updates
            .values()
            .find(|update| update.is_in_progress())
    }

    /// The update this cluster already recorded for `client_request_token`, if
    /// any — what makes a retry of the call that started it idempotent.
    pub fn update_for_token(&self, token: &str) -> Option<&UpdateRecord> {
        self.updates
            .values()
            .find(|update| update.client_request_token.as_deref() == Some(token))
    }

    /// Whether this is an EKS Auto Mode cluster, which is what
    /// `computeConfig.enabled` reports: "Request to enable or disable the
    /// compute capability on your EKS Auto Mode cluster"
    /// (`ComputeConfigRequest$enabled`). A cluster created without a
    /// `computeConfig`, or with one that does not enable compute, is not one.
    pub fn is_auto_mode(&self) -> bool {
        self.compute_config
            .as_ref()
            .and_then(|config| config.enabled)
            .unwrap_or(false)
    }

    /// This record as the modelled `Cluster` shape.
    pub fn to_cluster(&self) -> Cluster {
        Cluster {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            version: self.version.clone(),
            endpoint: self.endpoint.clone(),
            role_arn: self.role_arn.clone(),
            resources_vpc_config: self.resources_vpc_config.clone(),
            kubernetes_network_config: self.kubernetes_network_config.clone(),
            logging: self.logging.clone(),
            identity: self.identity.clone(),
            status: Some(self.status.clone()),
            certificate_authority: self.certificate_authority.clone(),
            client_request_token: self.client_request_token.clone(),
            platform_version: self.platform_version.clone(),
            tags: Some(
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            encryption_config: (!self.encryption_config.is_empty())
                .then(|| self.encryption_config.clone()),
            access_config: self.access_config.clone(),
            deletion_protection: Some(self.deletion_protection),
            outpost_config: self.outpost_config.clone(),
            upgrade_policy: self.upgrade_policy.clone(),
            zonal_shift_config: self.zonal_shift_config.clone(),
            remote_network_config: self.remote_network_config.clone(),
            compute_config: self.compute_config.clone(),
            storage_config: self.storage_config.clone(),
            control_plane_scaling_config: self.control_plane_scaling_config.clone(),
            kube_api_server_config: self.kube_api_server_config.clone(),
            kube_scheduler_config: self.kube_scheduler_config.clone(),
            kube_controller_manager_config: self.kube_controller_manager_config.clone(),
            connector_config: self.connector_config.clone(),
            // Left unset by this slice, each with its own gap issue: `id` is the
            // id of a local cluster on an Outpost, and `health` is filled in by
            // cluster insights.
            id: None,
            health: None,
        }
    }
}

/// One managed node group, as this service stores it.
///
/// Everything a `Nodegroup` reports is held here — including the
/// server-generated `nodegroupArn`, `createdAt`/`modifiedAt` and every default
/// `CreateNodegroup` filled in — so `DescribeNodegroup` returns exactly what the
/// create wrote rather than recomputing anything per request.
#[derive(Debug, Clone)]
pub struct NodegroupRecord {
    pub nodegroup_name: String,
    pub cluster_name: String,
    pub arn: String,
    pub created_at: SystemTime,
    pub modified_at: SystemTime,
    pub version: String,
    /// Set only when the caller named a `releaseVersion`: this emulator
    /// publishes no EKS-optimized AMI catalog, so for a node group that named
    /// none it reports no AMI version rather than inventing one.
    pub release_version: Option<String>,
    pub status: String,
    /// When set, the instant at which the clock owes this node group the move
    /// from `CREATING` to `ACTIVE`. Cleared once the move is materialised.
    pub active_at: Option<SystemTime>,
    pub capacity_type: String,
    pub scaling_config: NodegroupScalingConfig,
    /// `None` when the node group was deployed with a launch template: "If the
    /// node group was deployed with a launch template, then this is `null`"
    /// (`Nodegroup$instanceTypes`).
    pub instance_types: Option<Vec<String>>,
    pub subnets: Vec<String>,
    /// `None` when the node group was deployed with a launch template, for the
    /// same reason as [`NodegroupRecord::instance_types`]
    /// (`Nodegroup$remoteAccess`).
    pub remote_access: Option<RemoteAccessConfig>,
    pub ami_type: String,
    pub node_role: String,
    pub labels: BTreeMap<String, String>,
    pub taints: Vec<Taint>,
    /// `None` when the node group was deployed with a launch template, for the
    /// same reason as [`NodegroupRecord::instance_types`] (`Nodegroup$diskSize`).
    pub disk_size: Option<i32>,
    pub update_config: NodegroupUpdateConfig,
    pub node_repair_config: Option<NodeRepairConfig>,
    pub launch_template: Option<LaunchTemplateSpecification>,
    pub tags: BTreeMap<String, String>,
    pub warm_pool_config: Option<WarmPoolConfig>,
    pub client_request_token: Option<String>,
}

impl NodegroupRecord {
    /// Materialise the status move the clock already owes at `now`. See
    /// [`ClusterRecord::materialize_due_transitions`].
    pub fn materialize_due_transitions(&mut self, now: SystemTime) {
        if self.active_at.is_some_and(|due| now >= due) {
            self.status = STATUS_ACTIVE.to_string();
            self.active_at = None;
        }
    }

    /// This record as the modelled `Nodegroup` shape.
    pub fn to_nodegroup(&self) -> Nodegroup {
        Nodegroup {
            nodegroup_name: Some(self.nodegroup_name.clone()),
            nodegroup_arn: Some(self.arn.clone()),
            cluster_name: Some(self.cluster_name.clone()),
            version: Some(self.version.clone()),
            release_version: self.release_version.clone(),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            modified_at: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            status: Some(self.status.clone()),
            capacity_type: Some(self.capacity_type.clone()),
            scaling_config: Some(self.scaling_config.clone()),
            instance_types: self.instance_types.clone(),
            subnets: Some(self.subnets.clone()),
            remote_access: self.remote_access.clone(),
            ami_type: Some(self.ami_type.clone()),
            node_role: Some(self.node_role.clone()),
            labels: Some(
                self.labels
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            taints: Some(self.taints.clone()),
            disk_size: self.disk_size,
            // Nothing in this emulator reports a node group issue, so the
            // health block is present and empty rather than absent: "Any issues
            // that are associated with the node group" (`NodegroupHealth$issues`).
            health: Some(NodegroupHealth {
                issues: Some(Vec::new()),
            }),
            update_config: Some(self.update_config.clone()),
            node_repair_config: self.node_repair_config.clone(),
            launch_template: self.launch_template.clone(),
            tags: Some(
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            warm_pool_config: self.warm_pool_config.clone(),
            // `resources` names the Auto Scaling groups and the remote-access
            // security group EKS creates for the node group. This emulator
            // creates no EC2 resources, so it reports none rather than
            // inventing ids for resources no EC2 store holds — the same choice
            // the cluster slice makes for `clusterSecurityGroupId` and `vpcId`.
            resources: None,
        }
    }
}

/// The `AddonStatus` members this slice uses. The model's enum
/// (`smithy_query query="shape" shape="AddonStatus"`) is
/// `CREATING | ACTIVE | CREATE_FAILED | UPDATING | DELETING | DELETE_FAILED |
/// DEGRADED | UPDATE_FAILED`; `Addon$status` carries that enum, and this slice
/// stores the wire token.
pub const ADDON_STATUS_CREATING: &str = "CREATING";
/// See [`ADDON_STATUS_CREATING`].
///
/// Deliberately its own constant rather than the cluster slice's
/// [`STATUS_ACTIVE`]: `AddonStatus` and `ClusterStatus` are different enums that
/// only happen to share this token (the add-on enum has `DEGRADED` and
/// `UPDATE_FAILED`, the cluster enum has `PENDING`), so a change to one must not
/// silently change the other.
pub const ADDON_STATUS_ACTIVE: &str = "ACTIVE";

/// How long a newly created add-on reports `CREATING` before the clock owes it
/// `ACTIVE`.
///
/// `CreateAddon` publishes no duration for the install, so this span is CHOSEN
/// rather than cited — the same reasoning as [`CLUSTER_ACTIVATION_DELAY`]: long
/// enough that a caller which describes the add-on straight after creating it
/// still observes `CREATING`, short enough to be worth waiting for. The due
/// instant is stored on the record; nothing sleeps for it.
pub const ADDON_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// The `owner` a created add-on reports.
///
/// This emulator installs only Amazon EKS-managed add-ons — it holds no
/// Marketplace subscription and mints no `marketplaceInformation` — and an
/// EKS-managed add-on is owned by `aws` and published by `eks`, which is what
/// `DescribeAddonVersions` reports for them.
pub const ADDON_OWNER_AWS: &str = "aws";
/// See [`ADDON_OWNER_AWS`].
pub const ADDON_PUBLISHER_EKS: &str = "eks";

/// The Kubernetes namespace an add-on is installed into when the request names
/// none.
///
/// `CreateAddon$namespaceConfig` is "The namespace configuration for the addon.
/// If specified, this will override the DEFAULT namespace for the addon", and an
/// Amazon EKS-managed add-on's default namespace is `kube-system` — the
/// namespace the EKS User Guide documents every managed add-on's workloads
/// (`vpc-cni`, `coredns`, `kube-proxy` and the rest) as running in. The resolved
/// namespace is stored on the record, so `Addon$namespaceConfig` ("the Kubernetes
/// namespace where the addon is installed") reports where the add-on actually
/// went, and the EKS Pod Identity associations the add-on owns are created for
/// service accounts in that namespace.
pub const ADDON_DEFAULT_NAMESPACE: &str = "kube-system";

/// The number of add-ons one cluster holds before `CreateAddon` refuses another.
///
/// AWS publishes no "add-ons per cluster" quota, so this ceiling is CHOSEN: it
/// is what bounds the store, since add-on names are caller-supplied and
/// repeated creates would otherwise let one cluster retain an unbounded number
/// of records. It is generous next to the handful of add-ons a real cluster
/// runs, and `MAX_STORED_CLUSTERS` bounds how many clusters can hold that many.
pub const MAX_ADDONS_PER_CLUSTER: usize = 100;

/// One add-on of one cluster, as this service stores it.
///
/// Everything an `Addon` reports is held here — including the server-generated
/// parts (`addonArn`, `createdAt`, `modifiedAt`, the assigned version and the
/// ARNs of the EKS Pod Identity associations the add-on owns) — so a later read
/// returns exactly what `CreateAddon` wrote rather than recomputing anything.
#[derive(Debug, Clone)]
pub struct AddonRecord {
    pub cluster_name: String,
    pub addon_name: String,
    pub arn: String,
    pub addon_version: String,
    pub status: String,
    pub created_at: SystemTime,
    pub modified_at: SystemTime,
    /// When set, the instant at which the clock owes this add-on the move from
    /// `CREATING` to `ACTIVE`. Cleared once the move is materialised.
    pub active_at: Option<SystemTime>,
    pub service_account_role_arn: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub configuration_values: Option<String>,
    /// The ARNs of the EKS Pod Identity associations this add-on owns — the
    /// associations `CreateAddon` created for the `podIdentityAssociations` the
    /// request asked for, each one a record of its own in
    /// [`ComAmazonawsEksAwswesleyfrontendState::pod_identity_associations`]
    /// carrying this add-on's ARN as its `ownerArn`.
    pub pod_identity_association_arns: Vec<String>,
    /// The Kubernetes namespace this add-on is installed into: the one the
    /// request's `namespaceConfig` named, or [`ADDON_DEFAULT_NAMESPACE`].
    pub namespace: String,
    pub publisher: String,
    pub owner: String,
    pub client_request_token: Option<String>,
}

impl AddonRecord {
    /// Materialise the status move the clock already owes at `now`.
    ///
    /// As on [`ClusterRecord::materialize_due_transitions`], the move was due at
    /// `active_at` whether or not anybody called, so applying it on read does
    /// not change what a later caller sees.
    pub fn materialize_due_transitions(&mut self, now: SystemTime) {
        if self.active_at.is_some_and(|due| now >= due) {
            self.status = ADDON_STATUS_ACTIVE.to_string();
            self.active_at = None;
        }
    }

    /// This record as the modelled `Addon` shape.
    pub fn to_addon(&self) -> Addon {
        Addon {
            addon_name: Some(self.addon_name.clone()),
            cluster_name: Some(self.cluster_name.clone()),
            status: Some(self.status.clone()),
            addon_version: Some(self.addon_version.clone()),
            // The add-on runs nowhere in this emulator, so nothing can go wrong
            // with it: the health object is reported with an EMPTY issue list,
            // which is what a healthy add-on reports, rather than left unset.
            health: Some(AddonHealth {
                issues: Some(Vec::new()),
            }),
            addon_arn: Some(self.arn.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            modified_at: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            service_account_role_arn: self.service_account_role_arn.clone(),
            tags: Some(
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            publisher: Some(self.publisher.clone()),
            owner: Some(self.owner.clone()),
            configuration_values: self.configuration_values.clone(),
            pod_identity_associations: (!self.pod_identity_association_arns.is_empty())
                .then(|| self.pod_identity_association_arns.clone()),
            namespace_config: Some(AddonNamespaceConfigResponse {
                namespace: Some(self.namespace.clone()),
            }),
            // Left unset: `marketplaceInformation` describes an add-on bought
            // through the Amazon Web Services Marketplace, and this emulator
            // installs only EKS-managed add-ons ([`ADDON_OWNER_AWS`]).
            marketplace_information: None,
        }
    }
}

/// One managed capability of one cluster, as this service stores it.
///
/// Everything a [`Capability`] reports is held here — including the
/// server-generated `arn`, `createdAt` and `modifiedAt` — so `DescribeCapability`
/// and `ListCapabilities` return exactly what `CreateCapability` wrote rather than
/// recomputing anything per request.
#[derive(Debug, Clone)]
pub struct CapabilityRecord {
    pub capability_name: String,
    pub cluster_name: String,
    pub arn: String,
    /// A `CapabilityType` wire token (`ACK | ARGOCD | KRO`).
    pub r#type: String,
    pub role_arn: String,
    /// A [`CAPABILITY_STATUS_CREATING`]-family wire token.
    pub status: String,
    /// When set, the instant at which the clock owes this capability the move from
    /// `CREATING` to `ACTIVE`. Cleared once the move is materialised.
    pub active_at: Option<SystemTime>,
    pub configuration: Option<CapabilityConfigurationResponse>,
    pub tags: BTreeMap<String, String>,
    /// The capability's open health issues. This emulator runs no capability
    /// infrastructure, so nothing adds to it yet and a healthy capability reports
    /// an EMPTY issue list rather than an absent one.
    pub health_issues: Vec<CapabilityIssue>,
    pub created_at: SystemTime,
    pub modified_at: SystemTime,
    /// A `CapabilityDeletePropagationPolicy` wire token (only `RETAIN` is modelled).
    pub delete_propagation_policy: String,
    pub client_request_token: Option<String>,
}

impl CapabilityRecord {
    /// Materialise the status move the clock already owes at `now`.
    ///
    /// The only write a reporting operation may make: the transition was due at
    /// `active_at` whether or not anybody called, so applying it on read does not
    /// change what a later caller sees.
    pub fn materialize_due_transitions(&mut self, now: SystemTime) {
        if self.active_at.is_some_and(|due| now >= due) {
            self.status = CAPABILITY_STATUS_ACTIVE.to_string();
            self.active_at = None;
        }
    }

    /// This record as the modelled `Capability` shape.
    pub fn to_capability(&self) -> Capability {
        Capability {
            capability_name: Some(self.capability_name.clone()),
            arn: Some(self.arn.clone()),
            cluster_name: Some(self.cluster_name.clone()),
            r#type: Some(self.r#type.clone()),
            role_arn: Some(self.role_arn.clone()),
            status: Some(self.status.clone()),
            configuration: self.configuration.clone(),
            tags: Some(
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            health: Some(CapabilityHealth {
                issues: Some(self.health_issues.clone()),
            }),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            modified_at: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            delete_propagation_policy: Some(self.delete_propagation_policy.clone()),
            // `version` is "the version of the capability software that is
            // currently running" — software this emulator does not run and AWS
            // publishes no version list for, so it is left unset rather than
            // reporting a version nothing is running. [`to_summary`] omits it for
            // the same reason.
            version: None,
        }
    }

    /// This record as the modelled `CapabilitySummary` shape `ListCapabilities`
    /// returns: the same identity, type, status and timestamps, without the
    /// configuration, tags and health detail `DescribeCapability` carries.
    pub fn to_summary(&self) -> CapabilitySummary {
        CapabilitySummary {
            capability_name: Some(self.capability_name.clone()),
            arn: Some(self.arn.clone()),
            r#type: Some(self.r#type.clone()),
            status: Some(self.status.clone()),
            version: None,
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            modified_at: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
        }
    }
}

/// The fixed prefix of an EKS Pod Identity association id.
///
/// Every association id AWS publishes — in the `CreatePodIdentityAssociation`
/// response, in `ListPodIdentityAssociations` and inside the association ARN
/// (`.../podidentityassociation/<cluster>/<associationId>`) — has the form `a-`
/// followed by [`ASSOCIATION_ID_BODY_LEN`] lowercase alphanumeric characters
/// (`a-abcdefghijklmnop1`). The model types `associationId` as a plain `String`
/// with no `@pattern`, so nothing refuses a malformed id before a handler runs
/// and each handler that takes one checks the form itself.
pub const ASSOCIATION_ID_PREFIX: &str = "a-";
/// The number of characters after [`ASSOCIATION_ID_PREFIX`] in an association id.
pub const ASSOCIATION_ID_BODY_LEN: usize = 17;
/// The association-id form as a regular expression, for the refusal message a
/// malformed id earns.
pub const ASSOCIATION_ID_PATTERN: &str = "^a-[0-9a-z]{17}$";
/// The total length of an association id: the prefix plus the body.
pub const ASSOCIATION_ID_LEN: usize = ASSOCIATION_ID_PREFIX.len() + ASSOCIATION_ID_BODY_LEN;

/// The refusal message an identifier outside the association-id form earns.
///
/// Built from the member the value arrived in and the constraint it failed, the
/// way every service's parameter complaint is: an id of the wrong LENGTH is told
/// the length an association id has, and one of the right length carrying
/// characters the form does not admit is told the form. Shared by every
/// operation that takes an `associationId`, so all of them phrase the same
/// refusal the same way.
pub fn association_id_refusal(member: &str, value: &str) -> String {
    if value.chars().count() == ASSOCIATION_ID_LEN {
        format!("The parameter {member} should match the pattern {ASSOCIATION_ID_PATTERN}.")
    } else {
        format!("The parameter {member} should be {ASSOCIATION_ID_LEN} characters long.")
    }
}

/// The characters an association id's body is drawn from: lowercase letters and
/// digits, which is what [`ASSOCIATION_ID_PATTERN`] accepts.
const ASSOCIATION_ID_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// Whether `candidate` has the form of an association id
/// ([`ASSOCIATION_ID_PATTERN`]).
///
/// An id outside this form names no association in any account or Region, so a
/// request carrying one is refused as an invalid parameter rather than looked up.
pub fn is_association_id(candidate: &str) -> bool {
    candidate
        .strip_prefix(ASSOCIATION_ID_PREFIX)
        .is_some_and(|body| {
            body.len() == ASSOCIATION_ID_BODY_LEN
                && body
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

/// The cluster-name form, as a regular expression.
///
/// `CreateCluster$name` is `@length(1..100)` + `@pattern` this expression
/// (`smithy_query query="operation" operation="CreateCluster"`), so no cluster in
/// any Region can carry a name outside it. The `clusterName` member of the pod
/// identity operations is a plain `String`, so they check the form themselves.
pub const CLUSTER_NAME_PATTERN: &str = "^[0-9A-Za-z][A-Za-z0-9\\-_]*$";
/// The maximum length of a cluster name (`CreateCluster$name` is `@length(1..100)`).
pub const MAX_CLUSTER_NAME_LEN: usize = 100;

/// Whether `candidate` has the form of a cluster name
/// ([`CLUSTER_NAME_PATTERN`], within [`MAX_CLUSTER_NAME_LEN`]).
pub fn is_cluster_name(candidate: &str) -> bool {
    let mut characters = candidate.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        && characters.all(|rest| rest.is_ascii_alphanumeric() || rest == '-' || rest == '_')
        && candidate.chars().count() <= MAX_CLUSTER_NAME_LEN
}

/// The refusal message a value outside the cluster-name form earns, built from
/// the member it arrived in and the constraint it failed. Shared by every
/// operation that takes a `clusterName` without a modelled constraint on it, so
/// all of them phrase the same refusal the same way.
pub fn cluster_name_refusal(member: &str, value: &str) -> String {
    if value.chars().count() > MAX_CLUSTER_NAME_LEN {
        format!(
            "Value '{value}' at '{member}' failed to satisfy constraint: Member must have \
             length less than or equal to {MAX_CLUSTER_NAME_LEN}"
        )
    } else {
        format!(
            "Value '{value}' at '{member}' failed to satisfy constraint: Member must satisfy \
             regular expression pattern: {CLUSTER_NAME_PATTERN}"
        )
    }
}

/// Whether `candidate` has the form of an IAM role ARN.
///
/// `roleArn` and `targetRoleArn` are documented as "The Amazon Resource Name
/// (ARN) of the IAM role", and an IAM role ARN is
/// `arn:<partition>:iam::<12-digit account>:role/<path and name>` — IAM is a
/// global service, so the Region segment is empty. A value outside that form
/// names no role in any account, so a request carrying one is refused as an
/// invalid parameter. The account is NOT compared against this emulator's own:
/// a role in another account is a well-formed ARN.
pub fn is_iam_role_arn(candidate: &str) -> bool {
    let fields: Vec<&str> = candidate.split(':').collect();
    fields.len() == 6
        && fields[0] == "arn"
        && !fields[1].is_empty()
        && fields[2] == "iam"
        && fields[3].is_empty()
        && fields[4].len() == 12
        && fields[4].bytes().all(|byte| byte.is_ascii_digit())
        && fields[5]
            .strip_prefix("role/")
            .is_some_and(|name| !name.is_empty())
}

/// The maximum length of a Kubernetes name this service accepts: the DNS-1123
/// SUBDOMAIN limit, which bounds the longest Kubernetes object name (a service
/// account name); a namespace, a DNS-1123 label, is shorter still.
pub const MAX_KUBERNETES_NAME_LEN: usize = 253;

/// The Kubernetes name form, as a regular expression, for the refusal message an
/// `namespace` or `serviceAccount` outside it earns.
pub const KUBERNETES_NAME_PATTERN: &str = "^[a-z0-9]([-a-z0-9.]*[a-z0-9])?$";

/// Whether `candidate` has the form of a Kubernetes object name
/// ([`KUBERNETES_NAME_PATTERN`], within [`MAX_KUBERNETES_NAME_LEN`]).
///
/// `namespace` and `serviceAccount` name Kubernetes objects ("The name of the
/// Kubernetes namespace inside the cluster", "The name of the Kubernetes service
/// account inside the cluster"), and Kubernetes names are DNS-1123 names:
/// lowercase alphanumerics, `-` and `.`, starting and ending alphanumeric. A
/// value outside that form can never name an object in any cluster, so a request
/// carrying one is refused as an invalid parameter.
pub fn is_kubernetes_name(candidate: &str) -> bool {
    let bytes = candidate.as_bytes();
    let is_alphanumeric = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    !bytes.is_empty()
        && bytes.len() <= MAX_KUBERNETES_NAME_LEN
        && is_alphanumeric(bytes[0])
        && is_alphanumeric(bytes[bytes.len() - 1])
        && bytes
            .iter()
            .all(|byte| is_alphanumeric(*byte) || *byte == b'-' || *byte == b'.')
}

/// The number of EKS Pod Identity associations one cluster holds before
/// `CreatePodIdentityAssociation` answers `ResourceLimitExceededException`.
///
/// AWS publishes an adjustable quota for EKS Pod Identity associations per
/// cluster but the model does not state its value, so this ceiling is CHOSEN
/// rather than cited: high enough that no realistic use reaches it, and a hard
/// stop on how much one cluster's repeated creates can retain. The store as a
/// whole is bounded by it, because an association is only created against a
/// cluster that exists and clusters are bounded by [`MAX_STORED_CLUSTERS`].
pub const MAX_POD_IDENTITY_ASSOCIATIONS_PER_CLUSTER: usize = 1_000;

/// One EKS Pod Identity association, as this service stores it.
///
/// Everything a `PodIdentityAssociation` reports is held here, including the
/// server-generated parts (`associationId`, `associationArn`, `externalId`,
/// `createdAt`, `modifiedAt`), so a describe answers exactly what the create or
/// the last update wrote rather than recomputing anything per request.
#[derive(Debug, Clone)]
pub struct PodIdentityAssociationRecord {
    pub cluster_name: String,
    pub association_id: String,
    pub association_arn: String,
    pub namespace: String,
    pub service_account: String,
    pub role_arn: String,
    pub target_role_arn: Option<String>,
    /// Minted only for an association that names a `targetRoleArn`: the model
    /// documents `externalId` as the value "you put in the trust policy of the
    /// TARGET role", so an association without one has nothing to condition.
    pub external_id: Option<String>,
    pub policy: Option<String>,
    pub disable_session_tags: Option<bool>,
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    pub modified_at: SystemTime,
    /// The `@idempotencyToken` of the CREATE that minted this association, so a
    /// retry of that create is absorbed rather than answered with a conflict.
    pub client_request_token: Option<String>,
    /// The `@idempotencyToken` of the last UPDATE that applied to this
    /// association, kept apart from the create's own token: a token absorbs a
    /// retry only of the call that sent it, so reusing a create's token on an
    /// update can never swallow the update's change.
    pub update_request_token: Option<String>,
    /// The ARN of the Amazon EKS add-on that owns this association, when an
    /// add-on created it: "If defined, the EKS Pod Identity association is owned
    /// by an Amazon EKS add-on" (`PodIdentityAssociation$ownerArn`). An
    /// association created through `CreatePodIdentityAssociation` is owned by the
    /// caller and carries none.
    pub owner_arn: Option<String>,
}

impl PodIdentityAssociationRecord {
    /// This record as the modelled `PodIdentityAssociation` shape.
    pub fn to_association(&self) -> PodIdentityAssociation {
        PodIdentityAssociation {
            cluster_name: Some(self.cluster_name.clone()),
            namespace: Some(self.namespace.clone()),
            service_account: Some(self.service_account.clone()),
            role_arn: Some(self.role_arn.clone()),
            association_arn: Some(self.association_arn.clone()),
            association_id: Some(self.association_id.clone()),
            tags: Some(
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            modified_at: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            // "If defined, the EKS Pod Identity association is owned by an Amazon
            // EKS add-on" — an association created through this API is owned by
            // the caller, so it reports no owner; one an add-on created carries
            // that add-on's ARN.
            owner_arn: self.owner_arn.clone(),
            disable_session_tags: self.disable_session_tags,
            target_role_arn: self.target_role_arn.clone(),
            external_id: self.external_id.clone(),
            policy: self.policy.clone(),
        }
    }

    /// This record as the modelled `PodIdentityAssociationSummary` shape, which
    /// is what `ListPodIdentityAssociations` reports.
    ///
    /// The summary is the association "simplified by removing these fields
    /// compared to the full `PodIdentityAssociation`: the IAM role `roleArn`,
    /// `createdAt`, `modifiedAt` and `tags`" (`PodIdentityAssociationSummary`) —
    /// so this projects exactly the six members that shape declares and drops
    /// the rest.
    pub fn to_summary(&self) -> PodIdentityAssociationSummary {
        PodIdentityAssociationSummary {
            cluster_name: Some(self.cluster_name.clone()),
            namespace: Some(self.namespace.clone()),
            service_account: Some(self.service_account.clone()),
            association_arn: Some(self.association_arn.clone()),
            association_id: Some(self.association_id.clone()),
            // As on the full association: only an Amazon EKS add-on owns one.
            owner_arn: self.owner_arn.clone(),
        }
    }
}

/// The cluster type the Kubernetes version calendar publishes versions for.
///
/// `DescribeClusterVersionsRequest$clusterType` is a free-form string ("The type
/// of cluster to filter versions by"), and the only kind of cluster this service
/// stands up is an Amazon EKS cluster, which EKS names `eks` — the value its own
/// CLI examples pass to `--cluster-type`.
pub const CLUSTER_TYPE_EKS: &str = "eks";

/// Where a Kubernetes version sits in the Amazon EKS support lifecycle at one
/// instant.
///
/// EKS publishes the same state under two modelled enums
/// (`smithy_query query="shape" shape="ClusterVersionStatus,VersionStatus"`): the
/// deprecated `ClusterVersionStatus` (`unsupported | standard-support |
/// extended-support`) and `VersionStatus` (`UNSUPPORTED | STANDARD_SUPPORT |
/// EXTENDED_SUPPORT`). Both spellings are reported for every version, and both
/// input filters are matched against this one state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportStatus {
    /// The version is inside its standard-support window.
    StandardSupport,
    /// Standard support has ended, extended support has not.
    ExtendedSupport,
    /// Extended support has ended too.
    Unsupported,
}

impl SupportStatus {
    /// This state as a `VersionStatus` member.
    pub fn version_status(self) -> &'static str {
        match self {
            Self::StandardSupport => "STANDARD_SUPPORT",
            Self::ExtendedSupport => "EXTENDED_SUPPORT",
            Self::Unsupported => "UNSUPPORTED",
        }
    }

    /// This state as a (deprecated) `ClusterVersionStatus` member.
    pub fn cluster_version_status(self) -> &'static str {
        match self {
            Self::StandardSupport => "standard-support",
            Self::ExtendedSupport => "extended-support",
            Self::Unsupported => "unsupported",
        }
    }

    /// The state a `VersionStatus` member names, or `None` for a token that enum
    /// does not declare.
    pub fn from_version_status(token: &str) -> Option<Self> {
        [Self::StandardSupport, Self::ExtendedSupport, Self::Unsupported]
            .into_iter()
            .find(|status| status.version_status() == token)
    }

    /// The state a `ClusterVersionStatus` member names, or `None` for a token that
    /// enum does not declare.
    pub fn from_cluster_version_status(token: &str) -> Option<Self> {
        [Self::StandardSupport, Self::ExtendedSupport, Self::Unsupported]
            .into_iter()
            .find(|status| status.cluster_version_status() == token)
    }

    /// Every member of either enum, in the order this type declares them — used to
    /// word the refusal of a token neither enum declares.
    pub fn all() -> [Self; 3] {
        [Self::StandardSupport, Self::ExtendedSupport, Self::Unsupported]
    }
}

/// One row of the Kubernetes version calendar Amazon EKS publishes, as this
/// service stores it.
///
/// The calendar is a PUBLICATION, not per-account data: `DescribeClusterVersions`
/// "Lists available Kubernetes versions for Amazon EKS clusters", so every row is
/// seeded once (see [`VERSION_CALENDAR`]) and no operation writes one. The
/// support status is NOT stored: it is a function of the clock and the two
/// end-of-support dates, so it is derived per request by
/// [`ClusterVersionRecord::support_status`] and cannot go stale.
#[derive(Debug, Clone)]
pub struct ClusterVersionRecord {
    /// The Kubernetes minor version, e.g. `1.31`.
    pub cluster_version: String,
    /// The cluster type this row is for ([`CLUSTER_TYPE_EKS`]).
    pub cluster_type: String,
    /// Whether `CreateCluster` without a `version` creates this version — the row
    /// that matches [`DEFAULT_KUBERNETES_VERSION`].
    pub default_version: bool,
    /// The day Amazon EKS released this version.
    pub release_date: SystemTime,
    /// The day standard support for it ends.
    pub end_of_standard_support: SystemTime,
    /// The day extended support for it ends.
    pub end_of_extended_support: SystemTime,
}

impl ClusterVersionRecord {
    /// The key a `DescribeClusterVersions` page token names, so the next page
    /// resumes after this row. A calendar row is identified by its cluster type
    /// and version together, which is what makes the key unique.
    pub fn page_key(&self) -> String {
        format!("{}/{}", self.cluster_type, self.cluster_version)
    }

    /// Whether this version has been released at `now`. A version the calendar
    /// lists but has not released yet is not an available one.
    pub fn is_released(&self, now: SystemTime) -> bool {
        now >= self.release_date
    }

    /// Where this version sits in the support lifecycle at `now`: standard support
    /// until its end-of-standard-support day, extended support until its
    /// end-of-extended-support day, unsupported after that.
    pub fn support_status(&self, now: SystemTime) -> SupportStatus {
        if now < self.end_of_standard_support {
            SupportStatus::StandardSupport
        } else if now < self.end_of_extended_support {
            SupportStatus::ExtendedSupport
        } else {
            SupportStatus::Unsupported
        }
    }

    /// This row as the modelled `ClusterVersionInformation` shape, as of `now`.
    pub fn to_cluster_version_information(&self, now: SystemTime) -> ClusterVersionInformation {
        let status = self.support_status(now);
        ClusterVersionInformation {
            cluster_version: Some(self.cluster_version.clone()),
            cluster_type: Some(self.cluster_type.clone()),
            // The platform version a cluster of this Kubernetes version is created
            // at — the same constant `CreateCluster` writes onto a new record, so
            // the catalog and the cluster slice cannot disagree. Amazon EKS moves
            // this number on as it patches the control planes of a minor version;
            // this emulator patches nothing, so it publishes the INITIAL platform
            // version rather than a number it would have to invent per row.
            default_platform_version: Some(INITIAL_PLATFORM_VERSION.to_string()),
            default_version: self.default_version,
            release_date: Some(::aws_smithy_types::DateTime::from(self.release_date)),
            end_of_standard_support_date: Some(::aws_smithy_types::DateTime::from(
                self.end_of_standard_support,
            )),
            end_of_extended_support_date: Some(::aws_smithy_types::DateTime::from(
                self.end_of_extended_support,
            )),
            // The deprecated spelling and the current one, from the same state.
            status: Some(status.cluster_version_status().to_string()),
            version_status: Some(status.version_status().to_string()),
            kubernetes_patch_version: Some(self.kubernetes_patch_version()),
            // The provisioned control-plane catalogs. They do not depend on the
            // row: this service publishes one set of scaling tiers and one set of
            // component defaults and constraints, so every version reports them
            // (see [`CONTROL_PLANE_SCALING_TIERS`] and
            // [`control_plane_component_config`]).
            control_plane_scaling_tiers: Some(control_plane_scaling_tiers()),
            control_plane_component_config: Some(control_plane_component_config()),
        }
    }

    /// The Kubernetes patch version this service publishes for this row.
    ///
    /// `ClusterVersionInformation$kubernetesPatchVersion` is "The patch version of
    /// Kubernetes for this cluster version". This emulator runs no Kubernetes
    /// control plane, so there is no later patch build of a minor version to
    /// report: it publishes each minor version at its INITIAL patch level, the
    /// same way it publishes the initial platform version
    /// ([`INITIAL_PLATFORM_VERSION`]) for the control plane of a cluster it has
    /// just created.
    pub fn kubernetes_patch_version(&self) -> String {
        format!("{}.0", self.cluster_version)
    }
}

/// The provisioned control-plane scaling tiers this service publishes, as
/// `(tierName, apiRequestConcurrency, podSchedulingRatePerSecond,
/// clusterDatabaseSizeGb)`.
///
/// `ClusterVersionInformation$controlPlaneScalingTiers` is "The available
/// provisioned control plane scaling tiers and their capabilities for this
/// Kubernetes version", bounded by the model at 0..10 entries. The tier NAMES are
/// the members of `ProvisionedControlPlaneTier`
/// (`smithy_query query="shape" shape="ProvisionedControlPlaneTier"`:
/// `standard | tier-xl | tier-2xl | tier-4xl | tier-8xl`) — the same enum
/// `ControlPlaneScalingConfig$tier` accepts, so a caller can feed a tier name
/// this catalog publishes straight back into `CreateCluster`. `standard` is the
/// tier a cluster runs on when it asks for NO provisioned control plane, so it is
/// not one of the provisioned tiers published here; the four provisioned members
/// are published in the enum's own order, which is ascending capability.
///
/// The capabilities are the EKS provisioned-control-plane tier table: `tier-xl`
/// serves 2,000 concurrent API requests, schedules 167 pods a second and holds a
/// 16 GB cluster database, and each tier above it doubles all three. A caller
/// sizes a control plane from these three numbers, which is why a tier is
/// published with them rather than as a bare name.
const CONTROL_PLANE_SCALING_TIERS: &[(&str, i32, i32, i32)] = &[
    ("tier-xl", 2_000, 167, 16),
    ("tier-2xl", 4_000, 334, 32),
    ("tier-4xl", 8_000, 668, 64),
    ("tier-8xl", 16_000, 1_336, 128),
];

/// The provisioned control-plane scaling tiers, as the modelled shape.
///
/// Each tier carries no `controlPlaneComponentConfigOverrides`: that member is
/// "The control plane component configuration overrides specific to this scaling
/// tier", and the component defaults and constraints this service publishes
/// ([`control_plane_component_config`]) are the same on every tier — so there is
/// nothing a tier overrides, rather than a value invented per tier.
fn control_plane_scaling_tiers() -> Vec<ControlPlaneScalingTierInfo> {
    CONTROL_PLANE_SCALING_TIERS
        .iter()
        .map(
            |(tier_name, api_request_concurrency, pod_scheduling_rate, database_size_gb)| {
                ControlPlaneScalingTierInfo {
                    tier_name: Some((*tier_name).to_string()),
                    api_request_concurrency: Some(*api_request_concurrency),
                    pod_scheduling_rate_per_second: Some(*pod_scheduling_rate),
                    cluster_database_size_gb: Some(*database_size_gb),
                    control_plane_component_config_overrides: None,
                }
            },
        )
        .collect()
}

/// The default retention of Kubernetes events on a cluster's API server.
///
/// `KubeApiServerConfigRequest$eventTtl` is "The duration that Kubernetes events
/// are retained. Valid values are single-unit durations such as `30m` or `1h`",
/// and `1h` is the kube-apiserver `--event-ttl` default — so the catalog
/// publishes the component's own default, in the single-unit form the member
/// documents.
const DEFAULT_EVENT_TTL: &str = "1h";
/// The window the `eventTtl` catalog publishes around [`DEFAULT_EVENT_TTL`].
///
/// AWS publishes no bound for this parameter and the model constrains it only to
/// the `String` shape, so these two are the window this service publishes for it:
/// no shorter than a minute and no longer than a day.
const EVENT_TTL_MIN: &str = "1m";
/// See [`EVENT_TTL_MIN`].
const EVENT_TTL_MAX: &str = "24h";

/// The default `NodePort` range, which is the Kubernetes default
/// (`--service-node-port-range=30000-32767`) that
/// `KubeApiServerConfigRequest$serviceNodePortRange` ("The port range for
/// NodePort services") overrides.
const DEFAULT_SERVICE_NODE_PORT_MIN: i32 = 30_000;
/// See [`DEFAULT_SERVICE_NODE_PORT_MIN`].
const DEFAULT_SERVICE_NODE_PORT_MAX: i32 = 32_767;
/// The range each end of a `NodePort` range may be chosen from: an unprivileged
/// TCP/UDP port, so from 1024 up to the last port number there is.
const SERVICE_NODE_PORT_FLOOR: i32 = 1_024;
/// See [`SERVICE_NODE_PORT_FLOOR`].
const SERVICE_NODE_PORT_CEILING: i32 = 65_535;

/// The default number of terminated pods that may exist before the controller
/// manager starts deleting them — the kube-controller-manager
/// `--terminated-pod-gc-threshold` default, which
/// `PodGcControllerConfigRequest$terminatedPodGcThreshold` overrides.
const DEFAULT_TERMINATED_POD_GC_THRESHOLD: i32 = 12_500;
/// The bounds on `terminatedPodGcThreshold`. These are PUBLISHED by the model:
/// `TerminatedPodGcThresholdValue` is `range 1..1000000`
/// (`smithy_query query="shape" shape="PodGcControllerConfigRequest"`), so the
/// catalog publishes the same window the request member is validated against.
const TERMINATED_POD_GC_THRESHOLD_MIN: i32 = 1;
/// See [`TERMINATED_POD_GC_THRESHOLD_MIN`].
const TERMINATED_POD_GC_THRESHOLD_MAX: i32 = 1_000_000;

/// The default interval between horizontal-pod-autoscaler syncs — the
/// kube-controller-manager `--horizontal-pod-autoscaler-sync-period` default,
/// spelled as the single-unit duration
/// `HorizontalPodAutoscalerControllerConfigRequest$horizontalPodAutoscalerSyncPeriod`
/// documents ("Valid values are single-unit durations such as `15s` or `1m`").
const DEFAULT_HPA_SYNC_PERIOD: &str = "15s";
/// The window the `horizontalPodAutoscalerSyncPeriod` catalog publishes around
/// [`DEFAULT_HPA_SYNC_PERIOD`]. As with [`EVENT_TTL_MIN`], AWS publishes no
/// bound, so these are the window this service publishes: no faster than the
/// component's own default and no slower than five minutes.
const HPA_SYNC_PERIOD_MIN: &str = "15s";
/// See [`HPA_SYNC_PERIOD_MIN`].
const HPA_SYNC_PERIOD_MAX: &str = "5m";

/// The `ScoringStrategyType` members (`smithy_query query="shape"
/// shape="ScoringStrategyType"`), the first of which is the Kubernetes
/// `NodeResourcesFit` default.
const SCORING_STRATEGY_TYPES: &[&str] = &["LeastAllocated", "MostAllocated"];
/// The resources `NodeResourcesFit` scores a node on, which are the two the
/// Kubernetes scheduler weights by default (each at weight
/// [`DEFAULT_RESOURCE_WEIGHT`]).
const SCORED_RESOURCES: &[&str] = &["cpu", "memory"];
/// See [`SCORED_RESOURCES`].
const DEFAULT_RESOURCE_WEIGHT: i32 = 1;
/// The bounds on a resource weight. PUBLISHED by the model: `ResourceWeightValue`
/// is `range 1..100` (`smithy_query query="shape" shape="ResourceWeight"`).
const RESOURCE_WEIGHT_MIN: i32 = 1;
/// See [`RESOURCE_WEIGHT_MIN`].
const RESOURCE_WEIGHT_MAX: i32 = 100;

/// The control-plane component configuration defaults and constraints this
/// service publishes.
///
/// `ClusterVersionInformation$controlPlaneComponentConfig` is "The default control
/// plane component configuration and constraints for this Kubernetes version":
/// for each component parameter a cluster may override — on `CreateCluster`'s
/// `kubeApiServerConfig`, `kubeSchedulerConfig` and
/// `kubeControllerManagerConfig` — the value a cluster that overrides nothing
/// runs with, and the window an override may be chosen from. Every default is the
/// Kubernetes component's own documented default, and every constraint the model
/// publishes is taken from the model (see the constants above); the two the model
/// leaves open are the window this service publishes.
fn control_plane_component_config() -> ControlPlaneConfigInfo {
    ControlPlaneConfigInfo {
        kube_api_server_config: Some(KubeApiServerVersionConfig {
            event_ttl: Some(DurationParameterConfig {
                default_value: Some(DEFAULT_EVENT_TTL.to_string()),
                constraints: Some(DurationConstraints {
                    min: Some(EVENT_TTL_MIN.to_string()),
                    max: Some(EVENT_TTL_MAX.to_string()),
                }),
            }),
            service_node_port_range: Some(PortRangeParameterConfig {
                default_value: Some(ServiceNodePortRange {
                    min_port: DEFAULT_SERVICE_NODE_PORT_MIN,
                    max_port: DEFAULT_SERVICE_NODE_PORT_MAX,
                }),
                constraints: Some(PortRangeConstraints {
                    min_port: Some(IntegerRangeConstraint {
                        min: Some(SERVICE_NODE_PORT_FLOOR),
                        max: Some(SERVICE_NODE_PORT_CEILING),
                    }),
                    max_port: Some(IntegerRangeConstraint {
                        min: Some(SERVICE_NODE_PORT_FLOOR),
                        max: Some(SERVICE_NODE_PORT_CEILING),
                    }),
                }),
            }),
        }),
        kube_scheduler_config: Some(KubeSchedulerVersionConfig {
            node_resources_fit: Some(NodeResourcesFitVersionConfig {
                scoring_strategy: Some(ScoringStrategyConfig {
                    default_value: Some(ScoringStrategy {
                        r#type: SCORING_STRATEGY_TYPES
                            .first()
                            .map(|strategy| (*strategy).to_string()),
                        resources: Some(
                            SCORED_RESOURCES
                                .iter()
                                .map(|name| ResourceWeight {
                                    name: Some((*name).to_string()),
                                    weight: Some(DEFAULT_RESOURCE_WEIGHT),
                                })
                                .collect(),
                        ),
                    }),
                    constraints: Some(ScoringStrategyConstraints {
                        scoring_strategy: Some(AllowedValuesConstraint {
                            allowed_values: Some(
                                SCORING_STRATEGY_TYPES
                                    .iter()
                                    .map(|strategy| (*strategy).to_string())
                                    .collect(),
                            ),
                        }),
                        resources: Some(ResourceConstraints {
                            name: Some(AllowedValuesConstraint {
                                allowed_values: Some(
                                    SCORED_RESOURCES
                                        .iter()
                                        .map(|name| (*name).to_string())
                                        .collect(),
                                ),
                            }),
                            weight: Some(IntegerRangeConstraint {
                                min: Some(RESOURCE_WEIGHT_MIN),
                                max: Some(RESOURCE_WEIGHT_MAX),
                            }),
                        }),
                    }),
                }),
            }),
        }),
        kube_controller_manager_config: Some(KubeControllerManagerVersionConfig {
            pod_gc_controller_config: Some(PodGcControllerVersionConfig {
                terminated_pod_gc_threshold: Some(IntegerParameterConfig {
                    default_value: Some(DEFAULT_TERMINATED_POD_GC_THRESHOLD),
                    constraints: Some(IntegerConstraints {
                        min: Some(TERMINATED_POD_GC_THRESHOLD_MIN),
                        max: Some(TERMINATED_POD_GC_THRESHOLD_MAX),
                    }),
                }),
            }),
            horizontal_pod_autoscaler_controller_config: Some(
                HorizontalPodAutoscalerControllerVersionConfig {
                    horizontal_pod_autoscaler_sync_period: Some(DurationParameterConfig {
                        default_value: Some(DEFAULT_HPA_SYNC_PERIOD.to_string()),
                        constraints: Some(DurationConstraints {
                            min: Some(HPA_SYNC_PERIOD_MIN.to_string()),
                            max: Some(HPA_SYNC_PERIOD_MAX.to_string()),
                        }),
                    }),
                },
            ),
        }),
    }
}

/// The association id a create mints, derived from `seed`.
///
/// Opaque and stable: the digest of the seed, mapped onto
/// [`ASSOCIATION_ID_ALPHABET`], so the result always satisfies
/// [`ASSOCIATION_ID_PATTERN`]. It is computed once, at create time, and stored on
/// the record; no read recomputes it.
pub fn derive_association_id(seed: &str) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    let body: String = digest
        .iter()
        .take(ASSOCIATION_ID_BODY_LEN)
        .map(|byte| {
            ASSOCIATION_ID_ALPHABET[usize::from(*byte) % ASSOCIATION_ID_ALPHABET.len()] as char
        })
        .collect();
    format!("{ASSOCIATION_ID_PREFIX}{body}")
}

/// An EKS Pod Identity association's ARN, in the form EKS publishes:
/// `arn:aws:eks:<region>:<account>:podidentityassociation/<cluster>/<association id>`.
///
/// The ONE place this layout is written, so the handler that creates an
/// association directly and the one that creates associations for an add-on
/// cannot drift apart.
pub fn derive_association_arn(region: &str, cluster_name: &str, association_id: &str) -> String {
    ::engine::emulator_arn(
        "eks",
        region,
        &format!("podidentityassociation/{cluster_name}/{association_id}"),
    )
}

/// The `externalId` an association that names a `targetRoleArn` carries.
///
/// Derived from the association's own ARN — which already carries the account,
/// Region, cluster and association id — so it is distinct per association and
/// stable for the record's whole life, and rendered in the 8-4-4-4-12 hex form
/// AWS's external ids use. Computed once at create time and stored.
pub fn derive_association_external_id(association_arn: &str) -> String {
    let digest = Sha256::digest(association_arn.as_bytes());
    let hex = hex::encode(&digest[..16]);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// The Amazon EKS Kubernetes version calendar, as
/// `(version, EKS release, end of standard support, end of extended support)`.
///
/// These are the dates the "Amazon EKS Kubernetes release calendar" in the Amazon
/// EKS User Guide publishes, one row per Kubernetes minor version, each date a UTC
/// day. They follow the support policy the same guide states — a version enters
/// standard support on its Amazon EKS release date, standard support runs for 14
/// months, and extended support runs for a further 12 months — and the newer rows
/// sit on the cadence Amazon EKS publishes minor versions on, about one every four
/// months, so the calendar keeps carrying versions in standard support as the
/// clock advances rather than ageing into a table where every row is unsupported.
///
/// The table is the calendar as PUBLISHED, not the whole history of the service:
/// it carries the versions still in standard or extended support plus the one
/// most recently retired from support — the version a caller may still be running
/// and needs the dates for — and a version that dropped off the published
/// calendar before those is not a version `DescribeClusterVersions` reports.
///
/// Rows are written here in ascending Kubernetes version order; the calendar is
/// PUBLISHED newest version first (see [`seed_cluster_versions`]), which is the
/// order `DescribeClusterVersions` reports and pages them in. The calendar is a
/// fixed table, so it also bounds this part of the store.
const VERSION_CALENDAR: &[(&str, &str, &str, &str)] = &[
    ("1.30", "2024-05-23", "2025-07-23", "2026-07-23"),
    ("1.31", "2024-09-26", "2025-11-26", "2026-11-26"),
    ("1.32", "2025-01-23", "2026-03-23", "2027-03-23"),
    ("1.33", "2025-05-28", "2026-07-28", "2027-07-28"),
    ("1.34", "2025-10-01", "2026-12-01", "2027-12-01"),
    ("1.35", "2026-02-01", "2027-04-01", "2028-04-01"),
    ("1.36", "2026-06-01", "2027-08-01", "2028-08-01"),
    ("1.37", "2026-10-01", "2027-12-01", "2028-12-01"),
];

/// The first instant (UTC) of a `YYYY-MM-DD` day in [`VERSION_CALENDAR`].
///
/// The day is widened to an RFC-3339 instant and parsed by `aws-smithy-types`,
/// which is the same parser the wire layer uses, rather than by hand-rolled
/// calendar arithmetic. Every calendar day is after the epoch, so the parsed
/// seconds are non-negative. A row that does not parse is a typo in the constant
/// table above, which `calendar_rows_are_well_formed` pins.
fn calendar_instant(day: &str) -> SystemTime {
    let text = format!("{day}T00:00:00Z");
    let parsed = ::aws_smithy_types::DateTime::from_str(
        &text,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|error| panic!("{text} is not an RFC-3339 instant: {error}"));
    SystemTime::UNIX_EPOCH + Duration::from_secs(parsed.secs().unsigned_abs())
}

/// Build the version calendar this service publishes from [`VERSION_CALENDAR`].
///
/// The rows come out NEWEST VERSION FIRST, which is the order the calendar is
/// published in: a caller reading the catalog to pick a version sees the newest
/// Kubernetes version Amazon EKS offers at the head of the list, and the first
/// page of a paged listing is the newest versions rather than the oldest. The
/// listing reports the calendar in the order the store holds it, so this is where
/// that order is decided.
fn seed_cluster_versions() -> Vec<ClusterVersionRecord> {
    VERSION_CALENDAR
        .iter()
        .rev()
        .map(
            |(version, release, standard_end, extended_end)| ClusterVersionRecord {
                cluster_version: (*version).to_string(),
                cluster_type: CLUSTER_TYPE_EKS.to_string(),
                // The one row a `CreateCluster` without `version` lands on, so a
                // caller that asks this catalog for the default version and a
                // caller that omits the version get the same Kubernetes version.
                default_version: *version == DEFAULT_KUBERNETES_VERSION,
                release_date: calendar_instant(release),
                end_of_standard_support: calendar_instant(standard_end),
                end_of_extended_support: calendar_instant(extended_end),
            },
        )
        .collect()
}

/// Per-service state for com-amazonaws-eks-awswesleyfrontend. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsEksAwswesleyfrontendState {
    /// Clusters by `(region, name)`.
    ///
    /// A cluster name is unique per REGION, not globally — "The name must be
    /// unique within the Amazon Web Services Region and Amazon Web Services
    /// account that you're creating the cluster in" (`CreateCluster$name`) — and
    /// every ARN and endpoint a record carries names one region, so the region a
    /// request is signed for is part of the key. A `BTreeMap` so `ListClusters`
    /// walks a stable, sorted order within a region and a page token can name the
    /// key to resume after. The account is not part of the key: this emulator
    /// serves a single account ([`::engine::mock_account_id`]). Its size is
    /// bounded by [`MAX_STORED_CLUSTERS`], which `CreateCluster` enforces.
    pub clusters: BTreeMap<(String, String), ClusterRecord>,
    /// Cluster ARN -> the `(region, name)` key of the cluster it identifies, so
    /// the tag operations resolve a `resourceArn` without scanning every record.
    /// An EKS cluster ARN already carries the region, so this is what lets a tag
    /// call name a cluster in any region. It holds exactly one entry per stored
    /// cluster, so [`MAX_STORED_CLUSTERS`] bounds it too.
    pub arn_index: BTreeMap<String, (String, String)>,
    /// Managed node groups by `(region, cluster name, nodegroup name)`.
    ///
    /// A node group name identifies a node group only WITHIN one cluster, and a
    /// cluster name only within one region (see [`Self::clusters`]), so the key
    /// is the whole triple: the same node group name may exist in another
    /// cluster or another region. A `BTreeMap` so a listing walks a stable,
    /// sorted order within a cluster. Its size is bounded by
    /// [`MAX_STORED_NODEGROUPS`], which `CreateNodegroup` enforces.
    ///
    /// Node group ARNs are deliberately NOT added to [`Self::arn_index`]: that
    /// index answers "which CLUSTER does this ARN name" for the tag operations,
    /// and tagging a node group through `TagResource` belongs to the tag slice's
    /// own gap issue.
    pub nodegroups: BTreeMap<(String, String, String), NodegroupRecord>,
    /// Add-ons by `(region, cluster name, add-on name)`.
    ///
    /// An add-on is a resource INSIDE one cluster — `CreateAddon`'s `clusterName`
    /// is an `@httpLabel` on `/clusters/{clusterName}/addons` and one add-on name
    /// installs at most once per cluster — so the key is the cluster's own
    /// `(region, name)` key plus the add-on name. A `BTreeMap` so a listing walks
    /// one cluster's add-ons in a stable, sorted order. Its size is bounded by
    /// [`MAX_ADDONS_PER_CLUSTER`] per cluster, which `CreateAddon` enforces,
    /// times the [`MAX_STORED_CLUSTERS`] clusters the store holds.
    ///
    /// Add-on ARNs are deliberately NOT in [`Self::arn_index`]: that index is
    /// what the tag operations resolve a CLUSTER from, and tagging an add-on
    /// through its own ARN is the tag slice's own gap, not this one's.
    pub addons: BTreeMap<(String, String, String), AddonRecord>,
    /// Managed capabilities by `(region, cluster name, capability name)`.
    ///
    /// A capability name is unique within its CLUSTER ("The name must be unique
    /// within your cluster", `CreateCapability$capabilityName`), and a cluster name
    /// is unique within a region, so the key is the triple. A `BTreeMap` so
    /// `ListCapabilities` walks a stable, sorted order within one cluster and a
    /// page token can name the capability to resume after. Bounded by
    /// [`MAX_CAPABILITIES_PER_CLUSTER`] per cluster and so, with
    /// [`MAX_STORED_CLUSTERS`] clusters, bounded overall.
    pub capabilities: BTreeMap<(String, String, String), CapabilityRecord>,
    /// EKS Pod Identity associations by `(region, clusterName, associationId)`.
    ///
    /// An association belongs to exactly one cluster and a cluster name only
    /// identifies a cluster within one Region, so the key carries both; the
    /// association id is unique within the cluster. A `BTreeMap` so a listing
    /// walks one cluster's associations as a contiguous, sorted range. Bounded by
    /// [`MAX_POD_IDENTITY_ASSOCIATIONS_PER_CLUSTER`] per cluster, which
    /// `CreatePodIdentityAssociation` enforces, times the bounded number of
    /// clusters.
    pub pod_identity_associations: BTreeMap<(String, String, String), PodIdentityAssociationRecord>,
    /// How many association ids this store has minted, ever.
    ///
    /// It is the uniqueness seam for [`Self::mint_association_id`]: two creates
    /// for the same cluster, namespace and service account (one after the other
    /// was deleted) must not mint the same id, so the counter — which never goes
    /// back — is part of the seed.
    pub minted_associations: u64,
    /// The Kubernetes version calendar `DescribeClusterVersions` reports, seeded
    /// once from [`VERSION_CALENDAR`] (see [`ClusterVersionRecord`]) and never
    /// written by a request: it is what Amazon EKS publishes, not per-account
    /// data. A `Vec` because the calendar has a publication ORDER (newest
    /// Kubernetes version first, see [`seed_cluster_versions`]) that the listing
    /// and its page tokens follow.
    pub cluster_versions: Vec<ClusterVersionRecord>,
    /// How many update ids this store has minted. Mixed into the next one (see
    /// [`new_update_id`]) so two updates minted in the same nanosecond cannot
    /// collide.
    pub minted_updates: u64,
    // An asynchronous update is not a field here — it lives on the cluster it
    // updates ([`ClusterRecord::updates`]).
}

impl Default for ComAmazonawsEksAwswesleyfrontendState {
    /// A fresh store holds no clusters and the whole published version calendar:
    /// `DescribeClusterVersions` answers before anything has been created.
    fn default() -> Self {
        Self {
            clusters: BTreeMap::new(),
            arn_index: BTreeMap::new(),
            nodegroups: BTreeMap::new(),
            addons: BTreeMap::new(),
            capabilities: BTreeMap::new(),
            pod_identity_associations: BTreeMap::new(),
            minted_associations: 0,
            cluster_versions: seed_cluster_versions(),
            minted_updates: 0,
        }
    }
}

impl ComAmazonawsEksAwswesleyfrontendState {
    /// The calendar rows that are released at `now`, in the calendar's own
    /// publication order (newest Kubernetes version first).
    pub fn released_cluster_versions(
        &self,
        now: SystemTime,
    ) -> impl Iterator<Item = &ClusterVersionRecord> + '_ {
        self.cluster_versions
            .iter()
            .filter(move |entry| entry.is_released(now))
    }

    /// Materialise every clock-owed status move across the store.
    pub fn materialize_due_transitions(&mut self, now: SystemTime) {
        for cluster in self.clusters.values_mut() {
            cluster.materialize_due_transitions(now);
        }
        for nodegroup in self.nodegroups.values_mut() {
            nodegroup.materialize_due_transitions(now);
        }
        for addon in self.addons.values_mut() {
            addon.materialize_due_transitions(now);
        }
        for capability in self.capabilities.values_mut() {
            capability.materialize_due_transitions(now);
        }
    }

    /// The names of the add-ons installed on one cluster, in sorted order.
    ///
    /// The map is keyed `(region, cluster, addon)`, so its iteration order
    /// already sorts each cluster's add-on names; this only drops the other
    /// clusters.
    pub fn addon_names_in<'a>(
        &'a self,
        region: &'a str,
        cluster_name: &'a str,
    ) -> impl Iterator<Item = &'a str> + 'a {
        self.addons
            .keys()
            .filter(move |(key_region, key_cluster, _)| {
                key_region == region && key_cluster == cluster_name
            })
            .map(|(_, _, addon_name)| addon_name.as_str())
    }

    /// Drop every add-on installed on one cluster.
    ///
    /// An add-on only exists inside its cluster, so whatever removes the cluster
    /// removes its add-ons with it — otherwise a cluster created again under the
    /// same name would inherit the add-ons of the one that is gone.
    pub fn remove_cluster_addons(&mut self, region: &str, cluster_name: &str) {
        self.addons.retain(|(key_region, key_cluster, _), _| {
            key_region != region || key_cluster != cluster_name
        });
    }

    /// The next value to mix into a minted update id, counted from this store's
    /// own total rather than from any one cluster's.
    pub fn next_update_sequence(&mut self) -> u64 {
        self.minted_updates = self.minted_updates.wrapping_add(1);
        self.minted_updates
    }

    /// The `(region, name)` key of the cluster `arn` identifies, if this store
    /// holds it.
    pub fn cluster_key_for_arn(&self, arn: &str) -> Option<(String, String)> {
        self.arn_index.get(arn).cloned()
    }

    /// The names of the clusters this store holds in `region`, in sorted order.
    ///
    /// The map is keyed `(region, name)`, so its iteration order already sorts
    /// each region's names; this only drops the other regions. EVERY cluster is
    /// counted, connected ones included, because what this feeds is the quota and
    /// the uniqueness of a name within a region — neither of which distinguishes
    /// how the cluster got there.
    pub fn cluster_names_in<'a>(&'a self, region: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.clusters
            .keys()
            .filter(move |(key_region, _)| key_region == region)
            .map(|(_, name)| name.as_str())
    }

    /// The names of the node groups this store holds in `cluster_name` in
    /// `region`, in sorted order.
    ///
    /// The map is keyed `(region, cluster, nodegroup)`, so its iteration order
    /// already sorts each cluster's node group names; this only drops the other
    /// clusters and regions.
    pub fn nodegroup_names_in<'a>(
        &'a self,
        region: &'a str,
        cluster_name: &'a str,
    ) -> impl Iterator<Item = &'a str> + 'a {
        self.nodegroups
            .keys()
            .filter(move |(key_region, key_cluster, _)| {
                key_region == region && key_cluster == cluster_name
            })
            .map(|(_, _, name)| name.as_str())
    }

    /// The capabilities one cluster holds, in capability-name order.
    ///
    /// The map is keyed `(region, cluster, capability)`, so one cluster's
    /// capabilities are a contiguous, name-sorted RANGE of it: the walk starts at
    /// that cluster's first possible key and stops at its last, the way
    /// [`Self::pod_identity_associations_in`] does for its own child resource.
    pub fn capabilities_of<'a>(
        &'a self,
        region: &'a str,
        cluster_name: &'a str,
    ) -> impl Iterator<Item = &'a CapabilityRecord> + 'a {
        self.capabilities
            .range((
                std::ops::Bound::Included((
                    region.to_string(),
                    cluster_name.to_string(),
                    String::new(),
                )),
                std::ops::Bound::Unbounded,
            ))
            .take_while(move |((key_region, key_cluster, _), _)| {
                key_region == region && key_cluster == cluster_name
            })
            .map(|(_, record)| record)
    }

    /// The names `ListClusters` projects for `region`, in sorted order.
    ///
    /// A connected cluster is listed only when the caller asked for it:
    /// `include` "indicates whether external clusters are included in the
    /// returned list. Use 'all' to return connected clusters, or blank to return
    /// only Amazon EKS clusters" (`ListClustersRequest$include`).
    pub fn listed_cluster_names_in<'a>(
        &'a self,
        region: &'a str,
        include_connected: bool,
    ) -> impl Iterator<Item = &'a str> + 'a {
        self.clusters
            .iter()
            .filter(move |((key_region, _), record)| {
                key_region == region && (include_connected || !record.is_connected())
            })
            .map(|((_, name), _)| name.as_str())
    }

    /// Every EKS Pod Identity association of one cluster, in association-id order.
    ///
    /// The map is keyed `(region, clusterName, associationId)`, so one cluster's
    /// associations are a contiguous range of it — which is also the ascending
    /// id order a page token resumes after.
    pub fn pod_identity_associations_in<'a>(
        &'a self,
        region: &'a str,
        cluster_name: &'a str,
    ) -> impl Iterator<Item = &'a PodIdentityAssociationRecord> + 'a {
        self.pod_identity_associations
            .range((
                std::ops::Bound::Included((
                    region.to_string(),
                    cluster_name.to_string(),
                    String::new(),
                )),
                std::ops::Bound::Unbounded,
            ))
            .take_while(move |((key_region, key_cluster, _), _)| {
                key_region == region && key_cluster == cluster_name
            })
            .map(|(_, record)| record)
    }

    /// Drop every capability of one cluster, for a caller that is removing the
    /// cluster itself.
    ///
    /// A capability only exists inside a cluster — its ARN and its key both name
    /// one — so a cluster that leaves the store takes its capabilities with it.
    /// Without this, a later cluster created with the same name in the same region
    /// would inherit the dead cluster's capabilities.
    pub fn remove_capabilities_of(&mut self, region: &str, cluster_name: &str) {
        self.capabilities.retain(|(key_region, key_cluster, _), _| {
            key_region != region || key_cluster != cluster_name
        });
    }

    /// Mint an association id that no association of this cluster already holds.
    ///
    /// The seed carries the cluster, the pair the association is for and the
    /// store's monotonic mint counter, so a re-create after a delete mints a NEW
    /// id rather than resurrecting the old one. The loop is the collision guard:
    /// a digest can repeat, and an id must be unique within its cluster.
    pub fn mint_association_id(
        &mut self,
        region: &str,
        cluster_name: &str,
        namespace: &str,
        service_account: &str,
    ) -> String {
        loop {
            self.minted_associations = self.minted_associations.wrapping_add(1);
            let id = derive_association_id(&format!(
                "{region}/{cluster_name}/{namespace}/{service_account}/{}",
                self.minted_associations
            ));
            let key = (region.to_string(), cluster_name.to_string(), id.clone());
            if !self.pod_identity_associations.contains_key(&key) {
                return id;
            }
        }
    }

    /// Drop every EKS Pod Identity association of one cluster.
    ///
    /// An association is a child of its cluster: once the cluster is gone the
    /// associations are gone with it, so a cluster later created with the same
    /// name in the same Region does not inherit them.
    pub fn remove_pod_identity_associations_for_cluster(
        &mut self,
        region: &str,
        cluster_name: &str,
    ) {
        self.pod_identity_associations
            .retain(|(key_region, key_cluster, _), _| {
                key_region != region || key_cluster != cluster_name
            });
    }

    /// Drop every managed node group of one cluster.
    ///
    /// A node group is a child of its cluster — it is addressed as
    /// `/clusters/{clusterName}/node-groups/{nodegroupName}` — so once the
    /// cluster is gone its node groups are gone with it, and a cluster later
    /// created with the same name in the same Region does not inherit them.
    pub fn remove_nodegroups_for_cluster(&mut self, region: &str, cluster_name: &str) {
        self.nodegroups
            .retain(|(key_region, key_cluster, _), _| {
                key_region != region || key_cluster != cluster_name
            });
    }
}

/// The region a request is signed for, read from its SigV4 credential scope.
///
/// Every ARN and endpoint this service mints is regional, so the region comes
/// from the caller rather than from a constant. A request with no parseable
/// scope (authentication off, no `Authorization` header) falls back to
/// [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The first 16 bytes of `seed`'s SHA-256 digest, as lowercase hex. The shared
/// step of the identifier this service derives from stable content
/// ([`derive_cluster_resource_id`]); 16 bytes is the width it publishes.
fn digest_hex(seed: &str) -> String {
    hex::encode(&Sha256::digest(seed.as_bytes())[..16])
}

/// 16 bytes rendered in the hyphenated `8-4-4-4-12` lowercase-hex layout a UUID
/// takes. The one renderer for every one of this service's UUID-shaped
/// identifiers ([`new_update_id`], [`derive_activation_id`] and
/// [`derive_access_entry_id`]).
fn hyphenate_uuid(bytes: &[u8; 16]) -> String {
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// A fresh identifier in the shape EKS gives an `Update$id`: "a UUID that is
/// used to track the update" — a version-4 UUID, `8-4-4-4-12` lowercase hex with
/// the version and variant bits RFC 4122 §4.4 fixes.
///
/// The 128 bits come from three sources hashed together with SHA-256 (the `sha2`
/// crate) and hex-encoded (the `hex` crate): the OS-seeded randomness a
/// `RandomState` draws, the system clock in nanoseconds, and `sequence` — a
/// counter the store hands out ([`ComAmazonawsEksAwswesleyfrontendState::next_update_sequence`]),
/// so two ids minted in the same nanosecond cannot collide. An update id names a
/// resource for correlation, never a credential.
pub fn new_update_id(sequence: u64) -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::BuildHasher;

    let nanos = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(RandomState::new().hash_one(sequence).to_le_bytes());
    hasher.update(nanos.to_le_bytes());
    hasher.update(sequence.to_le_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 4122 §4.4: the version nibble is 4, and the variant's two top bits are `10`.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    hyphenate_uuid(&bytes)
}

/// The 32-hex-character resource identifier EKS gives a cluster, which appears in
/// the cluster's API server endpoint host and in its OIDC issuer URL.
///
/// Derived from the cluster ARN — which already carries the account, region and
/// name — so it is distinct per cluster and stable for the record's whole life.
/// It is computed once, when the cluster is created, and stored on the record;
/// no read recomputes it.
pub fn derive_cluster_resource_id(arn: &str) -> String {
    digest_hex(arn).to_ascii_uppercase()
}

/// The cluster's Kubernetes API server endpoint, in the host form EKS publishes:
/// `https://<resource id>.<shard>.<region>.eks.amazonaws.com`.
pub fn derive_cluster_endpoint(resource_id: &str, region: &str) -> String {
    let shard = resource_id
        .get(..3)
        .unwrap_or("eks")
        .to_ascii_lowercase();
    format!("https://{resource_id}.{shard}.{region}.eks.amazonaws.com")
}

/// The cluster's OIDC identity provider issuer URL, in the form EKS publishes:
/// `https://oidc.eks.<region>.amazonaws.com/id/<resource id>`.
pub fn derive_oidc_issuer(resource_id: &str, region: &str) -> String {
    format!("https://oidc.eks.{region}.amazonaws.com/id/{resource_id}")
}

/// A fresh 32-hex-character resource identifier, in the shape
/// [`derive_cluster_resource_id`] gives a cluster.
///
/// A cluster's id is derived from its ARN, because the ARN already identifies it
/// uniquely. An identity provider configuration's id is the other way round — it
/// is a part of the ARN — so there is nothing to derive it from and it is minted
/// per call. The bits are [`new_update_id`]'s, which this crate already mints for
/// an `Update$id` from OS-seeded randomness, the clock and the store's own
/// counter; this only drops the hyphens and upper-cases the hex, which is the
/// shape a cluster's resource id has.
pub fn new_resource_id(sequence: u64) -> String {
    new_update_id(sequence).replace('-', "").to_uppercase()
}

/// The ARN of an identity provider configuration, in the form EKS publishes:
/// `arn:aws:eks:<region>:<account>:identityproviderconfig/<cluster>/<type>/<name>/<resource id>`.
///
/// The configuration is a sub-resource of its cluster, so the cluster name, the
/// configuration type and its name are all path components of the resource
/// portion, with the server-minted id last. It is computed once, when the
/// configuration is associated, and stored on the record; no read recomputes it.
pub fn identity_provider_config_arn(
    region: &str,
    cluster_name: &str,
    config_type: &str,
    config_name: &str,
    resource_id: &str,
) -> String {
    ::engine::emulator_arn(
        "eks",
        region,
        &format!("identityproviderconfig/{cluster_name}/{config_type}/{config_name}/{resource_id}"),
    )
}

/// The IPv6 service CIDR a cluster gets when `ipFamily` is `ipv6`.
///
/// The model states the rule: "Kubernetes assigns services `IPv6` addresses from
/// the unique local address range `(fc00::/7)`. You can't specify a custom
/// `IPv6` CIDR block" (`KubernetesNetworkConfigRequest$ipFamily`) — so the block
/// is always server-assigned. It is derived from the cluster's resource id, which
/// puts it inside `fd00::/8`, and stored with the record.
pub fn derive_service_ipv6_cidr(resource_id: &str) -> String {
    let hextet = |range: std::ops::Range<usize>| {
        resource_id
            .get(range)
            .unwrap_or("0000")
            .to_ascii_lowercase()
    };
    format!("fd{}:{}:{}::/108", hextet(0..2), hextet(2..6), hextet(6..10))
}

/// A certificate authority's `data`, which a cluster also reports as its
/// `certificateAuthority.data`.
///
/// A real cluster reports the base64 of the PEM bundle its API server is fronted
/// by. This emulator runs no certificate authority, so rather than synthesise
/// bytes that look like a certificate it stores an explicitly labelled
/// placeholder: a caller that base64-decodes it reads, in plain text, that no CA
/// exists here. It is written once at create time and read back unchanged.
/// `subject` names what the certificate would have been issued for (a cluster id,
/// a certificate authority id), so two certificate authorities never report the
/// same blob.
pub fn derive_certificate_authority_data(subject: &str) -> String {
    let placeholder = format!(
        "-----BEGIN CERTIFICATE-----\n\
         EMULATOR PLACEHOLDER: this emulator runs no certificate authority, so this\n\
         is not a usable certificate for {subject}.\n\
         -----END CERTIFICATE-----\n"
    );
    ::aws_smithy_types::base64::encode(placeholder)
}

/// The node group's own identifier, the last segment of its `nodegroupArn`.
///
/// An EKS node group ARN is
/// `arn:aws:eks:<region>:<account>:nodegroup/<cluster>/<nodegroup>/<id>`, where
/// the trailing id distinguishes two node groups that carried the same name at
/// different times. It is derived from the region, the two names and the
/// creation instant — so it is distinct per creation and stable for the
/// renderer for this service's UUID-shaped identifiers, through
/// [`derive_uuid_from_seed`]. It is computed once, when the node group is
/// created, and stored on the record; no read recomputes it.
pub fn derive_nodegroup_id(
    region: &str,
    cluster_name: &str,
    nodegroup_name: &str,
    created_at: SystemTime,
) -> String {
    let created_nanos = created_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_nanos());
    derive_uuid_from_seed(&format!(
        "{region}/{cluster_name}/{nodegroup_name}/{created_nanos}"
    ))
}

/// The identifier EKS puts in the last segment of an add-on's ARN
/// (`arn:aws:eks:<region>:<account>:addon/<cluster>/<add-on>/<id>`), in the
/// 8-4-4-4-12 hex form AWS prints it in ([`hyphenate_uuid`], the one renderer
/// for this service's UUID-shaped identifiers).
///
/// Derived from the add-on's own key — region, cluster and add-on name — so it
/// is distinct per add-on and stable for the record's whole life. It is computed
/// once, when the add-on is created, and stored on the record; no read
/// recomputes it.
pub fn derive_addon_resource_id(region: &str, cluster_name: &str, addon_name: &str) -> String {
    derive_uuid_from_seed(&format!("{region}/{cluster_name}/{addon_name}"))
}

/// The add-on's ARN, in the form EKS publishes:
/// `arn:aws:eks:<region>:<account>:addon/<cluster>/<add-on>/<resource id>`.
pub fn derive_addon_arn(
    region: &str,
    cluster_name: &str,
    addon_name: &str,
    resource_id: &str,
) -> String {
    ::engine::emulator_arn(
        "eks",
        region,
        &format!("addon/{cluster_name}/{addon_name}/{resource_id}"),
    )
}

/// The version `CreateAddon` assigns when the caller names none.
///
/// "The version of the add-on. The version must match one of the versions
/// returned by `DescribeAddonVersions`" (`CreateAddon$addonVersion`) — and when
/// the member is absent the real service installs the default version for the
/// cluster's Kubernetes version. `DescribeAddonVersions` is still a scaffold in
/// this emulator (its own gap issue,
/// `issues/.../eks-describeaddonversions-stub.md`), so there is no catalog to
/// pick that version from: this builds a version in the `vX.Y.Z-eksbuild.N` form
/// every EKS add-on version takes, out of the cluster's own Kubernetes version,
/// so a cluster on one Kubernetes version always reports the same default and
/// the value a caller reads back is the one the record was created with.
///
/// It is a CHOSEN default, not a real one: a real add-on's version tracks the
/// ADD-ON, not the control plane (`vpc-cni` on a 1.31 cluster is a `v1.19.x`
/// build), and nothing here can know it. When the version catalog lands, the
/// default must come from it and this function must go, so the two cannot
/// disagree.
pub fn derive_default_addon_version(kubernetes_version: &str) -> String {
    let mut parts = kubernetes_version
        .split('.')
        .filter(|part| !part.is_empty());
    let major = parts.next().unwrap_or("1");
    let minor = parts.next().unwrap_or("0");
    let patch = parts.next().unwrap_or("0");
    format!("v{major}.{minor}.{patch}-eksbuild.1")
}

/// Whether `name` has the FORM an Amazon EKS add-on name takes: a non-empty
/// string of ASCII alphanumerics, `-`, `.` and `_`.
///
/// `CreateAddon$addonName` targets a plain `String` with no `length` or
/// `pattern` trait (`smithy_query query="operation" operation="CreateAddon"`),
/// so the boundary validator accepts anything — including the empty string —
/// and this is the check that refuses a value no add-on name can be. It is a
/// FORM check, not a catalog check: "the name must match one of the names
/// returned by `DescribeAddonVersions`", and that operation is still a scaffold
/// here (its own gap issue), so this emulator holds no catalog to match
/// against, and a hand-written subset of it would refuse add-ons the real
/// service accepts. The character set is the one every published add-on name
/// uses, EKS-managed (`vpc-cni`) and Marketplace (`<publisher>_<product>`)
/// alike.
///
/// The LENGTH ceiling is [`MAX_KUBERNETES_NAME_LEN`], the DNS-1123 subdomain
/// limit this module already bounds Kubernetes names by: an add-on name names
/// the Kubernetes workloads the add-on installs, it becomes part of this store's
/// key and of the add-on's ARN, and no published add-on name comes near it — so
/// a name past it names no add-on the real service would accept either.
pub fn is_addon_name_shaped(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MAX_KUBERNETES_NAME_LEN
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
}

/// The digest one registration's server-generated values are derived from.
///
/// `domain` separates the values derived for the same registration, so the
/// activation id does not reveal the activation code; the cluster ARN and the
/// creation instant together make it distinct per registration, so registering
/// the same name again after deregistering it mints fresh values rather than
/// repeating the previous ones.
fn registration_digest(arn: &str, created_at: SystemTime, domain: &str) -> [u8; 32] {
    let nanos = created_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update([0u8]);
    hasher.update(arn.as_bytes());
    hasher.update([0u8]);
    hasher.update(nanos.to_be_bytes());
    hasher.finalize().into()
}

/// The `connectorConfig.activationId` of a registration: "a unique ID associated
/// with the cluster for registration purposes"
/// (`ConnectorConfigResponse$activationId`), in the hyphenated 8-4-4-4-12 hex
/// form an activation id takes.
///
/// Computed once, by `RegisterCluster`, and stored on the record — no read
/// recomputes it, so it is stable for the registration's whole life.
pub fn derive_activation_id(arn: &str, created_at: SystemTime) -> String {
    let digest = registration_digest(arn, created_at, "activation-id");
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    hyphenate_uuid(&bytes)
}

/// The `connectorConfig.activationCode` of a registration: "a unique code
/// associated with the cluster for registration purposes"
/// (`ConnectorConfigResponse$activationCode`).
///
/// It is NOT a credential this emulator authenticates anything with: the code is
/// consumed by the connector agent against AWS Systems Manager, which this
/// emulator does not serve, so nothing here accepts it and no request is
/// authorized by it (an operation that returns a real credential mints it
/// through the credential store instead). It is derived like the activation id,
/// from a separate domain, so a read-back returns the same value the
/// registration reported.
pub fn derive_activation_code(arn: &str, created_at: SystemTime) -> String {
    let digest = registration_digest(arn, created_at, "activation-code");
    // 15 bytes encode to 20 base64 characters with no padding.
    ::aws_smithy_types::base64::encode(&digest[..15])
}

/// How many bytes of the name's own digest a page token carries, so a token this
/// service did not mint is refused instead of being resumed from. Four bytes is
/// the usual width for a checksum of this kind; it is a provenance check, not a
/// secret (the name travels in the token, as AWS's own opaque-but-unsigned
/// pagination tokens do).
pub const PAGE_TOKEN_TAG_LEN: usize = 4;

/// The listing a page token belongs to, mixed into its provenance tag so a token
/// one listing minted is not resumed from by another.
///
/// `ListClusters` pages the cluster names and is the listing whose tokens carry the
/// EMPTY scope, which is what [`encode_page_token`] passes.
pub const CLUSTERS_LISTING: &str = "";
/// The scope `ListAccessPolicies`'s tokens carry. See [`CLUSTERS_LISTING`].
pub const ACCESS_POLICIES_LISTING: &str = "access-policies";
/// The scope `ListCapabilities`'s tokens carry. See [`CLUSTERS_LISTING`].
pub const CAPABILITIES_LISTING: &str = "capabilities";
/// The scope `ListPodIdentityAssociations`'s tokens carry, whose key is an
/// association id. See [`CLUSTERS_LISTING`].
pub const POD_IDENTITY_ASSOCIATIONS_LISTING: &str = "pod-identity-associations";
/// The scope `DescribeClusterVersions`'s tokens carry, over the calendar rows'
/// [`ClusterVersionRecord::page_key`]s. See [`CLUSTERS_LISTING`].
pub const CLUSTER_VERSIONS_LISTING: &str = "cluster-versions";
/// The scope `ListAccessEntries`'s tokens carry, over one cluster's principal
/// ARNs. See [`CLUSTERS_LISTING`].
pub const ACCESS_ENTRIES_LISTING: &str = "access-entries";
/// The scope `ListAssociatedAccessPolicies`'s tokens carry, over one access
/// entry's associated policy ARNs. See [`CLUSTERS_LISTING`].
pub const ASSOCIATED_ACCESS_POLICIES_LISTING: &str = "associated-access-policies";

/// Encode a page token for `listing` that resumes AFTER `last_returned`.
///
/// Derived from the last key returned, not from an index into a transient list,
/// so a token stays valid across creates and deletes. Hex rather than base64
/// because the token travels in a query string (`@httpQuery nextToken`) and the
/// standard base64 alphabet contains `+` and `/`, which a query parser may read
/// as a space and a separator.
///
/// The name is prefixed with [`PAGE_TOKEN_TAG_LEN`] bytes of the digest of
/// `listing` and the name together, so only a token THIS listing minted is
/// honoured: an arbitrary hex string — including a well-formed token another
/// listing issued — fails the check and is refused rather than silently resumed
/// from.
pub fn encode_scoped_page_token(listing: &str, last_returned: &str) -> String {
    let mut token = Vec::with_capacity(PAGE_TOKEN_TAG_LEN + last_returned.len());
    token.extend_from_slice(&page_token_tag(listing, last_returned));
    token.extend_from_slice(last_returned.as_bytes());
    hex::encode(token)
}

/// The provenance tag a `listing`'s token carries for `last_returned`: the first
/// [`PAGE_TOKEN_TAG_LEN`] bytes of the digest of the two together.
///
/// The scope is LENGTH-PREFIXED before it is digested, which is what makes the tag
/// a function of the PAIR `(listing, last_returned)` rather than of their
/// concatenation. Digesting the bare concatenation assigns the SAME tag to two
/// different pairs whose concatenations are equal — `("", "cluster-versionsmy-cluster")`
/// and `("cluster-versions", "my-cluster")`, and the empty [`CLUSTERS_LISTING`]
/// makes such a pair exist for every scope — so one listing's tag could be spliced
/// onto the other listing's payload. That is domain separation of the tag input,
/// not a security boundary: the tag is unkeyed and public (see
/// [`PAGE_TOKEN_TAG_LEN`]), and [`decode_scoped_page_token`] re-digests the
/// payload it reads, so a token is never resumed under a scope that did not mint
/// it either way.
fn page_token_tag(listing: &str, last_returned: &str) -> [u8; PAGE_TOKEN_TAG_LEN] {
    let mut digest = Sha256::new();
    // `usize` is at most 64 bits on every target this emulator builds for, so the
    // conversion is total; the fixed 8-byte width is what keeps the prefix itself
    // unambiguous.
    digest.update(
        u64::try_from(listing.len())
            .expect("a listing scope shorter than 2^64 bytes")
            .to_be_bytes(),
    );
    digest.update(listing.as_bytes());
    digest.update(last_returned.as_bytes());
    let digest = digest.finalize();
    let mut tag = [0u8; PAGE_TOKEN_TAG_LEN];
    tag.copy_from_slice(&digest[..PAGE_TOKEN_TAG_LEN]);
    tag
}

/// The name a `listing`'s page token resumes after, or `None` when the token is
/// not one that listing issued.
pub fn decode_scoped_page_token(listing: &str, token: &str) -> Option<String> {
    let bytes = hex::decode(token).ok()?;
    if bytes.len() <= PAGE_TOKEN_TAG_LEN {
        return None;
    }
    let (tag, name) = bytes.split_at(PAGE_TOKEN_TAG_LEN);
    let name = String::from_utf8(name.to_vec()).ok()?;
    (page_token_tag(listing, &name) == *tag).then_some(name)
}

/// Encode a `ListClusters` page token: [`encode_scoped_page_token`] in the
/// [`CLUSTERS_LISTING`] scope.
pub fn encode_page_token(last_returned: &str) -> String {
    encode_scoped_page_token(CLUSTERS_LISTING, last_returned)
}

/// The cluster name a page token resumes after, or `None` when the token is not
/// one `ListClusters` issued.
pub fn decode_page_token(token: &str) -> Option<String> {
    decode_scoped_page_token(CLUSTERS_LISTING, token)
}

/// The ARN of one managed capability of one cluster.
///
/// A capability is a child of its cluster, so the resource part names both, the
/// way every nested EKS resource ARN does. It is computed once, when the
/// capability is created, and stored on the record; no read recomputes it.
pub fn capability_arn(region: &str, cluster_name: &str, capability_name: &str) -> String {
    ::engine::emulator_arn(
        "eks",
        region,
        &format!("capability/{cluster_name}/{capability_name}"),
    )
}

/// Whether `name` is a capability name `CreateCapability` accepts.
///
/// The rule is the one the model states for the member: "A unique name for the
/// capability. The name must be unique within your cluster and can contain
/// alphanumeric characters, hyphens, and underscores"
/// (`CreateCapability$capabilityName`). The shape carries no `@pattern`, so the
/// boundary validator does not enforce it and the handler does.
pub fn is_valid_capability_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEksAwswesleyfrontendState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEksAwswesleyfrontendState>)
}

// ---------------------------------------------------------------------------
// The certificate-authority slice.
//
// Everything this service stores about a cluster's certificate authorities is
// collected here — the wire tokens, the spans, the record, its projections and
// the two `ClusterRecord` accessors — rather than spread through the file beside
// the other slices, so one slice is one block to read and to review. The records
// themselves still live ON the cluster
// ([`ClusterRecord::certificate_authorities`]): a CA belongs to exactly one
// cluster, and deleting the cluster takes its CAs with it.
// ---------------------------------------------------------------------------

use crate::ops::types::{
    CertificateAuthority, CertificateAuthorityScheduledEvents, CertificateAuthoritySummary,
    CertificateAuthorityValidity,
};

/// The `CertificateAuthoritySigningStatus` members this slice uses. The model's
/// enum (`smithy_query query="shape" shape="CertificateAuthoritySigningStatus"`)
/// is `NOT_USED | ACTIVATING | IN_USE`; `CertificateAuthority$signingStatus` is a
/// plain string member, so the status is stored as the wire token. `ACTIVATING`
/// belongs to `ActivateCertificateAuthority`, which is still a 501 with its own
/// gap issue, so nothing here writes it.
pub const CA_SIGNING_NOT_USED: &str = "NOT_USED";
/// See [`CA_SIGNING_NOT_USED`].
pub const CA_SIGNING_IN_USE: &str = "IN_USE";

/// The `CertificateAuthorityDistributionStatus` members this slice uses, from
/// `smithy_query query="shape" shape="CertificateAuthorityDistributionStatus"`
/// (`IN_PROGRESS | COMPLETE | FAILED | DELETING`). `FAILED` and `DELETING` are
/// written by no operation here: this emulator's distribution never fails, and
/// `DeleteCertificateAuthority` is still a 501 with its own gap issue.
pub const CA_DISTRIBUTION_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`CA_DISTRIBUTION_IN_PROGRESS`].
pub const CA_DISTRIBUTION_COMPLETE: &str = "COMPLETE";

/// The `CertificateAuthorityCreatedBy`/`CertificateAuthorityActivatedBy` members
/// (both `EKS | CUSTOMER`): "Certificate authorities that you create are
/// `CUSTOMER`; those that Amazon EKS provisions on your behalf, such as a
/// cluster's initial certificate authority, are `EKS`"
/// (`CertificateAuthority$createdBy`).
pub const CA_BY_EKS: &str = "EKS";
/// See [`CA_BY_EKS`].
pub const CA_BY_CUSTOMER: &str = "CUSTOMER";

/// How many certificate authorities one cluster holds at a time: "Each cluster
/// can have at most two certificate authorities at a time: the outgoing CA
/// that's currently signing (its signingStatus is IN_USE) and one successor CA
/// (signingStatus of NOT_USED)" (`CreateCertificateAuthority`). Appending one
/// past this ceiling is a service limit, which is what
/// `ResourceLimitExceededException` reports. It also bounds
/// [`ClusterRecord::certificate_authorities`].
pub const MAX_CERTIFICATE_AUTHORITIES_PER_CLUSTER: usize = 2;

/// How long an appended successor CA reports `IN_PROGRESS` distribution before
/// the clock owes it `COMPLETE`.
///
/// "Amazon EKS then distributes the successor CA to the Amazon Web Services
/// managed components in your cluster; you can track this through the CA's
/// distributionStatus" — that distribution IS the asynchronous update the append
/// returns, so it is owed at the same instant the update is owed its `Successful`
/// status ([`UPDATE_COMPLETION_DELAY`], itself CHOSEN rather than cited). The due
/// instant is stored on the record; nothing sleeps for it.
pub const CA_DISTRIBUTION_DELAY: Duration = UPDATE_COMPLETION_DELAY;

/// How long a certificate authority's certificate is valid for.
///
/// `CertificateAuthorityValidity` only says the structure carries the period's
/// `notBefore` and `notAfter`, and no lifetime is published for an EKS cluster
/// CA, so this span is CHOSEN (ten 365-day years) rather than cited. It is
/// recorded on the record at creation, so a read reports the same two instants
/// every time.
pub const CA_VALIDITY: Duration = Duration::from_secs(10 * 365 * 24 * 60 * 60);

/// How long before a CA's `notAfter` Amazon EKS may first auto-activate it
/// ("The earliest Unix epoch timestamp in seconds at which Amazon EKS may
/// automatically activate this certificate authority",
/// `CertificateAuthorityScheduledEvents$firstAutoActivation`). The model states
/// only that the events are "computed from its validity period"; the lead is
/// CHOSEN, and the computation is `notAfter - lead`.
pub const CA_FIRST_AUTO_ACTIVATION_LEAD: Duration = Duration::from_secs(180 * 24 * 60 * 60);
/// The lead for `finalAutoActivation` — "the Unix epoch timestamp in seconds by
/// which Amazon EKS will automatically activate this certificate authority if
/// you haven't already activated it". Shorter than
/// [`CA_FIRST_AUTO_ACTIVATION_LEAD`], and CHOSEN for the same reason.
pub const CA_FINAL_AUTO_ACTIVATION_LEAD: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// The `UpdateType` member for a certificate-authority change, from
/// `smithy_query query="shape" shape="UpdateType"`.
pub const UPDATE_TYPE_CERTIFICATE_AUTHORITY: &str = "CertificateAuthorityUpdate";
/// The `UpdateParamType` member that names the certificate authority an update
/// is about, from `smithy_query query="shape" shape="UpdateParamType"`.
pub const UPDATE_PARAM_CERTIFICATE_AUTHORITY_ID: &str = "CertificateAuthorityId";

/// The id of the `seq`th certificate authority of the cluster `cluster_arn`
/// identifies.
///
/// The model documents the member only as "the unique identifier of the
/// certificate authority", so the form is this emulator's: the UUID shape EKS
/// uses for the ids it mints (`Update$id` is documented as "a UUID"), rendered by
/// the one [`hyphenate_uuid`] every id of that shape goes through. It is derived
/// from the cluster ARN — which already carries the account, region and cluster
/// name — and the cluster's CA counter, so it is distinct per CA and stable for
/// the record's whole life. It is computed once, when the CA is created, and
/// stored on the record; no read recomputes it.
pub fn derive_certificate_authority_id(cluster_arn: &str, seq: u64) -> String {
    let digest = Sha256::digest(format!("{cluster_arn}|certificate-authority|{seq}").as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 4122 §4.4: the version nibble is 4, and the variant's two top bits are `10`.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    hyphenate_uuid(&bytes)
}

/// One certificate authority of one cluster, as this service stores it.
///
/// Everything a `CertificateAuthority` reports is held here — including the
/// server-generated parts (`id`, `createdAt`, the validity period and the
/// public-certificate `data`) — so `DescribeCertificateAuthority` returns exactly
/// what the write operation recorded rather than recomputing anything per
/// request.
#[derive(Debug, Clone)]
pub struct CertificateAuthorityRecord {
    pub id: String,
    pub created_at: SystemTime,
    pub created_by: String,
    pub activated_at: Option<SystemTime>,
    pub activated_by: Option<String>,
    pub signing_status: String,
    pub distribution_status: String,
    /// When set, the instant at which the clock owes this CA the move from
    /// `IN_PROGRESS` to `COMPLETE` distribution. Cleared once the move is
    /// materialised.
    pub distribution_complete_at: Option<SystemTime>,
    pub not_before: SystemTime,
    pub not_after: SystemTime,
    /// Until when reverting to this CA is still possible: "After you activate a
    /// successor CA, rollback lets you revert to the outgoing CA for a limited"
    /// period (`CertificateAuthority$rollbackAvailable`). Only an activation
    /// retires an outgoing CA, and `ActivateCertificateAuthority` is still a 501
    /// with its own gap issue, so no implemented operation sets this yet — a CA
    /// that nothing has superseded reports `rollbackAvailable` false, which is
    /// what this `None` means.
    pub rollback_available_until: Option<SystemTime>,
    /// The Base64-encoded public certificate of this CA
    /// (`CertificateAuthority$data`), written once and read back unchanged.
    pub data: String,
    /// The `@idempotencyToken` of the request that appended this CA, so a retry
    /// of that request is absorbed and answered with this CA.
    pub client_request_token: Option<String>,
    /// The id of the [`UpdateRecord`] that appended this CA to the cluster's trust
    /// bundle ("This is an asynchronous operation that returns an update object",
    /// `CreateCertificateAuthority`), so the append and a retry of it both answer
    /// with the update the cluster actually holds. `None` for the CA Amazon EKS
    /// provisions with the cluster, which no update appended.
    pub update_id: Option<String>,
}

impl CertificateAuthorityRecord {
    /// The cluster's initial certificate authority — the one Amazon EKS
    /// provisions on the caller's behalf, so `createdBy` is `EKS` — already
    /// signing (`IN_USE`) and already distributed (`COMPLETE`) when the cluster
    /// is created. Its `data` is the cluster's own `certificateAuthority.data`,
    /// so the two are one value rather than two copies.
    pub fn initial_for_cluster(cluster_arn: &str, resource_id: &str, now: SystemTime) -> Self {
        Self {
            id: derive_certificate_authority_id(cluster_arn, 0),
            created_at: now,
            created_by: CA_BY_EKS.to_string(),
            activated_at: Some(now),
            activated_by: Some(CA_BY_EKS.to_string()),
            signing_status: CA_SIGNING_IN_USE.to_string(),
            distribution_status: CA_DISTRIBUTION_COMPLETE.to_string(),
            distribution_complete_at: None,
            not_before: now,
            not_after: now + CA_VALIDITY,
            rollback_available_until: None,
            data: derive_certificate_authority_data(&format!("cluster id {resource_id}")),
            client_request_token: None,
            update_id: None,
        }
    }

    /// A successor CA appended to a cluster's trust bundle by the caller, so
    /// `createdBy` is `CUSTOMER`. "Appending a successor CA adds its public
    /// certificate to the cluster's trust bundle ... but it doesn't begin signing
    /// certificates", hence `NOT_USED` signing, and its distribution starts
    /// `IN_PROGRESS` with the clock owing `COMPLETE` [`CA_DISTRIBUTION_DELAY`]
    /// later.
    ///
    /// `seq` is how many CAs the cluster has ever had, so the id is distinct from
    /// every id that cluster has already used, and `update_id` names the update
    /// that appends it.
    pub fn successor(
        cluster_arn: &str,
        seq: u64,
        now: SystemTime,
        client_request_token: Option<String>,
        update_id: String,
    ) -> Self {
        let id = derive_certificate_authority_id(cluster_arn, seq);
        Self {
            data: derive_certificate_authority_data(&format!("certificate authority {id}")),
            id,
            created_at: now,
            created_by: CA_BY_CUSTOMER.to_string(),
            activated_at: None,
            activated_by: None,
            signing_status: CA_SIGNING_NOT_USED.to_string(),
            distribution_status: CA_DISTRIBUTION_IN_PROGRESS.to_string(),
            distribution_complete_at: Some(now + CA_DISTRIBUTION_DELAY),
            not_before: now,
            not_after: now + CA_VALIDITY,
            rollback_available_until: None,
            client_request_token,
            update_id: Some(update_id),
        }
    }

    /// Materialise the distribution move the clock already owes at `now`.
    ///
    /// Like a cluster's `CREATING` -> `ACTIVE` move, this was due at
    /// `distribution_complete_at` whether or not anybody called, so applying it
    /// on a read does not change what a later caller sees. The
    /// certificate-authority operations apply it to the record they report,
    /// which is the only way a CA is ever observed.
    pub fn materialize_due_transitions(&mut self, now: SystemTime) {
        if self.distribution_complete_at.is_some_and(|due| now >= due) {
            self.distribution_status = CA_DISTRIBUTION_COMPLETE.to_string();
            self.distribution_complete_at = None;
        }
    }

    /// This record as the modelled `CertificateAuthoritySummary` shape, which the
    /// certificate-authority write operations and `ListCertificateAuthorities`
    /// answer with.
    pub fn to_summary(&self) -> CertificateAuthoritySummary {
        CertificateAuthoritySummary {
            id: Some(self.id.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            created_by: Some(self.created_by.clone()),
            activated_at: self.activated_at.map(::aws_smithy_types::DateTime::from),
            activated_by: self.activated_by.clone(),
            signing_status: Some(self.signing_status.clone()),
            distribution_status: Some(self.distribution_status.clone()),
        }
    }

    /// This record as the modelled `CertificateAuthority` shape — the summary
    /// plus the validity period, the scheduled auto-activation events computed
    /// from it, whether a rollback to this CA is still available at `now`, and
    /// the public certificate.
    pub fn to_certificate_authority(&self, now: SystemTime) -> CertificateAuthority {
        let summary = self.to_summary();
        CertificateAuthority {
            id: summary.id,
            created_at: summary.created_at,
            created_by: summary.created_by,
            activated_at: summary.activated_at,
            activated_by: summary.activated_by,
            signing_status: summary.signing_status,
            distribution_status: summary.distribution_status,
            validity: Some(CertificateAuthorityValidity {
                not_before: Some(::aws_smithy_types::DateTime::from(self.not_before)),
                not_after: Some(::aws_smithy_types::DateTime::from(self.not_after)),
            }),
            scheduled_events: Some(CertificateAuthorityScheduledEvents {
                first_auto_activation: Some(::aws_smithy_types::DateTime::from(
                    self.not_after - CA_FIRST_AUTO_ACTIVATION_LEAD,
                )),
                final_auto_activation: Some(::aws_smithy_types::DateTime::from(
                    self.not_after - CA_FINAL_AUTO_ACTIVATION_LEAD,
                )),
            }),
            rollback_available: Some(
                self.rollback_available_until
                    .is_some_and(|until| now < until),
            ),
            data: Some(self.data.clone()),
        }
    }

    /// The `UpdateRecord` for the asynchronous update that appends this CA to the
    /// cluster's trust bundle: "This is an asynchronous operation that returns an
    /// update object" (`CreateCertificateAuthority`), and that object is the one
    /// `DescribeUpdate` reads back from the cluster.
    ///
    /// The update's work IS the trust distribution, so it is owed its `Successful`
    /// status at the same instant the CA is owed `COMPLETE` distribution.
    pub fn append_update_record(&self, update_id: String) -> UpdateRecord {
        UpdateRecord {
            id: update_id,
            r#type: UPDATE_TYPE_CERTIFICATE_AUTHORITY.to_string(),
            status: UPDATE_IN_PROGRESS.to_string(),
            created_at: self.created_at,
            // "A key-value map that contains the parameters associated with the
            // update": one entry, keyed by the `UpdateParamType` that names a
            // certificate authority, whose value is the CA this update appends.
            params: vec![UpdateParam {
                r#type: Some(UPDATE_PARAM_CERTIFICATE_AUTHORITY_ID.to_string()),
                value: Some(self.id.clone()),
            }],
            client_request_token: self.client_request_token.clone(),
            succeeds_at: self.distribution_complete_at,
            // This update carries no encryption change; that is
            // `AssociateEncryptionConfig`'s field.
            pending_encryption_config: None,
            // Nothing has attempted to cancel this update yet; `CancelUpdate`
            // writes both of these.
            cancellation: None,
            cancellation_request_token: None,
        }
    }
}

impl ClusterRecord {
    /// This cluster's certificate authority with `id`, if it holds one.
    pub fn certificate_authority(&self, id: &str) -> Option<&CertificateAuthorityRecord> {
        self.certificate_authorities.get(id)
    }

    /// The same record, mutably, so the operation that reports a CA can first
    /// materialise the distribution move the clock already owes it.
    pub fn certificate_authority_mut(
        &mut self,
        id: &str,
    ) -> Option<&mut CertificateAuthorityRecord> {
        self.certificate_authorities.get_mut(id)
    }

    /// The certificate authority this cluster recorded for `client_request_token`,
    /// if any — what makes a retry of the call that appended it idempotent.
    pub fn certificate_authority_for_token(
        &self,
        token: &str,
    ) -> Option<&CertificateAuthorityRecord> {
        self.certificate_authorities
            .values()
            .find(|authority| authority.client_request_token.as_deref() == Some(token))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The seeded calendar's own invariants: every published date parses, the rows
    /// are ordered and unique, and each row's dates run release -> end of standard
    /// support -> end of extended support. A typo in [`VERSION_CALENDAR`] shows up
    /// here rather than as a mis-sorted page or a status nobody can explain.
    #[test]
    fn calendar_rows_are_well_formed() {
        let calendar = seed_cluster_versions();
        assert_eq!(
            calendar.len(),
            VERSION_CALENDAR.len(),
            "case row count: expected every published row to be seeded, got {}",
            calendar.len()
        );
        let mut seen: Vec<&str> = Vec::new();
        for row in &calendar {
            let case = row.page_key();
            assert!(
                row.release_date < row.end_of_standard_support,
                "case {case}: standard support must end after the release, got {:?} and {:?}",
                row.release_date,
                row.end_of_standard_support
            );
            assert!(
                row.end_of_standard_support < row.end_of_extended_support,
                "case {case}: extended support must end after standard support, got {:?} and \
                 {:?}",
                row.end_of_standard_support,
                row.end_of_extended_support
            );
            assert!(
                !seen.contains(&row.cluster_version.as_str()),
                "case {case}: the calendar lists this version twice, rows so far {seen:?}"
            );
            assert_eq!(
                row.kubernetes_patch_version(),
                format!("{}.0", row.cluster_version),
                "case {case}: expected the initial patch level of its own minor version, got {}",
                row.kubernetes_patch_version()
            );
            seen.push(row.cluster_version.as_str());
        }
        assert!(
            seen.windows(2).all(|pair| pair[0] > pair[1]),
            "case ordering: the calendar is published newest version first, got {seen:?}"
        );
    }

    /// Exactly one row is the default version, it is the version `CreateCluster`
    /// creates when the caller names none — so the catalog and the cluster slice
    /// cannot disagree about the default — and it is the newest published row but
    /// one, which is the rule [`DEFAULT_KUBERNETES_VERSION`] states.
    #[test]
    fn exactly_one_row_is_the_default_version() {
        let calendar = seed_cluster_versions();
        let defaults: Vec<&str> = calendar
            .iter()
            .filter(|row| row.default_version)
            .map(|row| row.cluster_version.as_str())
            .collect();
        assert_eq!(
            defaults,
            vec![DEFAULT_KUBERNETES_VERSION],
            "case default row: expected exactly [{DEFAULT_KUBERNETES_VERSION}], got {defaults:?}"
        );
        let position = calendar
            .iter()
            .position(|row| row.default_version)
            .expect("the calendar marks one row as the default");
        assert_eq!(
            position, 1,
            "case default row position: the default is the newest published version but one, \
             got position {position} of {}",
            calendar.len()
        );
    }

    /// The published access-policy catalogue is sorted, unique, and every entry's
    /// ARN round-trips back to the entry's own name — which is what makes a name
    /// `ListAccessPolicies` reports usable as an `AssociateAccessPolicy`
    /// `policyArn`.
    #[test]
    fn the_access_policy_catalogue_is_sorted_and_round_trips() {
        assert!(
            ACCESS_POLICY_NAMES.windows(2).all(|pair| pair[0] < pair[1]),
            "case ordering: the catalogue must be sorted and unique, got {ACCESS_POLICY_NAMES:?}"
        );
        for name in ACCESS_POLICY_NAMES {
            let arn = access_policy_arn(name);
            assert_eq!(
                arn,
                format!("{ACCESS_POLICY_ARN_PREFIX}{name}"),
                "case {name}: expected the service-owned ARN form, got {arn}"
            );
            assert_eq!(
                access_policy_name(&arn),
                Some(*name),
                "case {name}: its own ARN must resolve back to it, got {:?}",
                access_policy_name(&arn)
            );
        }
        // And an ARN whose name the catalogue does not publish resolves to nothing,
        // so `AssociateAccessPolicy` cannot accept a policy that does not exist.
        for absent in [
            "arn:aws:eks::aws:cluster-access-policy/AmazonEKSNoSuchPolicy",
            "arn:aws:eks::aws:cluster-access-policy/",
            "arn:aws:iam::aws:policy/AmazonEKSViewPolicy",
        ] {
            assert_eq!(
                access_policy_name(absent),
                None,
                "case {absent}: expected no catalogue entry, got {:?}",
                access_policy_name(absent)
            );
        }
    }

    /// The support status is a function of the clock and the row's two
    /// end-of-support dates, in both modelled spellings.
    #[test]
    fn support_status_follows_the_clock() {
        let row = ClusterVersionRecord {
            cluster_version: "9.99".to_string(),
            cluster_type: CLUSTER_TYPE_EKS.to_string(),
            default_version: false,
            release_date: calendar_instant("2030-01-01"),
            end_of_standard_support: calendar_instant("2031-03-01"),
            end_of_extended_support: calendar_instant("2032-03-01"),
        };
        let day = Duration::from_secs(24 * 60 * 60);
        let cases = [
            ("the day before release", row.release_date - day, false, SupportStatus::StandardSupport),
            ("the release day", row.release_date, true, SupportStatus::StandardSupport),
            (
                "the day standard support ends",
                row.end_of_standard_support,
                true,
                SupportStatus::ExtendedSupport,
            ),
            (
                "the day before standard support ends",
                row.end_of_standard_support - day,
                true,
                SupportStatus::StandardSupport,
            ),
            (
                "the day extended support ends",
                row.end_of_extended_support,
                true,
                SupportStatus::Unsupported,
            ),
            (
                "a year after extended support ends",
                row.end_of_extended_support + 365 * day,
                true,
                SupportStatus::Unsupported,
            ),
        ];
        for (case, now, released, expected) in cases {
            assert_eq!(
                row.is_released(now),
                released,
                "case {case}: expected is_released {released}, got {}",
                row.is_released(now)
            );
            let actual = row.support_status(now);
            assert_eq!(
                actual, expected,
                "case {case}: expected {expected:?}, got {actual:?}"
            );
            assert_eq!(
                SupportStatus::from_version_status(actual.version_status()),
                Some(actual),
                "case {case}: the VersionStatus token must round-trip, got {}",
                actual.version_status()
            );
            assert_eq!(
                SupportStatus::from_cluster_version_status(actual.cluster_version_status()),
                Some(actual),
                "case {case}: the ClusterVersionStatus token must round-trip, got {}",
                actual.cluster_version_status()
            );
        }
    }

    /// The tag commits to the PAIR `(listing, last_returned)`, not to their
    /// concatenation: two pairs whose concatenations are equal must still get
    /// different tags, or one listing's tag could be spliced onto the other
    /// listing's payload. This is the assertion the length prefix in
    /// `page_token_tag` exists for — it fails on a digest of the bare
    /// concatenation — and the rows are the colliding pairs the empty
    /// [`CLUSTERS_LISTING`] creates for each of the other scopes.
    #[test]
    fn a_page_tag_separates_the_listing_from_the_key() {
        let colliding_pairs = [
            (
                (CLUSTERS_LISTING, "cluster-versionsmy-cluster"),
                (CLUSTER_VERSIONS_LISTING, "my-cluster"),
            ),
            (
                (CLUSTERS_LISTING, "access-policiesAmazonEKSViewPolicy"),
                (ACCESS_POLICIES_LISTING, "AmazonEKSViewPolicy"),
            ),
            (
                (CLUSTERS_LISTING, "cluster-versions"),
                (CLUSTER_VERSIONS_LISTING, ""),
            ),
        ];

        for ((left_listing, left_key), (right_listing, right_key)) in colliding_pairs {
            // The rows really are the ambiguous case: concatenating each pair gives
            // the same bytes, which is what an unprefixed digest would hash.
            assert_eq!(
                format!("{left_listing}{left_key}"),
                format!("{right_listing}{right_key}"),
                "case {left_listing:?}/{left_key:?}: the row must be a concatenation collision"
            );
            let left = page_token_tag(left_listing, left_key);
            let right = page_token_tag(right_listing, right_key);
            assert_ne!(
                left, right,
                "case {left_listing:?}/{left_key:?} vs {right_listing:?}/{right_key:?}: the tag \
                 must separate the listing from the key, got {left:?} for both"
            );
        }
    }

    /// A listing resumes from its own token and from no other listing's, over a
    /// table that spans every scope this service mints tokens under.
    #[test]
    fn a_page_token_is_scoped_to_the_listing_that_minted_it() {
        let cases = [
            (CLUSTERS_LISTING, "my-cluster"),
            (ACCESS_POLICIES_LISTING, "AmazonEKSViewPolicy"),
            (CLUSTER_VERSIONS_LISTING, DEFAULT_KUBERNETES_VERSION),
            // Keys that carry another scope's token as a prefix, so the payload a
            // listing reads back is not confused with the scope it was minted
            // under.
            (CLUSTERS_LISTING, "cluster-versionsmy-cluster"),
            (CLUSTERS_LISTING, "access-policiesAmazonEKSViewPolicy"),
        ];

        for (listing, key) in cases {
            let token = encode_scoped_page_token(listing, key);
            assert_eq!(
                decode_scoped_page_token(listing, &token),
                Some(key.to_string()),
                "case {listing:?}/{key:?}: a listing must resume from its own token"
            );
            for other in [
                CLUSTERS_LISTING,
                ACCESS_POLICIES_LISTING,
                CLUSTER_VERSIONS_LISTING,
            ] {
                if other == listing {
                    continue;
                }
                let resumed = decode_scoped_page_token(other, &token);
                assert_eq!(
                    resumed, None,
                    "case {listing:?}/{key:?}: expected listing {other:?} to refuse this token, \
                     got {resumed:?}"
                );
            }
        }
    }
}
