// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-eks-awswesleyfrontend service.
//!
//! This holds the cluster lifecycle slice: `CreateCluster` and `RegisterCluster`
//! write a [`ClusterRecord`] here, and `DescribeCluster`, `ListClusters`,
//! `TagResource`, `UntagResource`, `ListTagsForResource` and `DeleteCluster` all
//! compute their responses from it. Nothing in `src/ops/` keeps a second copy of a cluster.
//!
//! It also holds the managed node group slice: `CreateNodegroup` writes a
//! [`NodegroupRecord`] here and `DescribeNodegroup` computes its whole response
//! from that record, so a node group's server-generated parts (its ARN,
//! `createdAt`, defaults) are written once and read back unchanged.
//!
//! It also holds the Fargate profile slice: `CreateFargateProfile` writes a
//! [`FargateProfileRecord`] here and `DescribeFargateProfile` computes its whole
//! response from that record, so a profile's server-generated parts (its ARN and
//! `createdAt`) are written once and read back unchanged. A profile lives INSIDE
//! a cluster, so its key carries the cluster's key and `DeleteCluster` drops the
//! profiles of the cluster it removes.
//!
//! One record covers both kinds of cluster this API knows: an Amazon EKS cluster
//! `CreateCluster` provisions a control plane for, and an external Kubernetes
//! cluster `RegisterCluster` connects. The members only a provisioned control
//! plane has (`endpoint`, `roleArn`, `resourcesVpcConfig`, `version`,
//! `platformVersion`) are optional on the record, because a connected cluster
//! reports none of them — it reports [`ClusterRecord::connector_config`] instead.
//!
//! It also holds the add-on slice: `CreateAddon` writes an [`AddonRecord`] here,
//! `DescribeAddon` and `ListAddons` read it back and `DeleteAddon` removes it
//! ([`ComAmazonawsEksAwswesleyfrontendState::remove_addon`]). An add-on lives
//! INSIDE a cluster, so its
//! key carries the cluster's key and `DeleteCluster` drops the add-ons of the
//! cluster it removes. `UpdateAddon` does not write the change it is asked for
//! onto that record straight away: the update it starts is asynchronous, so the
//! change waits on the record as a [`PendingAddonUpdate`] until the clock
//! completes that update.
//!
//! It also holds the managed-capability slice that hangs off a cluster:
//! `CreateCapability` writes a [`CapabilityRecord`], and `DescribeCapability`,
//! `ListCapabilities` and `DeleteCapability` compute their responses from it.
//! `UpdateCapability` does not write the change it is asked for onto that record
//! straight away: the update it starts is asynchronous, so the change waits on
//! the record as a [`PendingCapabilityUpdate`] until the clock completes that
//! update — the same way an add-on update waits.
//!
//! It also holds the EKS Anywhere subscription slice
//! ([`EksAnywhereSubscriptionRecord`]), which hangs off NO cluster: a
//! subscription is an account-level support contract, so
//! `CreateEksAnywhereSubscription` writes one into
//! [`ComAmazonawsEksAwswesleyfrontendState::eks_anywhere_subscriptions`] and
//! `DescribeEksAnywhereSubscription` computes its whole response from that
//! record.
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
//! It also holds the add-on catalog ([`AddonCatalogRecord`]), seeded once for
//! the same reason: `DescribeAddonVersions` reports it, `DescribeAddonConfiguration`
//! answers one entry of it, and `CreateAddon` takes the version it installs from
//! it when the caller names none, so the versions a caller can pick and the
//! versions an installed add-on reports have one source.
//!
//! An asynchronous cluster update lives ON the cluster it updates
//! ([`ClusterRecord::updates`]), so `AssociateEncryptionConfig` and
//! `AssociateIdentityProviderConfig` write one there, `DescribeUpdate` reads it
//! back and `CancelUpdate` records its cancellation on it; deleting the cluster
//! drops its updates with it rather than leaving them behind in a second map.
//!
//! The on-demand cluster insights refresh lives on the cluster too
//! ([`ClusterRecord::insights_refresh`]): `StartInsightsRefresh` writes the
//! [`InsightsRefreshRecord`] for the refresh it begins and
//! `DescribeInsightsRefresh` reports that same record, so there is one place a
//! cluster's latest refresh is held and deleting the cluster takes it with it.
//!
//! A cluster's INSIGHTS are not stored beside it: an insight is the result of a
//! check Amazon EKS runs against a cluster, so [`InsightCheck`] declares the checks
//! this service can evaluate and
//! [`ComAmazonawsEksAwswesleyfrontendState::cluster_insights`] evaluates every one of
//! them from the cluster record, the node groups and add-ons inside it and the add-on
//! catalog. `ListInsights` and `DescribeInsight` both report that one walk. The only
//! part of an insight the store keeps is the instant of the latest completed analysis
//! ([`ClusterRecord::insights_analyzed_at`]), which the clock writes when it completes
//! a refresh.
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
    AccessConfigResponse, AccessEntry, AccessScope, Addon, AddonHealth, AddonInfo,
    AddonIssue,
    AddonNamespaceConfigResponse, AddonPodIdentityAssociations, AddonPodIdentityConfiguration,
    AddonVersionInfo,
    ArgoCdNetworkAccessConfigResponse, ArgoCdRoleMapping,
    AssociatedAccessPolicy, BadRequestException, Cancellation,
    Capability, Compatibility,
    CapabilityConfigurationResponse, CapabilityHealth, CapabilityIssue, CapabilitySummary,
    Certificate, ClientException, Cluster, ClusterVersionInformation, ComputeConfigResponse,
    ConnectorConfigResponse,
    ControlPlaneScalingConfig, EksAnywhereSubscription, EksAnywhereSubscriptionTerm,
    EncryptionConfig, FargateProfile, FargateProfileHealth,
    FargateProfileSelector, Identity, KubeApiServerConfigResponse,
    KubeControllerManagerConfigResponse, KubeSchedulerConfigResponse,
    InsightStatus, InsightSummary, Insight, InsightResourceDetail,
    KubernetesNetworkConfigResponse, LaunchTemplateSpecification, Logging, MarketplaceInformation,
    NodeRepairConfig,
    Nodegroup, NodegroupHealth, NodegroupScalingConfig, NodegroupUpdateConfig,
    OidcIdentityProviderConfig, OutpostConfigResponse, PodIdentityAssociation,
    PodIdentityAssociationSummary, RemoteAccessConfig, RemoteNetworkConfigResponse,
    ResourceInUseException, ResourceNotFoundException, StorageConfigResponse, Taint, Update,
    UpdateParam,
    UpgradePolicyResponse, VpcConfigResponse, WarmPoolConfig, ZonalShiftConfigResponse,
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

/// How many asynchronous updates one cluster's history keeps.
///
/// Amazon EKS publishes no limit on how many updates `ListUpdates` can report, so
/// this is CHOSEN to bound the store rather than cited: `UpdateClusterConfig`
/// mints one record per accepted configuration change, so without a cap a caller
/// that keeps changing the configuration would grow this map once per request.
/// Large enough that a caller's recent update history is reported exactly, and
/// enforced by evicting the oldest TERMINAL update
/// ([`ClusterRecord::record_update`]).
pub const MAX_UPDATES_PER_CLUSTER: usize = 50;

/// The `UpdateType` members a cluster configuration change reports
/// (`smithy_query query="shape" shape="UpdateType"`), one per CATEGORY of
/// configuration `UpdateClusterConfig` can change: an `Update` carries a single
/// `type`, so the type is the category of the change the call asked for.
pub const UPDATE_TYPE_LOGGING: &str = "LoggingUpdate";
/// See [`UPDATE_TYPE_LOGGING`] — the endpoint access half of
/// `resourcesVpcConfig` (public/private access and the public access CIDRs).
pub const UPDATE_TYPE_ENDPOINT_ACCESS: &str = "EndpointAccessUpdate";
/// See [`UPDATE_TYPE_LOGGING`] — the VPC half of `resourcesVpcConfig` (the
/// subnets and security groups the control plane uses).
pub const UPDATE_TYPE_VPC_CONFIG: &str = "VpcConfigUpdate";
/// See [`UPDATE_TYPE_LOGGING`] — `resourcesVpcConfig.controlPlaneEgressMode`.
pub const UPDATE_TYPE_CONTROL_PLANE_EGRESS: &str = "ControlPlaneEgressUpdate";
/// See [`UPDATE_TYPE_LOGGING`] — `accessConfig`.
pub const UPDATE_TYPE_ACCESS_CONFIG: &str = "AccessConfigUpdate";
/// See [`UPDATE_TYPE_LOGGING`] — `upgradePolicy`.
pub const UPDATE_TYPE_UPGRADE_POLICY: &str = "UpgradePolicyUpdate";
/// See [`UPDATE_TYPE_LOGGING`] — `zonalShiftConfig`.
pub const UPDATE_TYPE_ZONAL_SHIFT_CONFIG: &str = "ZonalShiftConfigUpdate";
/// See [`UPDATE_TYPE_LOGGING`] — the three EKS Auto Mode capabilities
/// (`computeConfig`, `storageConfig` and `kubernetesNetworkConfig`'s
/// `elasticLoadBalancing`), which are one category because they are the
/// capabilities Auto Mode manages.
pub const UPDATE_TYPE_AUTO_MODE: &str = "AutoModeUpdate";
/// See [`UPDATE_TYPE_LOGGING`] — `remoteNetworkConfig`.
pub const UPDATE_TYPE_REMOTE_NETWORK_CONFIG: &str = "RemoteNetworkConfigUpdate";
/// See [`UPDATE_TYPE_LOGGING`] — `deletionProtection`.
pub const UPDATE_TYPE_DELETION_PROTECTION: &str = "DeletionProtectionUpdate";
/// See [`UPDATE_TYPE_LOGGING`] — `controlPlaneScalingConfig`.
pub const UPDATE_TYPE_CONTROL_PLANE_SCALING_CONFIG: &str = "ControlPlaneScalingConfigUpdate";
/// See [`UPDATE_TYPE_LOGGING`] — the three Kubernetes control plane component
/// configurations (`kubeApiServerConfig`, `kubeSchedulerConfig` and
/// `kubeControllerManagerConfig`).
pub const UPDATE_TYPE_CONTROL_PLANE_COMPONENT_CONFIG: &str = "ControlPlaneComponentConfigUpdate";
/// See [`UPDATE_TYPE_LOGGING`] — the generic cluster configuration update, which
/// is what one call changing members of MORE than one category reports: the
/// change is a configuration update that no single narrower type names.
pub const UPDATE_TYPE_CONFIG: &str = "ConfigUpdate";

/// The `UpdateParamType` keys a cluster configuration change reports its
/// requested values under (`smithy_query query="shape"
/// shape="UpdateParamType"`). One key per member the request can carry, so a
/// caller reading the update back sees which values it asked to change.
pub const UPDATE_PARAM_CLUSTER_LOGGING: &str = "ClusterLogging";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_ENDPOINT_PUBLIC_ACCESS: &str = "EndpointPublicAccess";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_ENDPOINT_PRIVATE_ACCESS: &str = "EndpointPrivateAccess";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_PUBLIC_ACCESS_CIDRS: &str = "PublicAccessCidrs";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_SUBNETS: &str = "Subnets";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_SECURITY_GROUPS: &str = "SecurityGroups";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_CONTROL_PLANE_EGRESS_MODE: &str = "ControlPlaneEgressMode";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_AUTHENTICATION_MODE: &str = "AuthenticationMode";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_UPGRADE_POLICY: &str = "UpgradePolicy";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_ZONAL_SHIFT_CONFIG: &str = "ZonalShiftConfig";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_COMPUTE_CONFIG: &str = "ComputeConfig";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_STORAGE_CONFIG: &str = "StorageConfig";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_KUBERNETES_NETWORK_CONFIG: &str = "KubernetesNetworkConfig";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_REMOTE_NETWORK_CONFIG: &str = "RemoteNetworkConfig";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_DELETION_PROTECTION: &str = "DeletionProtection";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`] — the tier a scaling change moves the
/// control plane TO.
pub const UPDATE_PARAM_UPDATED_TIER: &str = "UpdatedTier";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`] — the tier it moves FROM.
pub const UPDATE_PARAM_PREVIOUS_TIER: &str = "PreviousTier";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_KUBE_API_SERVER_CONFIG: &str = "KubeApiServerConfig";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_KUBE_SCHEDULER_CONFIG: &str = "KubeSchedulerConfig";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_KUBE_CONTROLLER_MANAGER_CONFIG: &str = "KubeControllerManagerConfig";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`] — the Kubernetes version a version
/// update moves the cluster to.
pub const UPDATE_PARAM_VERSION: &str = "Version";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`] — the platform version the cluster runs
/// at the Kubernetes version a version update moves it to.
pub const UPDATE_PARAM_PLATFORM_VERSION: &str = "PlatformVersion";

/// The `UpdateParamType` keys a node group VERSION update reports the members it
/// changes under (`smithy_query query="shape" shape="UpdateParamType"`).
pub const UPDATE_PARAM_RELEASE_VERSION: &str = "ReleaseVersion";
/// See [`UPDATE_PARAM_RELEASE_VERSION`].
pub const UPDATE_PARAM_LAUNCH_TEMPLATE_NAME: &str = "LaunchTemplateName";
/// See [`UPDATE_PARAM_RELEASE_VERSION`].
pub const UPDATE_PARAM_LAUNCH_TEMPLATE_VERSION: &str = "LaunchTemplateVersion";

/// The `ControlPlaneEgressModeType` members
/// (`smithy_query query="shape" shape="ControlPlaneEgressModeType"`). "Once set
/// to `CUSTOMER_ROUTED`, this setting cannot be changed back to `AWS_MANAGED` on
/// the same cluster" (`VpcConfigRequest$controlPlaneEgressMode`), which is what
/// `UpdateClusterConfig` enforces with them.
pub const CONTROL_PLANE_EGRESS_AWS_MANAGED: &str = "AWS_MANAGED";
/// See [`CONTROL_PLANE_EGRESS_AWS_MANAGED`].
pub const CONTROL_PLANE_EGRESS_CUSTOMER_ROUTED: &str = "CUSTOMER_ROUTED";

/// The `AuthenticationMode` members, in the order a cluster may move through
/// them (`smithy_query query="shape" shape="AuthenticationMode"`).
///
/// A cluster can only move TOWARDS the API-based mode: "you can switch from
/// `CONFIG_MAP` to `API_AND_CONFIG_MAP` and from `API_AND_CONFIG_MAP` to `API`,
/// but you can't switch back" (Amazon EKS User Guide, "Grant IAM users access to
/// Kubernetes with EKS access entries"), so the index of a mode in this list is
/// how far along that one-way ladder it is.
pub const AUTHENTICATION_MODE_LADDER: &[&str] = &["CONFIG_MAP", "API_AND_CONFIG_MAP", "API"];

/// How far along [`AUTHENTICATION_MODE_LADDER`] `mode` is, or `None` when it is
/// not one of the modelled modes.
pub fn authentication_mode_rank(mode: &str) -> Option<usize> {
    AUTHENTICATION_MODE_LADDER
        .iter()
        .position(|known| *known == mode)
}

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
/// See [`CAPABILITY_STATUS_CREATING`] — the status a capability reports while the
/// asynchronous update `UpdateCapability` started is still in flight: "When you
/// update a capability, Amazon EKS applies the changes and may restart capability
/// components as needed" (`UpdateCapability`).
pub const CAPABILITY_STATUS_UPDATING: &str = "UPDATING";

/// The `CapabilityStatus` members a capability can take an update FROM: the ones in
/// which it has SETTLED.
///
/// A capability still being created or already being deleted is not settled, so it
/// cannot take a change; one that is running — healthy or `DEGRADED` — can.
pub const CAPABILITY_STATUSES_AN_UPDATE_STARTS_FROM: &[&str] =
    &[CAPABILITY_STATUS_ACTIVE, "DEGRADED"];

/// The `UpdateType` an `UpdateCapability` call reports on the `Update` it answers
/// with (`smithy_query query="shape" shape="UpdateType"`).
pub const UPDATE_TYPE_CAPABILITY: &str = "CapabilityUpdate";

/// The `UpdateParamType` keys a capability update reports the members it changes
/// under (`smithy_query query="shape" shape="UpdateParamType"`).
pub const UPDATE_PARAM_ROLE_ARN: &str = "RoleArn";
/// See [`UPDATE_PARAM_ROLE_ARN`].
pub const UPDATE_PARAM_ROLE_MAPPINGS_TO_ADD_OR_UPDATE: &str = "RoleMappingsToAddOrUpdate";
/// See [`UPDATE_PARAM_ROLE_ARN`].
pub const UPDATE_PARAM_ROLE_MAPPINGS_TO_REMOVE: &str = "RoleMappingsToRemove";
/// See [`UPDATE_PARAM_ROLE_ARN`].
pub const UPDATE_PARAM_NETWORK_ACCESS: &str = "NetworkAccess";
/// See [`UPDATE_PARAM_ROLE_ARN`].
pub const UPDATE_PARAM_ENABLE_CROSS_NAMESPACE: &str = "EnableCrossNamespace";
/// See [`UPDATE_PARAM_ROLE_ARN`].
pub const UPDATE_PARAM_DISABLED_SERVICES: &str = "DisabledServices";

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

/// The `InsightsRefreshStatus` members an insights refresh reports. The model's
/// enum (`smithy_query query="shape" shape="InsightsRefreshStatus"`) is
/// `IN_PROGRESS | FAILED | COMPLETED`, and the status is stored as the wire token,
/// as every other status in this service is. `FAILED` is the terminal status of a
/// refresh whose analysis did not finish; nothing in this emulator fails one, so no
/// record is written with it — it is named here because it is what makes
/// `IN_PROGRESS` the only NON-terminal status of the three.
pub const INSIGHTS_REFRESH_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`INSIGHTS_REFRESH_IN_PROGRESS`].
pub const INSIGHTS_REFRESH_COMPLETED: &str = "COMPLETED";
/// See [`INSIGHTS_REFRESH_IN_PROGRESS`].
pub const INSIGHTS_REFRESH_FAILED: &str = "FAILED";

/// How long an on-demand insights refresh reports `IN_PROGRESS` before the clock
/// owes it `COMPLETED`.
///
/// `StartInsightsRefresh` publishes no duration for the analysis it starts
/// ("Initiates an on-demand refresh operation for cluster insights, getting the
/// latest analysis outside of the standard refresh schedule"), so this span is
/// CHOSEN rather than cited, and matches [`CLUSTER_ACTIVATION_DELAY`]: long enough
/// that a caller which describes the refresh straight after starting it still
/// observes `IN_PROGRESS` — and the conflict that goes with it — short enough to be
/// worth waiting for. The due instant is stored on the record; nothing sleeps for
/// it.
pub const INSIGHTS_REFRESH_DURATION: Duration = Duration::from_millis(500);

/// The `Category` members a cluster insight reports. The model's enum
/// (`smithy_query query="shape" shape="Category"`) is `UPGRADE_READINESS |
/// MISCONFIGURATION | ROLLBACK_READINESS`, and the category is reported as the wire
/// token, as every other enum member in this service is.
pub const INSIGHT_CATEGORY_UPGRADE_READINESS: &str = "UPGRADE_READINESS";
/// See [`INSIGHT_CATEGORY_UPGRADE_READINESS`].
pub const INSIGHT_CATEGORY_MISCONFIGURATION: &str = "MISCONFIGURATION";
/// See [`INSIGHT_CATEGORY_UPGRADE_READINESS`].
pub const INSIGHT_CATEGORY_ROLLBACK_READINESS: &str = "ROLLBACK_READINESS";

/// The `InsightStatusValue` members a check's result reports. The model's enum
/// (`smithy_query query="shape" shape="InsightStatusValue"`) is `PASSING | WARNING |
/// ERROR | UNKNOWN`: `PASSING` is a check that found nothing, `WARNING` and `ERROR`
/// a check that found something, and `UNKNOWN` a check that could not be evaluated
/// because the state it examines is not there.
pub const INSIGHT_STATUS_PASSING: &str = "PASSING";
/// See [`INSIGHT_STATUS_PASSING`].
pub const INSIGHT_STATUS_WARNING: &str = "WARNING";
/// See [`INSIGHT_STATUS_PASSING`].
pub const INSIGHT_STATUS_ERROR: &str = "ERROR";
/// See [`INSIGHT_STATUS_PASSING`].
pub const INSIGHT_STATUS_UNKNOWN: &str = "UNKNOWN";

/// The `CapabilityType` members whose `configuration` this service maps from the
/// request shape to the response shape. The model's enum
/// (`smithy_query query="shape" shape="CapabilityType"`) is `ACK | ARGOCD | KRO`,
/// and `CapabilityConfigurationRequest` carries a member for exactly these two:
/// `KRO` takes no configuration.
pub const CAPABILITY_TYPE_ACK: &str = "ACK";
/// See [`CAPABILITY_TYPE_ACK`].
pub const CAPABILITY_TYPE_ARGOCD: &str = "ARGOCD";

/// The sentence `CreateCapability` refuses a `type` outside the modelled
/// `CapabilityType` value set with (`crate::ops::types::CAPABILITY_TYPE_VALUES`).
///
/// FIXED TEXT in its entirety: it has no variable part and does not echo the value it
/// rejected, because the request named no capability type at all — there is nothing of
/// this service to name. The refusal is worded where it has to be decided, before the
/// cluster the path names is resolved (see
/// [`crate::service::off_enum_member_contract`]).
pub const INVALID_CAPABILITY_TYPE_MESSAGE: &str = "Invalid capability type";

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

/// The platform version this service publishes for a Kubernetes version.
///
/// EKS platform versions are `eks.<n>`, the n-th platform revision Amazon EKS has
/// published for one Kubernetes minor version. Neither the model nor the EKS
/// documentation states which revision a version sits at — it is whatever the
/// service's catalogue publishes — and this emulator patches no control plane, so
/// it publishes ONE revision, the one the recorded catalogue publishes.
///
/// It is the single source for both places a platform version appears, so they
/// cannot disagree: `DescribeClusterVersions` reports it as a row's
/// `defaultPlatformVersion` ("Default platform version for this Kubernetes
/// version"), and `CreateCluster` stamps it on the control plane of a cluster it
/// creates — a new cluster runs the default platform version of its version.
pub const PLATFORM_VERSION: &str = "eks.4";

/// The `(major, minor)` release a Kubernetes minor-version string names, or
/// `None` for a string that is not one.
///
/// Every Kubernetes version this API carries is a MINOR version — `major.minor`,
/// e.g. the rows of [`VERSION_CALENDAR`] — so the pair this returns is what
/// orders two of them and what says whether one is the release right after
/// another. A string with a third component, a non-numeric component or no `.`
/// at all names no Kubernetes minor version.
pub fn kubernetes_minor_version(version: &str) -> Option<(u32, u32)> {
    let (major, minor) = version.split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// The Kubernetes minor version right AFTER `version` — the one a cluster running
/// `version` would move to, because "a cluster moves one minor version per update"
/// (`UpdateClusterVersion`). `None` for a string that names no minor version.
pub fn next_kubernetes_minor_version(version: &str) -> Option<String> {
    let (major, minor) = kubernetes_minor_version(version)?;
    Some(format!("{major}.{}", minor.saturating_add(1)))
}

/// The Kubernetes minor version right BEFORE `version` — the one a cluster running
/// `version` would roll back to. `None` for a string that names no minor version,
/// and for the first minor version of a major release, which has no earlier one.
pub fn previous_kubernetes_minor_version(version: &str) -> Option<String> {
    let (major, minor) = kubernetes_minor_version(version)?;
    minor.checked_sub(1).map(|earlier| format!("{major}.{earlier}"))
}

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

/// The sentence a tag request earns when the tags it would leave on the resource
/// exceed the number one resource may carry.
///
/// FIXED TEXT with `limit` as its only variable part: it names neither the resource
/// nor how many tags the request asked for, because the ceiling is a property of the
/// API and not of the request that happened to reach it. The one place this wording
/// lives, so the two paths that can reach it cannot disagree — the bound the model
/// declares on a single request's collection, refused before dispatch
/// (`crate::service::tag_contract`), and the ACCUMULATED map's own ceiling, refused
/// by the `TagResource` handler, which is the same ceiling counted after the merge.
#[must_use]
pub fn tag_ceiling_message(limit: usize) -> String {
    format!("The resultant tag set must not have more than {limit} user tags.")
}

/// How much of a caller-supplied value a refusal this service words may echo.
///
/// A refusal interpolates the value that was refused, so the caller can see what it
/// sent, and the caller chooses that value's length — so an unbounded echo would
/// amplify one request into an arbitrarily long response. The framework bounds its
/// own echo of a refused value at the same length.
pub const MAX_ECHOED_VALUE: usize = 256;

/// `value` elided at `limit` CHARACTERS, on a character boundary, with the elision
/// marked.
///
/// The one form every refusal this service words echoes a caller-supplied value in,
/// so two of them cannot elide the same value differently. RENDERING ONLY: each
/// caller judges the ORIGINAL value and uses this solely to interpolate, since
/// eliding before the check would judge text the caller never sent.
#[must_use]
pub fn elided_value(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    let kept: String = value.chars().take(limit).collect();
    format!("{kept}…")
}

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

/// The `UpdateType` a Kubernetes version update that moves the cluster FORWARD
/// reports — the other half of the pair [`UPDATE_TYPE_VERSION_ROLLBACK`] is the
/// backward one of (`smithy_query query="shape" shape="UpdateType"`).
pub const UPDATE_TYPE_VERSION: &str = "VersionUpdate";

/// The smallest and largest `RollbackConfig$timeoutMinutes` a version rollback
/// may ask for: "This value can be between 120 (2 hours) and 10080 (7 days)"
/// (`RollbackConfig$timeoutMinutes`). The model declares the member as a plain
/// integer, so the published range is enforced by the handler.
pub const MIN_ROLLBACK_TIMEOUT_MINUTES: i32 = 120;
/// See [`MIN_ROLLBACK_TIMEOUT_MINUTES`].
pub const MAX_ROLLBACK_TIMEOUT_MINUTES: i32 = 10080;

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

/// The `UpdateParamType` keys a node group configuration update reports the
/// members it changes under (`smithy_query query="shape"
/// shape="UpdateParamType"`).
pub const UPDATE_PARAM_LABELS_TO_ADD: &str = "LabelsToAdd";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_LABELS_TO_REMOVE: &str = "LabelsToRemove";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_TAINTS_TO_ADD: &str = "TaintsToAdd";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_TAINTS_TO_REMOVE: &str = "TaintsToRemove";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_MIN_SIZE: &str = "MinSize";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_MAX_SIZE: &str = "MaxSize";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_DESIRED_SIZE: &str = "DesiredSize";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_MAX_UNAVAILABLE: &str = "MaxUnavailable";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_MAX_UNAVAILABLE_PERCENTAGE: &str = "MaxUnavailablePercentage";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_UPDATE_STRATEGY: &str = "UpdateStrategy";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_NODE_REPAIR_CONFIG: &str = "NodeRepairConfig";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_WARM_POOL_ENABLED: &str = "WarmPoolEnabled";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_WARM_POOL_MIN_SIZE: &str = "WarmPoolMinSize";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_WARM_POOL_MAX_GROUP_PREPARED_CAPACITY: &str =
    "WarmPoolMaxGroupPreparedCapacity";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_WARM_POOL_STATE: &str = "WarmPoolState";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_WARM_POOL_REUSE_ON_SCALE_IN: &str = "WarmPoolReuseOnScaleIn";

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

/// The `FargateProfileStatus` members this slice uses. The model's enum
/// (`smithy_query query="shape" shape="FargateProfileStatus"`) is
/// `CREATING | ACTIVE | DELETING | CREATE_FAILED | DELETE_FAILED`;
/// `FargateProfile$status` carries that enum and this slice stores the wire
/// token.
///
/// Its own constants rather than the cluster slice's [`STATUS_CREATING`]:
/// `FargateProfileStatus` and `ClusterStatus` are different enums that only
/// happen to share these two tokens, so a change to one must not silently
/// change the other — the same reasoning as [`ADDON_STATUS_CREATING`].
pub const FARGATE_PROFILE_STATUS_CREATING: &str = "CREATING";
/// See [`FARGATE_PROFILE_STATUS_CREATING`].
pub const FARGATE_PROFILE_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`FARGATE_PROFILE_STATUS_CREATING`] — the status a profile reports once
/// `DeleteFargateProfile` has taken it off its cluster, which is the final status
/// that call answers with.
pub const FARGATE_PROFILE_STATUS_DELETING: &str = "DELETING";

/// How long a newly created Fargate profile reports `CREATING` before the clock
/// owes it `ACTIVE`.
///
/// `CreateFargateProfile` publishes no duration for the provisioning, so this
/// span is CHOSEN rather than cited — the same reasoning as
/// [`CLUSTER_ACTIVATION_DELAY`]: long enough that a caller which describes the
/// profile straight after creating it still observes `CREATING`, short enough to
/// be worth waiting for. The due instant is stored on the record; nothing sleeps
/// for it.
pub const FARGATE_PROFILE_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// The number of selectors one Fargate profile may carry.
///
/// "Each profile can have up to five selectors that contain a namespace and
/// labels" (`CreateFargateProfile`), restated on the member itself: "You may
/// specify up to five selectors in a Fargate profile"
/// (`CreateFargateProfileRequest$selectors`). The model expresses no length
/// bound for the list, so the handler is what enforces it.
pub const MAX_SELECTORS_PER_FARGATE_PROFILE: usize = 5;

/// The number of Fargate profiles one cluster holds before
/// `CreateFargateProfile` answers `ResourceLimitExceededException`.
///
/// AWS publishes an adjustable EKS service quota of 10 Fargate profiles per
/// cluster, and exceeding a service limit is exactly what that error reports —
/// the same reading as [`MAX_NODEGROUPS_PER_CLUSTER`].
pub const MAX_FARGATE_PROFILES_PER_CLUSTER: usize = 10;

/// The number of Fargate profiles this store holds ACROSS EVERY CLUSTER AND
/// REGION before `CreateFargateProfile` answers
/// `ResourceLimitExceededException`, for the reason
/// [`MAX_STORED_NODEGROUPS`] spells out: the caller decides how many clusters
/// and regions exist, so the per-cluster quota alone does not bound the store.
pub const MAX_STORED_FARGATE_PROFILES: usize = MAX_FARGATE_PROFILES_PER_CLUSTER * 100;

/// The number of capabilities one cluster can hold.
///
/// It is not a separate quota: `CreateCapability` documents "You can only create one
/// Capability of each type on a given Amazon EKS cluster", so the per-type
/// uniqueness rule the handler enforces caps a cluster at one capability per
/// `CapabilityType`. Named here so the bound on the store is stated where the store
/// is, and so a test can assert it: with clusters bounded by
/// [`MAX_STORED_CLUSTERS`], the capability map is bounded too.
pub const MAX_CAPABILITIES_PER_CLUSTER: usize = crate::ops::types::CAPABILITY_TYPE_VALUES.len();

/// The GENERIC not-found sentence: the message an operation that resolves a
/// resource INSIDE a cluster reports, whichever half of the pair is missing.
///
/// Such an operation addresses ONE resource (the `(cluster, child)` pair), so it
/// does not tell the two apart the way the operations whose subject IS the cluster
/// do ([`cluster_not_found_message`]). The model documents no message text for
/// `ResourceNotFoundException`, so a client branches on the error CODE, never on
/// this string.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "The requested resource does not exist.";

/// The message an operation that reports the CLUSTER as the resource it could not
/// resolve carries, built from the request: "No cluster found for name: <name>."
///
/// `DescribeCluster` is the plainest case; every operation that only validates the
/// cluster before creating in it or enumerating it reports the same sentence, as the
/// recorded corpus shows for `CreateAddon`, `CreateAccessEntry` and
/// `ListAccessEntries`.
pub fn cluster_not_found_message(cluster_name: &str) -> String {
    format!("No cluster found for name: {cluster_name}.")
}

/// The `ResourceNotFoundException` payload of an operation that reports the CLUSTER
/// as the resource it could not resolve.
///
/// RULE (one template, two forms — see [`resource_not_found`] for the other): the
/// exception's optional identity members are `clusterName`, `nodegroupName`,
/// `fargateProfileName`, `addonName` and `subscriptionId`, and EKS never echoes the
/// identifier of a resource INSIDE a cluster into them. It names only the resource
/// whose absence it reports: this form sets `clusterName`, and nothing else, beside
/// the message that names the same cluster.
pub fn cluster_not_found(cluster_name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        cluster_name: Some(cluster_name.to_string()),
        message: Some(cluster_not_found_message(cluster_name)),
        ..Default::default()
    }
}

/// The `ResourceNotFoundException` payload of an operation that resolves a resource
/// INSIDE a cluster: NO identity member at all, and the generic sentence
/// ([`RESOURCE_NOT_FOUND_MESSAGE`]).
///
/// See [`cluster_not_found`] for the rule. The missing half — the cluster or the
/// child — is not disclosed, so nothing about either is serialized; that is what the
/// recorded corpus shows for the reads and deletes of an add-on, a capability, a node
/// group, an access entry and an identity-provider configuration.
pub fn resource_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(RESOURCE_NOT_FOUND_MESSAGE.to_string()),
        ..Default::default()
    }
}

/// The same memberless payload as [`resource_not_found`], for the operations that
/// report the missing CHILD in a sentence of their own (an update id, a certificate
/// authority id, a pod identity association id).
///
/// The cluster is not the resource being reported, so `clusterName` stays unset here
/// too — see [`cluster_not_found`] for the rule.
pub fn child_resource_not_found(message: String) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(message),
        ..Default::default()
    }
}

/// The sentence this service refuses a `(addonName, addonVersion)` pair the
/// add-on catalog publishes no build of with.
///
/// `DescribeAddonConfiguration` resolves that pair as ONE published build ("The
/// version must match one of the versions returned by `DescribeAddonVersions`"),
/// so whichever half of it the catalog does not carry, the build the caller
/// asked about does not exist — and this service states a resource it could not
/// resolve in one sentence, [`RESOURCE_NOT_FOUND_MESSAGE`], naming neither half
/// (see [`resource_not_found`], the same form the reads of a resource inside a
/// cluster carry).
pub fn addon_not_found() -> ResourceNotFoundException {
    resource_not_found()
}

/// The sentence an operation addressed to ONE EKS Anywhere subscription refuses an
/// `id` the Region carries no subscription for with.
///
/// A subscription is not a resource inside a cluster, and this service states the
/// resource it could not resolve in the terms of the resource the operation addresses:
/// the subscription reads answer their own fixed sentence, which names nothing from
/// the request (see [`subscription_not_found`]).
pub const SUBSCRIPTION_NOT_FOUND_MESSAGE: &str = "subscription not found.";

/// The `ResourceNotFoundException` payload an operation addressed to ONE EKS Anywhere
/// subscription answers for an id the Region carries no subscription for.
///
/// NO identity member at all: `ResourceNotFoundException$subscriptionId` names a
/// subscription the service RESOLVED, and this refusal resolved none — the same half
/// of the rule [`resource_not_found`] carries for a resource inside a cluster (see
/// [`cluster_not_found`] for the whole rule). A caller reading that member to learn
/// whether the service identified a concrete subscription must not find the id it
/// just sent echoed back there.
pub fn subscription_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(SUBSCRIPTION_NOT_FOUND_MESSAGE.to_string()),
        ..Default::default()
    }
}

/// The sentence every Amazon EKS-managed-plane operation refuses a cluster this
/// service does not manage with.
///
/// A cluster `RegisterCluster` connected is an external Kubernetes cluster: it has no
/// Amazon EKS control plane and none of the resources Amazon EKS manages inside one
/// (add-ons, node groups, Fargate profiles, access entries, identity-provider
/// configurations, pod identity associations, capabilities, certificate authorities,
/// control-plane updates, cluster insights). The refusal is PERMANENT — it is what
/// tells a client the cluster is externally managed and to stop — so it is one fixed
/// sentence, naming nothing from the request.
pub const CONNECTED_CLUSTER_REFUSAL_MESSAGE: &str =
    "This operation is not supported for connected clusters";

/// The `ClientException` payload every EKS-managed-plane operation answers for a
/// cluster [`ClusterRecord::is_connected`] reports.
///
/// `ClientException` is "usually caused by a client action... using an action or
/// resource on behalf of an IAM principal that doesn't have permissions to use the
/// action or resource or specifying an identifier that is not valid" — the modelled
/// class for "this action does not apply to the resource you named", which is exactly
/// what a connected cluster is for these operations. It is deliberately NOT
/// `InvalidRequestException` ("the request is invalid given the STATE of the
/// cluster"), which a client or waiter reads as transient and retries.
///
/// Shared here so no two operations can word or classify the gate differently.
pub fn connected_cluster_refusal(cluster_name: &str) -> ClientException {
    ClientException {
        cluster_name: Some(cluster_name.to_string()),
        message: Some(CONNECTED_CLUSTER_REFUSAL_MESSAGE.to_string()),
        ..Default::default()
    }
}

/// The `ResourceInUseException` payload every operation that starts a cluster
/// update is refused with while that cluster is already running one.
///
/// A cluster runs at most one update at a time ([`ClusterRecord::in_flight_update`]),
/// so the refusal is a fact about the CLUSTER rather than about whatever the call
/// named inside it, and the recorded service words it as "Cannot <UpdateType>
/// because cluster <clusterName> currently has update <updateId> in progress": the
/// fixed text states WHICH kind of update was refused (`update_type`, the
/// `UPDATE_TYPE_*` token the call would have minted), the cluster the request named,
/// and the id of the update already running — the id a client keys its retry on, by
/// polling `DescribeUpdate` until that update leaves `InProgress`.
///
/// Shared here so no two operations can word or classify the gate differently.
pub fn update_in_progress(
    update_type: &str,
    cluster_name: &str,
    in_flight: &UpdateRecord,
) -> ResourceInUseException {
    let update_id = &in_flight.id;
    ResourceInUseException {
        cluster_name: Some(cluster_name.to_string()),
        message: Some(format!(
            "Cannot {update_type} because cluster {cluster_name} currently has update \
             {update_id} in progress"
        )),
        ..Default::default()
    }
}

/// This service's ARN namespace: the third field of every ARN its resources carry
/// (`aws.api#service$arnNamespace` is `eks`, as
/// `smithy_query query="service_info"` reports, and
/// [`::engine::emulator_arn`] is given the same token wherever this crate mints one).
pub const ARN_NAMESPACE: &str = "eks";

/// The sentence a `resourceArn` that is not a resource ARN of this service is refused
/// with.
///
/// `BadRequestException` is "thrown if the request contains a semantic error. The
/// precise meaning will depend on the API, and will be documented in the error
/// message", so the message is the service's own wording for THIS refusal — a syntax
/// failure on the input — and it names nothing from the request: what a client
/// branches on is the 400 and the error code.
pub const INVALID_RESOURCE_ARN_MESSAGE: &str = "Invalid input resource arn: bad syntax";

/// Whether `value` is syntactically a resource ARN OF THIS SERVICE: six well-formed
/// ARN fields ([`::service_common::arn`]) whose vendor field is [`ARN_NAMESPACE`].
///
/// An operation whose input binds a caller-chosen `resourceArn` reads it with this
/// before it looks anything up: a value that is not an ARN, or is an ARN of another
/// vendor, addresses nothing this API can name, so it is a bad REQUEST rather than a
/// resource that happens to be absent. A well-formed ARN of this namespace that no
/// stored resource owns is the absent case, and stays a not-found.
#[must_use]
pub fn is_eks_resource_arn(value: &str) -> bool {
    ::service_common::arn::is_resource_arn_of(value, ARN_NAMESPACE)
}

/// The `BadRequestException` payload for a `resourceArn` [`is_eks_resource_arn`]
/// rejects, shared by every operation that binds one so they cannot disagree on the
/// refusal.
#[must_use]
pub fn invalid_resource_arn() -> BadRequestException {
    BadRequestException {
        message: Some(INVALID_RESOURCE_ARN_MESSAGE.to_string()),
    }
}

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
/// See [`CONFIG_STATUS_CREATING`] — the status a configuration reports while the
/// asynchronous update `DisassociateIdentityProviderConfig` started is still in
/// flight, before that update takes it off the cluster.
pub const CONFIG_STATUS_DELETING: &str = "DELETING";

/// The `UpdateType` an `AssociateIdentityProviderConfig` call reports on the
/// `Update` it answers with (`smithy_query query="shape" shape="UpdateType"`).
pub const UPDATE_TYPE_ASSOCIATE_IDENTITY_PROVIDER_CONFIG: &str = "AssociateIdentityProviderConfig";

/// The `UpdateType` a `DisassociateIdentityProviderConfig` call reports on the
/// `Update` it answers with (`smithy_query query="shape" shape="UpdateType"`).
pub const UPDATE_TYPE_DISASSOCIATE_IDENTITY_PROVIDER_CONFIG: &str =
    "DisassociateIdentityProviderConfig";

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
/// (`ListAccessPolicies` reports it, and `AssociateAccessPolicy` checks the
/// `policyArn` it is handed against it, so it is also what BOUNDS the records an
/// access entry can accumulate).
///
/// Source for the rows: the EKS "Access policy permissions" reference, whose table
/// of Amazon EKS access policies lists the whole managed catalogue — the four
/// policies whose permissions mirror the Kubernetes `cluster-admin`, `admin`,
/// `edit` and `view` cluster roles that `AccessPolicy`'s own documentation points
/// at, the read-only mirror of `admin`, the policies Amazon EKS publishes for the
/// controllers and Amazon Web Services services that work on a cluster (EKS Auto
/// Mode's node lifecycle, the EKS compute, block-storage, load-balancing and
/// networking controllers — each published both namespace-scoped and
/// cluster-scoped — hybrid nodes, the AWS Controllers for Kubernetes, Argo CD,
/// AWS Batch, the AI operations and DevOps agents and Application Recovery
/// Controller's region switch), and the policies for the cluster capabilities EKS
/// manages (event forwarding, the Kube Resource Orchestrator, EKS Pod Identity
/// and secret admin/reader access), service-account impersonation, and the
/// policies Amazon EKS publishes for the Amazon Web Services services that
/// orchestrate workloads ON a cluster — Amazon EMR on EKS (its virtual-cluster
/// controller and its job runs) and Amazon SageMaker HyperPod (its cluster and
/// controller access plus its inference, space and space-template components).
/// The catalogue is what a caller discovers
/// through `ListAccessPolicies` INSTEAD of hardcoding ARNs, so a policy missing
/// here is a policy no caller can associate.
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
    // controllers, plus hybrid nodes. Each of the four Auto Mode capability
    // controllers is published TWICE — the namespace-scoped policy the controller
    // is granted on the workloads it manages (`AmazonEKS<Capability>Policy`) and
    // the cluster-scoped one it is granted on the cluster's own objects
    // (`AmazonEKS<Capability>ClusterPolicy`) — so a caller that discovers the
    // catalogue finds both halves of the pair rather than only one.
    "AmazonEKSAutoNodePolicy",
    "AmazonEKSBlockStorageClusterPolicy",
    "AmazonEKSBlockStoragePolicy",
    // `cluster-admin`.
    "AmazonEKSClusterAdminPolicy",
    "AmazonEKSComputeClusterPolicy",
    "AmazonEKSComputePolicy",
    // `edit`.
    "AmazonEKSEditPolicy",
    // The policies Amazon EKS publishes for the capabilities a cluster can run:
    // event forwarding, the Kube Resource Orchestrator, EKS Pod Identity, and
    // read/write and read-only access to Kubernetes secrets.
    "AmazonEKSEventPolicy",
    "AmazonEKSHybridPolicy",
    "AmazonEKSKROPolicy",
    "AmazonEKSLoadBalancingClusterPolicy",
    "AmazonEKSLoadBalancingPolicy",
    "AmazonEKSNetworkingClusterPolicy",
    "AmazonEKSNetworkingPolicy",
    "AmazonEKSPodIdentityPolicy",
    "AmazonEKSSecretAdminPolicy",
    "AmazonEKSSecretReaderPolicy",
    // The policy Amazon EKS publishes for impersonating a Kubernetes service
    // account from an IAM principal.
    "AmazonEKSServiceAccountImpersonationPolicy",
    // `view`.
    "AmazonEKSViewPolicy",
    // Amazon EMR on EKS: the access a virtual cluster's controller is granted on
    // the cluster, and the access an EMR job run is granted in its namespace.
    "AmazonEMRClusterPolicy",
    "AmazonEMRJobPolicy",
    // Amazon SageMaker HyperPod, whose orchestration runs on an EKS cluster: the
    // cluster- and controller-scoped access its control plane is granted, plus the
    // access the inference, space and space-template components are granted.
    "AmazonSagemakerHyperpodClusterPolicy",
    "AmazonSagemakerHyperpodControllerPolicy",
    "AmazonSagemakerHyperpodInferencePolicy",
    "AmazonSagemakerHyperpodSpacePolicy",
    "AmazonSagemakerHyperpodSpaceTemplatePolicy",
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
    /// its updates. A `BTreeMap` so a listing walks a stable, sorted order.
    ///
    /// BOUNDED at [`MAX_UPDATES_PER_CLUSTER`] entries, which
    /// [`ClusterRecord::record_update`] enforces by evicting the oldest TERMINAL
    /// update as it appends. Some minters are self-limiting on their own —
    /// `AssociateEncryptionConfig` is refused once encryption is enabled,
    /// `CreateCertificateAuthority` once the cluster holds
    /// [`MAX_CERTIFICATE_AUTHORITIES_PER_CLUSTER`] certificate authorities, and
    /// `AssociateIdentityProviderConfig` mints at most one update per
    /// configuration it associates — but `UpdateClusterConfig` can be called
    /// again for every configuration change a caller makes, so the history needs
    /// a bound of its own rather than relying on its minters. With that cap per
    /// cluster, [`MAX_STORED_CLUSTERS`] bounds the whole store.
    pub updates: BTreeMap<String, UpdateRecord>,
    /// The latest on-demand cluster insights refresh of this cluster, if one has
    /// ever been started.
    ///
    /// It lives on the cluster record because a refresh is addressed only through
    /// its cluster (`/clusters/{clusterName}/insights-refresh`) and a cluster runs
    /// at most ONE on-demand refresh — "the status of the LATEST on-demand cluster
    /// insights refresh operation" (`DescribeInsightsRefresh`) — so a single slot
    /// is what the API exposes: starting another replaces the terminal one it
    /// reports. Deleting the cluster takes it with it, and nothing else keeps a
    /// second copy.
    pub insights_refresh: Option<InsightsRefreshRecord>,
    /// When set, the instant of the latest COMPLETED analysis of this cluster's
    /// insights — `InsightSummary$lastRefreshTime`, read through
    /// [`ClusterRecord::insights_refreshed_at`].
    ///
    /// It is separate from [`Self::insights_refresh`] because that slot holds the
    /// LATEST refresh operation, whichever status it is in: starting a new one
    /// replaces the completed record it reported before, and an analysis that
    /// completed is not undone by the next refresh being started. Only the clock
    /// writes it, when it completes a refresh
    /// ([`ClusterRecord::materialize_due_transitions`]), so it never moves backwards
    /// and a read never changes it.
    pub insights_analyzed_at: Option<SystemTime>,
}

/// One on-demand cluster insights refresh, as this service stores it.
///
/// Everything `DescribeInsightsRefresh` reports is computed from here, so the
/// status and the instants a caller reads back are the ones the call that started
/// the refresh wrote plus the move the clock owed it — never recomputed per read.
#[derive(Debug, Clone)]
pub struct InsightsRefreshRecord {
    /// One of [`INSIGHTS_REFRESH_IN_PROGRESS`], [`INSIGHTS_REFRESH_COMPLETED`] or
    /// [`INSIGHTS_REFRESH_FAILED`], stored as the wire token.
    pub status: String,
    /// "The date and time when the insights refresh operation started"
    /// (`DescribeInsightsRefreshResponse$startedAt`).
    pub started_at: SystemTime,
    /// "The date and time when the insights refresh operation ended" — unset while
    /// the refresh is still `IN_PROGRESS`, because it has not ended yet.
    pub ended_at: Option<SystemTime>,
    /// When set, the instant at which the clock owes this refresh the move from
    /// `IN_PROGRESS` to `COMPLETED` (see [`INSIGHTS_REFRESH_DURATION`]). Cleared
    /// once the move is materialised.
    pub completes_at: Option<SystemTime>,
}

impl InsightsRefreshRecord {
    /// The record a refresh started at `now` begins as: in progress, with the move
    /// to `COMPLETED` owed [`INSIGHTS_REFRESH_DURATION`] later.
    pub fn started(now: SystemTime) -> Self {
        Self {
            status: INSIGHTS_REFRESH_IN_PROGRESS.to_string(),
            started_at: now,
            ended_at: None,
            completes_at: Some(now + INSIGHTS_REFRESH_DURATION),
        }
    }

    /// Whether this refresh is still running, i.e. in the one non-terminal
    /// `InsightsRefreshStatus`. This is what a second `StartInsightsRefresh` is
    /// refused on.
    pub fn is_in_progress(&self) -> bool {
        self.status == INSIGHTS_REFRESH_IN_PROGRESS
    }

    /// Materialise the clock-owed completion of this refresh.
    ///
    /// The analysis was finished at `completes_at` whether or not anybody called,
    /// so `endedAt` is that DUE instant rather than the instant of the call that
    /// noticed it: two callers reading the completed refresh see the same end time.
    pub fn materialize_due_transitions(&mut self, now: SystemTime) {
        if let Some(due) = self.completes_at.filter(|due| now >= *due) {
            self.status = INSIGHTS_REFRESH_COMPLETED.to_string();
            self.ended_at = Some(due);
            self.completes_at = None;
        }
    }

    /// "The message associated with the insights refresh operation"
    /// (`DescribeInsightsRefreshResponse$message`): what this refresh is doing,
    /// derived from its status so the message a caller reads never disagrees with
    /// the status beside it. The model documents no text for it, so the sentence is
    /// this emulator's own.
    pub fn message(&self) -> String {
        match self.status.as_str() {
            INSIGHTS_REFRESH_COMPLETED => {
                "The cluster insights refresh completed.".to_string()
            }
            INSIGHTS_REFRESH_FAILED => "The cluster insights refresh failed.".to_string(),
            _ => "The cluster insights refresh is in progress.".to_string(),
        }
    }
}

/// One check Amazon EKS runs against a cluster, which is what ONE insight of that
/// cluster is.
///
/// `ListInsights` "Returns a list of all insights CHECKED FOR against the specified
/// cluster", so an insight is not a record a caller writes: it is the result of a
/// check, evaluated against the cluster and the resources inside it. This enum is the
/// set of checks this service holds the state to evaluate, and
/// [`ComAmazonawsEksAwswesleyfrontendState::cluster_insights`] evaluates every one of
/// them for one cluster. A check is a fixed part of the API surface, like the version
/// calendar and the add-on catalog, so it is declared here rather than stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsightCheck {
    /// Whether the cluster's managed node groups all run the cluster's own
    /// Kubernetes version, which is what an upgrade of the control plane needs.
    NodegroupVersions,
    /// Whether every add-on installed on the cluster is published for the Kubernetes
    /// version the cluster would upgrade to.
    AddonVersions,
    /// Whether the remote networks the cluster's hybrid nodes need are both
    /// configured.
    HybridNodeNetworks,
    /// Whether the cluster has a completed Kubernetes version update its version can
    /// be rolled back to.
    VersionRollback,
}

impl InsightCheck {
    /// Every check this service evaluates, in the order a listing reports them.
    pub fn all() -> [Self; 4] {
        [
            Self::NodegroupVersions,
            Self::AddonVersions,
            Self::HybridNodeNetworks,
            Self::VersionRollback,
        ]
    }

    /// The stable key this check is identified by inside a cluster, which is what
    /// its insight id is derived from (see [`derive_insight_id`]). It never travels
    /// on the wire: an insight is addressed by its id.
    pub fn key(self) -> &'static str {
        match self {
            Self::NodegroupVersions => "node-group-kubernetes-versions",
            Self::AddonVersions => "addon-version-compatibility",
            Self::HybridNodeNetworks => "hybrid-node-remote-networks",
            Self::VersionRollback => "cluster-version-rollback",
        }
    }

    /// The `Category` this check's insight is reported under.
    ///
    /// The first two are `UPGRADE_READINESS` — "Amazon EKS identifies issues that
    /// could impact your ability to upgrade to new versions of Kubernetes"
    /// (`ListInsights`) — the hybrid-nodes check is `MISCONFIGURATION`, "a
    /// misconfiguration in your EKS Hybrid Nodes setup that could impair
    /// functionality of your cluster or workloads", and the rollback check is the
    /// third member of the enum, which is what a rollback-blocking readiness issue
    /// is reported under (`UpdateClusterVersion$force` "override[s] upgrade-blocking
    /// or rollback-blocking readiness checks").
    pub fn category(self) -> &'static str {
        match self {
            Self::NodegroupVersions | Self::AddonVersions => {
                INSIGHT_CATEGORY_UPGRADE_READINESS
            }
            Self::HybridNodeNetworks => INSIGHT_CATEGORY_MISCONFIGURATION,
            Self::VersionRollback => INSIGHT_CATEGORY_ROLLBACK_READINESS,
        }
    }

    /// `InsightSummary$name`, "The name of the insight": what the check is called.
    pub fn name(self) -> &'static str {
        match self {
            Self::NodegroupVersions => "Managed node group Kubernetes version skew",
            Self::AddonVersions => "Amazon EKS add-on version compatibility",
            Self::HybridNodeNetworks => "Hybrid nodes remote network configuration",
            Self::VersionRollback => "Cluster version rollback readiness",
        }
    }

    /// `InsightSummary$description`, "The description of the insight which includes
    /// alert criteria, remediation recommendation, and additional resources": what
    /// the check looks at and what to do about a finding.
    pub fn description(self) -> &'static str {
        match self {
            Self::NodegroupVersions => {
                "Checks whether every managed node group of the cluster runs the cluster's \
                 Kubernetes version. A node group left on an older version blocks an update of \
                 the cluster's version; update the node group before updating the cluster."
            }
            Self::AddonVersions => {
                "Checks whether the installed version of each Amazon EKS add-on is published \
                 for the Kubernetes version the cluster would be updated to. Update an add-on \
                 to a version compatible with that Kubernetes version before updating the \
                 cluster."
            }
            Self::HybridNodeNetworks => {
                "Checks the remote networks configured for the cluster's hybrid nodes. Pods \
                 running on hybrid nodes are only reachable from the rest of the cluster when \
                 the remote pod networks they run in are configured alongside the remote node \
                 networks."
            }
            Self::VersionRollback => {
                "Checks whether the cluster has a completed Kubernetes version update whose \
                 version it can be rolled back from. A cluster that has never completed a \
                 version update has no earlier version to roll back to."
            }
        }
    }

    /// `Insight$recommendation`, "A recommendation to remediate the issue": what to
    /// do about a finding of this check.
    pub fn recommendation(self) -> &'static str {
        match self {
            Self::NodegroupVersions => {
                "Update every managed node group to the cluster's Kubernetes version, or pass \
                 force on the cluster version update to override the readiness check."
            }
            Self::AddonVersions => {
                "Update each add-on to a version published for the Kubernetes version the \
                 cluster is being updated to before updating the cluster."
            }
            Self::HybridNodeNetworks => {
                "Add the remote pod networks the cluster's hybrid nodes run pods in to the \
                 cluster's remote network configuration."
            }
            Self::VersionRollback => {
                "Complete a Kubernetes version update of the cluster before attempting to roll \
                 its version back."
            }
        }
    }
}

/// One insight of one cluster, as this service computes it.
///
/// Everything the modelled `InsightSummary` reports is here: the parts that are a
/// property of the CHECK come from [`InsightView::check`], and the parts that are a
/// property of this cluster's state are computed from that state. It carries no
/// behaviour of its own beyond rendering, so `ListInsights` and `DescribeInsight`
/// report the same values for the same insight.
#[derive(Debug, Clone)]
pub struct InsightView {
    /// `InsightSummary$id`, derived from the cluster and the check (see
    /// [`derive_insight_id`]) rather than minted, because no call creates an insight.
    pub id: String,
    /// Which check this insight is the result of.
    pub check: InsightCheck,
    /// `InsightSummary$kubernetesVersion`, "The Kubernetes version associated with
    /// the insight": the version the check is about, which is unset for a check that
    /// is about no particular version.
    pub kubernetes_version: Option<String>,
    /// The `InsightStatusValue` the check produced.
    pub status: String,
    /// `InsightStatus$reason`, "The explanation of the insight which includes
    /// additional details": what the check found, named from this cluster's state.
    pub reason: String,
    /// `InsightSummary$lastRefreshTime`, "The time Amazon EKS last successfully
    /// completed a refresh of this insight check on the cluster".
    pub last_refresh_time: SystemTime,
    /// `InsightSummary$lastTransitionTime`, "The time the status of the insight last
    /// changed".
    pub last_transition_time: SystemTime,
}

impl InsightView {
    /// This insight as the modelled `InsightSummary` shape.
    pub fn to_summary(&self) -> InsightSummary {
        InsightSummary {
            id: Some(self.id.clone()),
            name: Some(self.check.name().to_string()),
            category: Some(self.check.category().to_string()),
            kubernetes_version: self.kubernetes_version.clone(),
            last_refresh_time: Some(::aws_smithy_types::DateTime::from(self.last_refresh_time)),
            last_transition_time: Some(::aws_smithy_types::DateTime::from(
                self.last_transition_time,
            )),
            description: Some(self.check.description().to_string()),
            insight_status: Some(InsightStatus {
                status: Some(self.status.clone()),
                reason: Some(self.reason.clone()),
            }),
        }
    }

    /// This insight as the modelled `Insight` shape — the whole insight
    /// `DescribeInsight` reports, which is the summary plus the check's
    /// recommendation and the resources it examined.
    ///
    /// `additionalInfo` ("summary information about deprecated resource usage") and
    /// `categorySpecificSummary` are left unset: both report the Kubernetes API usage
    /// and deprecation data an API server collects, and this emulator runs no
    /// Kubernetes control plane to collect any — so there is nothing to report rather
    /// than a number to invent.
    pub fn to_insight(&self, resources: Vec<InsightResourceDetail>) -> Insight {
        let summary = self.to_summary();
        Insight {
            id: summary.id,
            name: summary.name,
            category: summary.category,
            kubernetes_version: summary.kubernetes_version,
            last_refresh_time: summary.last_refresh_time,
            last_transition_time: summary.last_transition_time,
            description: summary.description,
            insight_status: summary.insight_status,
            recommendation: Some(self.check.recommendation().to_string()),
            additional_info: None,
            resources: Some(resources),
            category_specific_summary: None,
        }
    }
}

/// What evaluating one [`InsightCheck`] against one cluster produced: the parts of an
/// [`InsightView`] that depend on the cluster's state rather than on the check.
struct InsightEvaluation {
    status: &'static str,
    reason: String,
    kubernetes_version: Option<String>,
    last_transition_time: SystemTime,
}

/// The id of one insight of one cluster.
///
/// An insight is the result of a check rather than a resource a call creates, so
/// there is no create to mint an id at: it is DERIVED from the cluster's own ARN —
/// which already carries the account, the Region and the cluster name — and the
/// check's [`InsightCheck::key`]. So the id is stable for as long as the cluster
/// lives, two checks of one cluster never share one, and the same check of two
/// clusters does not either. Rendered in the 8-4-4-4-12 hex form AWS insight ids
/// take, the same way [`derive_association_external_id`] renders its own.
pub fn derive_insight_id(cluster_arn: &str, check_key: &str) -> String {
    let digest = Sha256::digest(format!("insight/{cluster_arn}/{check_key}").as_bytes());
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

/// Which resource of a cluster an [`UpdateRecord`] is an update OF.
///
/// An update is always addressed through its cluster
/// (`/clusters/{name}/updates/...`), but the resource it belongs to may be one
/// INSIDE that cluster: `ListUpdates` and `DescribeUpdate` narrow to it with
/// `nodegroupName`, `addonName` or `capabilityName` — "Lists the updates
/// associated with an Amazon EKS resource" (`ListUpdates`) — so every stored
/// update records WHICH resource it is an update of, and a listing reports only
/// the updates of the resource the request names.
///
/// Every operation that mints an update of the cluster's own control plane
/// records [`UpdateScope::Cluster`]; the other variants are what an update of a
/// resource INSIDE the cluster carries, which is what `UpdateAddon` records for
/// the add-on it updates and what the minters of the remaining kinds
/// (`UpdateNodegroupVersion`, `UpdateNodegroupConfig`, `UpdateCapability`, each
/// still a scaffold with its own gap issue) will record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum UpdateScope {
    /// The cluster's own control plane: the scope a request that names no node
    /// group, add-on or capability lists.
    #[default]
    Cluster,
    /// One managed node group of the cluster, by node group name.
    Nodegroup(String),
    /// One installed add-on of the cluster, by add-on name.
    Addon(String),
    /// One managed capability of the cluster, by capability name.
    Capability(String),
}

/// The sentence this service refuses a second EKS Pod Identity association for
/// one `(namespace, service account)` pair with.
///
/// A pair with two associations makes the credential lookup it resolves
/// ambiguous, which is why every writer of an association refuses one — the
/// `CreatePodIdentityAssociation` call, the `CreateAddon` install and the
/// `UpdateAddon` update that owns associations, and the completion of that update
/// — so the wording lives here once rather than in each of them.
#[must_use]
pub fn association_already_exists(namespace: &str, service_account: &str) -> String {
    format!(
        "Association already exists with namespace: {namespace} and service account: \
         {service_account}"
    )
}

/// The sentence a request that names more than one of the members selecting a
/// resource inside a cluster is refused with.
///
/// `ListUpdates` and `DescribeUpdate` each read the updates of ONE resource, so
/// `nodegroupName`, `addonName` and `capabilityName` are alternatives rather than
/// filters that compose.
pub const ONE_UPDATE_SCOPE_MESSAGE: &str =
    "Only one of nodegroupName, addonName and capabilityName can be specified.";

/// Which resource of a cluster a request's `nodegroupName`, `addonName` and
/// `capabilityName` select the updates of, or `None` when it names more than one
/// (see [`ONE_UPDATE_SCOPE_MESSAGE`]).
///
/// Shared by every operation that selects updates, so the scope a request names
/// cannot be resolved two ways.
#[must_use]
pub fn update_scope_of(
    nodegroup_name: Option<&str>,
    addon_name: Option<&str>,
    capability_name: Option<&str>,
) -> Option<UpdateScope> {
    match (nodegroup_name, addon_name, capability_name) {
        (None, None, None) => Some(UpdateScope::Cluster),
        (Some(nodegroup), None, None) => Some(UpdateScope::Nodegroup(nodegroup.to_string())),
        (None, Some(addon), None) => Some(UpdateScope::Addon(addon.to_string())),
        (None, None, Some(capability)) => Some(UpdateScope::Capability(capability.to_string())),
        _ => None,
    }
}

/// One asynchronous update of a cluster, as this service stores it.
///
/// Everything the modelled `Update` reports is held here, so `DescribeUpdate`
/// returns what the call that started the update wrote, and what a later
/// `CancelUpdate` recorded on it, rather than recomputing either. Three fields
/// are not reported: `succeeds_at` is the instant the clock owes this update its
/// `Successful` status, `pending_config` is the change applied to the cluster at
/// that same instant, and `cancellation_request_token` is what makes a retried
/// cancel request idempotent.
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
    /// The change this update puts on its cluster when it completes — the
    /// `encryptionConfig` an `AssociateEncryptionConfig` carries, the
    /// configuration members an `UpdateClusterConfig` carries, or the signer
    /// promotion an `ActivateCertificateAuthority` carries. Taken when
    /// applied. `None` on an update whose work is not a change to the cluster
    /// record itself (associating an identity provider, appending a certificate
    /// authority), which writes its own resource instead.
    pub pending_config: Option<PendingClusterConfig>,
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
    /// Which resource of the cluster this is an update OF — see [`UpdateScope`].
    /// It is not reported: the modelled `Update` carries no member for it, and it
    /// is what SELECTS the update instead (`ListUpdates$nodegroupName` and its
    /// siblings).
    pub scope: UpdateScope,
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

/// The change an in-flight update will put on its cluster when it completes.
///
/// A cluster update is asynchronous — "Cluster updates are asynchronous... During
/// an update, the cluster status moves to UPDATING... When the update is complete
/// ... the cluster status moves to Active" (`UpdateClusterConfig`) — so the
/// requested values are held HERE until the clock completes the update, and only
/// then written onto the [`ClusterRecord`]. One struct for every member a change
/// can carry, so an update holds exactly one pending change rather than a field
/// per operation that mints one.
///
/// A member left `None` is a member this change does not touch: applying the
/// change leaves whatever the cluster already reports for it.
#[derive(Debug, Clone, Default)]
pub struct PendingClusterConfig {
    pub logging: Option<Logging>,
    pub resources_vpc_config: Option<VpcConfigResponse>,
    pub kubernetes_network_config: Option<KubernetesNetworkConfigResponse>,
    pub access_config: Option<AccessConfigResponse>,
    pub upgrade_policy: Option<UpgradePolicyResponse>,
    pub zonal_shift_config: Option<ZonalShiftConfigResponse>,
    pub compute_config: Option<ComputeConfigResponse>,
    pub storage_config: Option<StorageConfigResponse>,
    pub remote_network_config: Option<RemoteNetworkConfigResponse>,
    pub deletion_protection: Option<bool>,
    pub control_plane_scaling_config: Option<ControlPlaneScalingConfig>,
    pub kube_api_server_config: Option<KubeApiServerConfigResponse>,
    pub kube_scheduler_config: Option<KubeSchedulerConfigResponse>,
    pub kube_controller_manager_config: Option<KubeControllerManagerConfigResponse>,
    /// The `encryptionConfig` an `AssociateEncryptionConfig` update enables. It
    /// is a list rather than an `Option`-wrapped member of the cluster record,
    /// so an empty list here would mean "enable nothing"; the `Option` is what
    /// distinguishes a change that does not touch encryption at all.
    pub encryption_config: Option<Vec<EncryptionConfig>>,
    /// The Kubernetes version an `UpdateClusterVersion` update moves the cluster
    /// to — "the desired Kubernetes version following a successful update"
    /// (`UpdateClusterVersionRequest$version`), so the cluster reports the new
    /// version only once the clock completes the update.
    pub version: Option<String>,
    /// The certificate-authority promotion an `ActivateCertificateAuthority`
    /// update makes when it completes. It is a change to the cluster's CA
    /// records rather than to a configuration member of the cluster itself, so it
    /// carries its own structure; see [`CertificateAuthorityActivation`].
    pub certificate_authority_activation: Option<CertificateAuthorityActivation>,
    /// The certificate authority a `DeleteCertificateAuthority` update removes
    /// from the cluster's trust bundle when it completes: "Deleting a certificate
    /// authority removes its public certificate from the cluster's trust bundle",
    /// and "this is an asynchronous operation that returns an update object", so
    /// the record stays on the cluster reporting `DELETING` distribution until the
    /// clock completes that update.
    pub certificate_authority_removal: Option<String>,
    /// The identity provider configuration a
    /// `DisassociateIdentityProviderConfig` update takes off the cluster when it
    /// completes, by name. The disassociation is asynchronous, so the record
    /// stays on the cluster reporting [`CONFIG_STATUS_DELETING`] until the clock
    /// completes that update.
    pub identity_provider_config_removal: Option<String>,
}

impl PendingClusterConfig {
    /// Write this change onto `record`, which is what completing the update does.
    pub fn apply(self, record: &mut ClusterRecord) {
        if let Some(logging) = self.logging {
            record.logging = Some(logging);
        }
        if let Some(vpc_config) = self.resources_vpc_config {
            record.resources_vpc_config = Some(vpc_config);
        }
        if let Some(network_config) = self.kubernetes_network_config {
            record.kubernetes_network_config = Some(network_config);
        }
        if let Some(access_config) = self.access_config {
            record.access_config = Some(access_config);
        }
        if let Some(upgrade_policy) = self.upgrade_policy {
            record.upgrade_policy = Some(upgrade_policy);
        }
        if let Some(zonal_shift_config) = self.zonal_shift_config {
            record.zonal_shift_config = Some(zonal_shift_config);
        }
        if let Some(compute_config) = self.compute_config {
            record.compute_config = Some(compute_config);
        }
        if let Some(storage_config) = self.storage_config {
            record.storage_config = Some(storage_config);
        }
        if let Some(remote_network_config) = self.remote_network_config {
            record.remote_network_config = Some(remote_network_config);
        }
        if let Some(deletion_protection) = self.deletion_protection {
            record.deletion_protection = deletion_protection;
        }
        if let Some(scaling_config) = self.control_plane_scaling_config {
            record.control_plane_scaling_config = Some(scaling_config);
        }
        if let Some(kube_api_server_config) = self.kube_api_server_config {
            record.kube_api_server_config = Some(kube_api_server_config);
        }
        if let Some(kube_scheduler_config) = self.kube_scheduler_config {
            record.kube_scheduler_config = Some(kube_scheduler_config);
        }
        if let Some(controller_manager_config) = self.kube_controller_manager_config {
            record.kube_controller_manager_config = Some(controller_manager_config);
        }
        if let Some(encryption_config) = self.encryption_config {
            record.encryption_config = encryption_config;
        }
        if let Some(version) = self.version {
            record.version = Some(version);
        }
        if let Some(activation) = self.certificate_authority_activation {
            activation.apply(record);
        }
        if let Some(removed) = self.certificate_authority_removal {
            record.certificate_authorities.remove(&removed);
        }
        if let Some(removed) = self.identity_provider_config_removal {
            record.identity_provider_configs.remove(&removed);
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

        // The on-demand insights refresh this cluster is running: its analysis was
        // owed its completion at the stored instant whether or not anybody called,
        // and completing it is what makes its end the instant the cluster's insights
        // were last analysed at.
        if let Some(refresh) = self.insights_refresh.as_mut() {
            refresh.materialize_due_transitions(now);
            if refresh.status == INSIGHTS_REFRESH_COMPLETED {
                let completed_at = refresh.ended_at;
                self.insights_analyzed_at = self
                    .insights_analyzed_at
                    .into_iter()
                    .chain(completed_at)
                    .max();
            }
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
            let scope = update.scope.clone();
            let pending = update.pending_config.take();
            if let Some(pending) = pending {
                pending.apply(self);
            }
            // An update of the cluster's own control plane only starts from an
            // `ACTIVE` cluster and took it out of that status to make the change,
            // so completing one hands the cluster back to the status it was taken
            // from. An update of a resource INSIDE the cluster never moved the
            // cluster's status, so completing it does not write one.
            if scope == UpdateScope::Cluster {
                self.status = STATUS_ACTIVE.to_string();
            }
        }
    }

    /// Whether this record is an external cluster `RegisterCluster` connected,
    /// rather than an Amazon EKS cluster `CreateCluster` provisioned.
    pub fn is_connected(&self) -> bool {
        self.connector_config.is_some()
    }

    /// The instant of the latest COMPLETED analysis of this cluster's insights —
    /// `InsightSummary$lastRefreshTime` for every insight of the cluster.
    ///
    /// Amazon EKS analyses a cluster's insights when it creates the cluster and again
    /// whenever a refresh of them completes, so the latest analysis is the on-demand
    /// refresh the cluster has completed, or the cluster's own creation when it has
    /// completed none. A refresh that is still `IN_PROGRESS` has not produced an
    /// analysis yet, which is why it does not move this instant until the clock ends
    /// it ([`InsightsRefreshRecord::materialize_due_transitions`]), and starting
    /// another refresh does not take a completed analysis back: the latest of the
    /// instant the clock recorded on the cluster and the one the refresh it still
    /// holds completed at is the latest analysis.
    pub fn insights_refreshed_at(&self) -> SystemTime {
        let from_latest_refresh = self
            .insights_refresh
            .as_ref()
            .filter(|refresh| refresh.status == INSIGHTS_REFRESH_COMPLETED)
            .and_then(|refresh| refresh.ended_at);
        self.insights_analyzed_at
            .into_iter()
            .chain(from_latest_refresh)
            .max()
            .unwrap_or(self.created_at)
    }

    /// The instant at which the configuration of this cluster's own control plane
    /// last changed: its creation, or the latest configuration update it has taken.
    ///
    /// An insight whose subject is a member of the cluster's configuration reports
    /// this as its `lastTransitionTime`, because the status of such a check can only
    /// have changed when that configuration did. `UpdateClusterConfig` is the one
    /// writer of those members after the create, and it records the change it made as
    /// an update of the narrow type for the category it changed or
    /// [`UPDATE_TYPE_CONFIG`] for a request that changed more than one.
    pub fn configuration_changed_at(&self, update_type: &str) -> SystemTime {
        self.updates
            .values()
            .filter(|update| update.r#type == update_type || update.r#type == UPDATE_TYPE_CONFIG)
            .map(|update| update.created_at)
            .chain(std::iter::once(self.created_at))
            .max()
            .unwrap_or(self.created_at)
    }

    /// The latest successful Kubernetes version change of this cluster, as
    /// `(UpdateType, when)` — what the cluster's rollback readiness is computed from.
    ///
    /// A version change is only a change the cluster has actually made once its
    /// update is [`UPDATE_SUCCESSFUL`]: an update still running has not moved the
    /// version yet, and a cancelled or failed one never did.
    pub fn latest_version_change(&self) -> Option<(&str, SystemTime)> {
        self.updates
            .values()
            .filter(|update| {
                update.status == UPDATE_SUCCESSFUL
                    && (update.r#type == UPDATE_TYPE_VERSION
                        || update.r#type == UPDATE_TYPE_VERSION_ROLLBACK)
            })
            .max_by_key(|update| update.created_at)
            .map(|update| (update.r#type.as_str(), update.created_at))
    }

    /// The update of this cluster's own control plane that is still running, if
    /// any.
    ///
    /// A cluster's control plane takes at most one update at a time, so this is
    /// what an operation that changes the control plane checks before starting
    /// another. An update of a resource INSIDE the cluster is a change to THAT
    /// resource and is not reported here — see [`Self::in_flight_update_in_scope`],
    /// which is what the operation that updates such a resource checks for the
    /// resource it names.
    pub fn in_flight_update(&self) -> Option<&UpdateRecord> {
        self.in_flight_update_in_scope(&UpdateScope::Cluster)
    }

    /// The update of `scope` this cluster is still running, if any.
    ///
    /// One resource takes at most one update at a time, so this is what the
    /// operation that updates a resource of the cluster checks before starting
    /// another for it.
    pub fn in_flight_update_in_scope(&self, scope: &UpdateScope) -> Option<&UpdateRecord> {
        self.updates
            .values()
            .find(|update| update.is_in_progress() && &update.scope == scope)
    }

    /// The ids of this cluster's updates that belong to `scope`, in the stable
    /// order [`Self::updates`] holds them in.
    ///
    /// That order is update-id order, because [`Self::updates`] is keyed by the
    /// update id: it is what `ListUpdates`'s page token resumes AFTER, so a change
    /// of this map's type would have to keep a stable, sorted walk or that
    /// resumption would skip ids.
    ///
    /// This is what `ListUpdates` pages over: the updates of the resource the
    /// request names — the cluster's own control plane, or one node group, add-on
    /// or capability inside it — and never another resource's.
    pub fn update_ids_in_scope<'a>(
        &'a self,
        scope: &'a UpdateScope,
    ) -> impl Iterator<Item = &'a str> + 'a {
        self.updates
            .values()
            .filter(move |update| &update.scope == scope)
            .map(|update| update.id.as_str())
    }

    /// The update this cluster already recorded for `client_request_token`, if
    /// any — what makes a retry of the call that started it idempotent.
    pub fn update_for_token(&self, token: &str) -> Option<&UpdateRecord> {
        self.updates
            .values()
            .find(|update| update.client_request_token.as_deref() == Some(token))
    }

    /// Append `update` to this cluster's update history, keeping that history
    /// bounded at [`MAX_UPDATES_PER_CLUSTER`].
    ///
    /// `UpdateClusterConfig` mints one record per accepted configuration change,
    /// so this map is the one structure here that grows per REQUEST rather than
    /// per resource name. Reaching the cap evicts the OLDEST TERMINAL update — a
    /// `Successful`, `Cancelled` or `Failed` one, whose work is done and whose
    /// record nothing is owed from — so a caller that keeps changing the
    /// configuration keeps its recent history exact while the emulator's memory
    /// stays bounded. An update still `InProgress` is never evicted: the clock
    /// still owes it a status move and the change it carries.
    pub fn record_update(&mut self, update: UpdateRecord) {
        self.updates.insert(update.id.clone(), update);
        while self.updates.len() > MAX_UPDATES_PER_CLUSTER {
            let Some(oldest_terminal) = self
                .updates
                .values()
                .filter(|update| !update.is_in_progress())
                .min_by_key(|update| update.created_at)
                .map(|update| update.id.clone())
            else {
                // Every entry is still in flight, which the one-update-at-a-time
                // rule each minter enforces makes unreachable; there is nothing
                // this may drop, so the history is left as it is.
                break;
            };
            self.updates.remove(&oldest_terminal);
        }
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
            certificate_authority: self.certificate_authority.clone().map(|certificate| {
                Certificate {
                    // "An object identifying the certificate authority that is
                    // currently signing certificates for the cluster" is derived
                    // from the CA records, so it follows an activation without a
                    // second copy of the signer's identity.
                    active: self.active_certificate_authority(),
                    ..certificate
                }
            }),
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
    /// The change the asynchronous update this node group is taking puts on it
    /// when the clock completes that update; `None` when it is taking none.
    ///
    /// The update itself is a record on the cluster ([`ClusterRecord::updates`]),
    /// which is where its own due instant and status live; this is the part of it
    /// that belongs to the node group, applied at [`Self::active_at`] — the
    /// instant the node group is owed its `ACTIVE` status back, which is the
    /// instant that update is owed its `Successful` one.
    pub pending_config: Option<PendingNodegroupConfig>,
}

/// The change an in-flight node group update will put on its node group when it
/// completes.
///
/// Both operations that update a node group are asynchronous — `UpdateNodegroupConfig`
/// ("you can update... the Kubernetes labels... and the scaling and version update
/// configuration") and `UpdateNodegroupVersion` ("updates the Kubernetes version or
/// AMI version"), each answering with an `Update` rather than with the node group —
/// so the values the request asked for are held HERE until the clock completes that
/// update, and only then written onto the [`NodegroupRecord`]. A member left `None`
/// (or an empty label/taint change) is one the request did not ask to change.
#[derive(Debug, Clone, Default)]
pub struct PendingNodegroupConfig {
    /// The id of the update that carries this change, so a caller tracking the
    /// update and the node group it changes sees one resource, not two copies.
    pub update_id: String,
    /// The Kubernetes labels to add or replace, by key.
    pub add_or_update_labels: BTreeMap<String, String>,
    /// The Kubernetes label keys to remove.
    pub remove_labels: Vec<String>,
    /// The taints to add or replace, matched on the `(key, effect)` pair a taint
    /// is identified by.
    pub add_or_update_taints: Vec<Taint>,
    /// The taints to remove, matched the same way.
    pub remove_taints: Vec<Taint>,
    pub scaling_config: Option<NodegroupScalingConfig>,
    pub update_config: Option<NodegroupUpdateConfig>,
    pub node_repair_config: Option<NodeRepairConfig>,
    pub warm_pool_config: Option<WarmPoolConfig>,
    /// The Kubernetes version an `UpdateNodegroupVersion` update moves the node
    /// group to — "the Kubernetes version to update to"
    /// (`UpdateNodegroupVersionRequest$version`) — so the node group reports the
    /// new version only once the clock completes that update.
    pub version: Option<String>,
    /// The AMI version that same update moves it to: "the AMI version of the
    /// Amazon EKS optimized AMI to use for the update" (`$releaseVersion`).
    pub release_version: Option<String>,
}

/// Whether two taints name the same taint of a node group: "The key of the
/// taint" and "the effect of the taint" are what a node group's scheduler keys
/// on, so a taint added for a `(key, effect)` pair replaces the one already
/// there and the `value` is what the change carries.
fn same_taint(left: &Taint, right: &Taint) -> bool {
    left.key == right.key && left.effect == right.effect
}

impl PendingNodegroupConfig {
    /// Whether this change asks for anything at all — what tells a request that
    /// names no member from one that asks for a change.
    pub fn is_empty(&self) -> bool {
        self.add_or_update_labels.is_empty()
            && self.remove_labels.is_empty()
            && self.add_or_update_taints.is_empty()
            && self.remove_taints.is_empty()
            && self.scaling_config.is_none()
            && self.update_config.is_none()
            && self.node_repair_config.is_none()
            && self.warm_pool_config.is_none()
            && self.version.is_none()
            && self.release_version.is_none()
    }

    /// Write this change onto `record`, which is what completing the update
    /// does. `at` is the instant the update was owed its completion, so
    /// `modifiedAt` is the same value whenever the read that materialises it
    /// comes.
    fn apply(self, record: &mut NodegroupRecord, at: SystemTime) {
        for (key, value) in self.add_or_update_labels {
            record.labels.insert(key, value);
        }
        for key in &self.remove_labels {
            record.labels.remove(key);
        }
        for requested in self.add_or_update_taints {
            match record
                .taints
                .iter_mut()
                .find(|taint| same_taint(taint, &requested))
            {
                Some(existing) => existing.value = requested.value.clone(),
                None => record.taints.push(requested),
            }
        }
        record
            .taints
            .retain(|taint| !self.remove_taints.iter().any(|removed| same_taint(taint, removed)));
        if let Some(scaling_config) = self.scaling_config {
            record.scaling_config = scaling_config;
        }
        if let Some(update_config) = self.update_config {
            record.update_config = update_config;
        }
        if let Some(node_repair_config) = self.node_repair_config {
            record.node_repair_config = Some(node_repair_config);
        }
        if let Some(warm_pool_config) = self.warm_pool_config {
            record.warm_pool_config = Some(warm_pool_config);
        }
        if let Some(version) = self.version {
            record.version = version;
        }
        if let Some(release_version) = self.release_version {
            record.release_version = Some(release_version);
        }
        record.modified_at = at;
    }
}

impl NodegroupRecord {
    /// Materialise the status move the clock already owes at `now`, and write the
    /// change an update owed this record at the instant it was owed. See
    /// [`ClusterRecord::materialize_due_transitions`].
    pub fn materialize_due_transitions(&mut self, now: SystemTime) {
        let Some(due) = self.active_at.filter(|due| now >= *due) else {
            return;
        };
        self.active_at = None;
        if let Some(pending) = self.pending_config.take() {
            pending.apply(self, due);
        }
        self.status = STATUS_ACTIVE.to_string();
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
            // creates neither, so it reports none rather than inventing ids
            // for resources no EC2 store holds.
            resources: None,
        }
    }
}

/// One Fargate profile, as this service stores it.
///
/// Everything a `FargateProfile` reports is held here — including the
/// server-generated `fargateProfileArn` and `createdAt` — so
/// `DescribeFargateProfile` returns exactly what the create wrote rather than
/// recomputing anything per request. A profile is IMMUTABLE ("Fargate profiles
/// are immutable", `CreateFargateProfile`), so nothing but the clock-owed status
/// move ever changes a stored record.
#[derive(Debug, Clone)]
pub struct FargateProfileRecord {
    pub fargate_profile_name: String,
    pub cluster_name: String,
    pub arn: String,
    pub created_at: SystemTime,
    pub pod_execution_role_arn: String,
    /// The subnets the request named, as it named them. Amazon EKS launches the
    /// profile's Pods into these ("The IDs of subnets to launch a Pod into",
    /// `FargateProfile$subnets`); a request that named none carries an empty
    /// list, because this emulator cannot tell a private subnet of the cluster's
    /// VPC from a public one and so has none to choose on the caller's behalf.
    pub subnets: Vec<String>,
    pub selectors: Vec<FargateProfileSelector>,
    pub status: String,
    /// When set, the instant at which the clock owes this profile the move from
    /// `CREATING` to `ACTIVE`. Cleared once the move is materialised.
    pub active_at: Option<SystemTime>,
    pub tags: BTreeMap<String, String>,
    pub client_request_token: Option<String>,
}

impl FargateProfileRecord {
    /// Materialise the status move the clock already owes at `now`. See
    /// [`ClusterRecord::materialize_due_transitions`].
    pub fn materialize_due_transitions(&mut self, now: SystemTime) {
        if self.active_at.is_some_and(|due| now >= due) {
            self.status = FARGATE_PROFILE_STATUS_ACTIVE.to_string();
            self.active_at = None;
        }
    }

    /// This record as the modelled `FargateProfile` shape.
    pub fn to_fargate_profile(&self) -> FargateProfile {
        FargateProfile {
            fargate_profile_name: Some(self.fargate_profile_name.clone()),
            fargate_profile_arn: Some(self.arn.clone()),
            cluster_name: Some(self.cluster_name.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            pod_execution_role_arn: Some(self.pod_execution_role_arn.clone()),
            subnets: Some(self.subnets.clone()),
            selectors: Some(self.selectors.clone()),
            status: Some(self.status.clone()),
            tags: Some(
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            // Nothing in this emulator reports a Fargate profile issue, so the
            // health block is present and empty rather than absent: "Any issues
            // that are associated with the Fargate profile"
            // (`FargateProfileHealth$issues`).
            health: Some(FargateProfileHealth {
                issues: Some(Vec::new()),
            }),
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
/// See [`ADDON_STATUS_CREATING`] — the status an add-on reports while the
/// asynchronous update `UpdateAddon` started is still in flight.
pub const ADDON_STATUS_UPDATING: &str = "UPDATING";
/// See [`ADDON_STATUS_CREATING`] — the status an add-on reports when the change
/// an update carried could not be made.
pub const ADDON_STATUS_UPDATE_FAILED: &str = "UPDATE_FAILED";
/// See [`ADDON_STATUS_CREATING`] — the status an add-on reports once `DeleteAddon`
/// has taken it off its cluster, which is the final status that call answers with
/// ([`ComAmazonawsEksAwswesleyfrontendState::remove_addon`]).
pub const ADDON_STATUS_DELETING: &str = "DELETING";

/// The `AddonIssueCode` an add-on reports for a change an update could not make
/// because the cluster already holds something it conflicts with
/// (`smithy_query query="shape" shape="AddonIssueCode"`).
pub const ADDON_ISSUE_CONFIGURATION_CONFLICT: &str = "ConfigurationConflict";

/// The `AddonIssueCode` an add-on reports when a Kubernetes resource its own
/// resources are applied against is not on the cluster
/// (`smithy_query query="shape" shape="AddonIssueCode"`).
pub const ADDON_ISSUE_K8S_RESOURCE_NOT_FOUND: &str = "K8sResourceNotFound";

/// The `AddonIssueCode` an add-on reports when fewer of its replicas are
/// running than it needs — on a cluster with no compute to schedule them onto,
/// none of them is.
pub const ADDON_ISSUE_INSUFFICIENT_REPLICAS: &str = "InsufficientNumberOfReplicas";

/// The `AddonIssueCode` an add-on reports when the API server refused its
/// resources at admission, which is what an admission webhook with no replica
/// to answer the review does to every resource it gates.
pub const ADDON_ISSUE_ADMISSION_REQUEST_DENIED: &str = "AdmissionRequestDenied";

/// See [`ADDON_STATUS_CREATING`] — the status an add-on reports when it is
/// running but not healthy.
pub const ADDON_STATUS_DEGRADED: &str = "DEGRADED";

/// See [`ADDON_STATUS_CREATING`] — the status an add-on reports when the
/// install never came up, which is terminal: the add-on is removed and
/// installed again rather than repaired in place.
pub const ADDON_STATUS_CREATE_FAILED: &str = "CREATE_FAILED";

/// The `AddonStatus` members an add-on can take an update from: the ones in
/// which it has SETTLED.
///
/// An add-on that is still being created, is being deleted, or is already taking
/// an update has not settled, and nothing can be asked of it until it has. The
/// other three have: a running add-on, one running in a degraded state, and one
/// whose last update failed — for the last two, another `UpdateAddon` is how such
/// an add-on is remediated, which is why they are not refused.
pub const ADDON_STATUSES_AN_UPDATE_STARTS_FROM: &[&str] = &[
    ADDON_STATUS_ACTIVE,
    ADDON_STATUS_DEGRADED,
    ADDON_STATUS_UPDATE_FAILED,
];

/// The `UpdateType` an add-on's own asynchronous update reports
/// (`smithy_query query="shape" shape="UpdateType"`): `UpdateAddon` answers "an
/// object representing an asynchronous update", and this is the one member of
/// that enum which names a change to an installed add-on.
pub const UPDATE_TYPE_ADDON: &str = "AddonUpdate";

/// The `UpdateParamType` keys an add-on update reports its requested values
/// under (`smithy_query query="shape" shape="UpdateParamType"`), one key per
/// member of the request that asks for a change — so a caller reading the update
/// back sees what it asked to change.
pub const UPDATE_PARAM_ADDON_VERSION: &str = "AddonVersion";
/// See [`UPDATE_PARAM_ADDON_VERSION`].
pub const UPDATE_PARAM_SERVICE_ACCOUNT_ROLE_ARN: &str = "ServiceAccountRoleArn";
/// See [`UPDATE_PARAM_ADDON_VERSION`].
pub const UPDATE_PARAM_RESOLVE_CONFLICTS: &str = "ResolveConflicts";
/// See [`UPDATE_PARAM_ADDON_VERSION`].
pub const UPDATE_PARAM_CONFIGURATION_VALUES: &str = "ConfigurationValues";
/// See [`UPDATE_PARAM_ADDON_VERSION`].
pub const UPDATE_PARAM_POD_IDENTITY_ASSOCIATIONS: &str = "PodIdentityAssociations";

/// How long a newly created add-on reports `CREATING` before the clock owes it
/// the terminal status its install settles in
/// ([`ComAmazonawsEksAwswesleyfrontendState::addon_install_outcome`]).
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

/// The `owner` of an add-on a caller subscribes to through AWS Marketplace
/// rather than one Amazon EKS manages.
///
/// `DescribeAddonVersions$owners` filters on it, and the catalog publishes both
/// kinds (see [`ADDON_CATALOG`]): a Marketplace add-on is owned by
/// `aws-marketplace`, published by its VENDOR rather than by `eks`, and carries
/// the `marketplaceInformation` of the listing it is subscribed through.
pub const ADDON_OWNER_MARKETPLACE: &str = "aws-marketplace";

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
/// AWS publishes no "add-ons per cluster" quota, so this ceiling is CHOSEN. What
/// bounds this part of the store is the catalog: an add-on installs only under a
/// name the catalog publishes for the cluster's Kubernetes version
/// ([`ComAmazonawsEksAwswesleyfrontendState::publishes_addon_for`]) and one name
/// installs at most once per cluster, so [`ADDON_CATALOG`] already caps how many
/// records a cluster can retain. This ceiling is the backstop behind that cap: it
/// is generous next to the handful of add-ons a real cluster runs, and
/// `MAX_STORED_CLUSTERS` bounds how many clusters can hold that many.
pub const MAX_ADDONS_PER_CLUSTER: usize = 100;

/// The change an in-flight add-on update puts on its add-on when it completes.
///
/// `UpdateAddon` answers "an object representing an asynchronous update", so the
/// values the request asked for are held HERE until the clock completes that
/// update, and only then written onto the [`AddonRecord`] and the EKS Pod
/// Identity associations the add-on owns. A member left `None` is one the request
/// did not ask to change: applying the change leaves whatever the add-on already
/// reports for it.
#[derive(Debug, Clone, Default)]
pub struct PendingAddonUpdate {
    /// The id of the update that carries this change, so completing it reaches
    /// the record on the cluster this change belongs to — whether to report it
    /// `Successful` or, where the change can no longer be made, `Failed`.
    pub update_id: String,
    pub addon_version: Option<String>,
    pub service_account_role_arn: Option<String>,
    pub configuration_values: Option<String>,
    /// The EKS Pod Identity associations the add-on owns once the update
    /// completes. `None` when the request named none — "if this value is left
    /// blank, no change" (`UpdateAddonRequest$podIdentityAssociations`) — and an
    /// EMPTY list when it asked for the associations the add-on owns to be
    /// deleted.
    pub pod_identity_associations: Option<Vec<AddonPodIdentityAssociations>>,
}

/// The terminal state an add-on install is owed: the `AddonStatus` it settles
/// in and the `AddonHealth$issues` it settles with.
///
/// An install is asynchronous and "leaves `CREATING` for a terminal state that
/// reflects what happened in the cluster", so what it settles in is decided
/// from the cluster's own state when the install starts
/// ([`ComAmazonawsEksAwswesleyfrontendState::addon_install_outcome`]) and
/// materialised when the clock reaches the due instant the record carries
/// ([`AddonRecord::active_at`]) — nothing is recomputed on the read that
/// observes it.
#[derive(Debug, Clone)]
pub struct AddonInstallOutcome {
    /// The `AddonStatus` the install settles in.
    pub status: String,
    /// The `AddonHealth$issues` it settles with: EMPTY for an install that came
    /// up, one issue naming what the cluster could not give it otherwise.
    pub issues: Vec<AddonIssue>,
}

impl AddonInstallOutcome {
    /// The outcome of an install the cluster can carry out: the add-on runs,
    /// with nothing wrong with it.
    pub fn running() -> Self {
        Self {
            status: ADDON_STATUS_ACTIVE.to_string(),
            issues: Vec::new(),
        }
    }

    /// The outcome of an install that never came up, carrying the issue that
    /// says why.
    pub fn failed(issue: AddonIssue) -> Self {
        Self {
            status: ADDON_STATUS_CREATE_FAILED.to_string(),
            issues: vec![issue],
        }
    }

    /// The outcome of an install whose resources were applied but which is not
    /// healthy, carrying the issue that says why.
    pub fn degraded(issue: AddonIssue) -> Self {
        Self {
            status: ADDON_STATUS_DEGRADED.to_string(),
            issues: vec![issue],
        }
    }
}

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
    /// The change the asynchronous update this add-on is taking puts on it when
    /// the clock completes that update; `None` when the add-on is taking none.
    ///
    /// The update itself is a record on the cluster ([`ClusterRecord::updates`]),
    /// which is where its own due instant and status live; this is the part of it
    /// that belongs to the add-on, applied at [`Self::active_at`] — the instant
    /// the add-on is owed its `ACTIVE` status back, which is the instant that
    /// update is owed its `Successful` one.
    pub pending_update: Option<PendingAddonUpdate>,
    /// The add-on's open health issues — "An object representing the health
    /// issues for an add-on" (`AddonHealth$issues`).
    ///
    /// A healthy add-on reports an EMPTY list. An add-on carries an issue when
    /// its install did not come up in the cluster it was installed into
    /// ([`Self::install_outcome`]), or when a change an update carried could not
    /// be made ([`Self::fail_update`]) — in both cases the issue says which.
    pub health_issues: Vec<AddonIssue>,
    /// The terminal state this add-on's INSTALL is owed, decided when the
    /// install started and written onto [`Self::status`] and
    /// [`Self::health_issues`] when the clock reaches [`Self::active_at`].
    pub install_outcome: AddonInstallOutcome,
}

impl AddonRecord {
    /// Materialise the status move the clock already owes at `now`, and hand back
    /// the change an add-on update owes this record, at the instant it was owed.
    ///
    /// As on [`ClusterRecord::materialize_due_transitions`], the move was due at
    /// `active_at` whether or not anybody called, so applying it on read does
    /// not change what a later caller sees.
    ///
    /// Whether that change can be MADE depends on the EKS Pod Identity
    /// associations the add-on owns, which are records of their own, so the change
    /// is returned rather than written here: the one caller that can reach them
    /// ([`ComAmazonawsEksAwswesleyfrontendState::complete_addon_update`]) applies
    /// it, or fails the update, in the same call.
    #[must_use = "the change an add-on update carries is applied by the caller; dropping it \
                  would leave the add-on reporting UPDATING for an update nothing completes"]
    pub fn materialize_due_transitions(
        &mut self,
        now: SystemTime,
    ) -> Option<(SystemTime, PendingAddonUpdate)> {
        let due = self.active_at.filter(|due| now >= *due)?;
        self.active_at = None;
        match self.pending_update.take() {
            // No update is in flight, so the instant was stored for the move out
            // of `CREATING`: once the clock reaches it the add-on reports the
            // terminal state its install was owed, with the health issues that
            // state carries.
            None => {
                self.status = self.install_outcome.status.clone();
                self.health_issues = self.install_outcome.issues.clone();
                None
            }
            Some(pending) => Some((due, pending)),
        }
    }

    /// Write the change an add-on update carried, which is what completing that
    /// update does: the add-on reports the values the request asked for, carries no
    /// open issue and is running again.
    ///
    /// `at` is the instant the update was owed its completion, so `modifiedAt` is
    /// the same value whenever the read that materialises it comes. A member the
    /// change leaves `None` is one the request did not ask to change.
    ///
    /// The status written here is the one the CHANGE earns; whether the add-on's
    /// replicas are running is decided for every settled add-on alike, by
    /// [`ComAmazonawsEksAwswesleyfrontendState::resettle_addon_replicas`].
    pub fn apply_update(&mut self, at: SystemTime, pending: &PendingAddonUpdate) {
        if let Some(addon_version) = pending.addon_version.clone() {
            self.addon_version = addon_version;
        }
        if let Some(role_arn) = pending.service_account_role_arn.clone() {
            self.service_account_role_arn = Some(role_arn);
        }
        if let Some(configuration_values) = pending.configuration_values.clone() {
            self.configuration_values = Some(configuration_values);
        }
        self.status = ADDON_STATUS_ACTIVE.to_string();
        self.health_issues.clear();
        self.modified_at = at;
    }

    /// The `AddonStatus` this add-on has settled in, or the one its install is
    /// owed when the clock has not reached the due instant yet.
    ///
    /// Whatever asks what an add-on IS — rather than what it reports at this
    /// instant — asks this: an install whose outcome is already decided answers
    /// with that outcome, so the answer does not depend on whether a read has
    /// run since it came due.
    pub fn settled_status(&self) -> &str {
        match (&self.pending_update, self.active_at) {
            (None, Some(_)) => self.install_outcome.status.as_str(),
            _ => self.status.as_str(),
        }
    }

    /// Whether this add-on is running on its cluster: its install came up and
    /// nothing has taken it out of service since.
    ///
    /// An add-on that is not running serves nothing the rest of the cluster can
    /// depend on, which is what an add-on installed against its resources is
    /// judged by.
    pub fn is_running(&self) -> bool {
        !matches!(
            self.settled_status(),
            ADDON_STATUS_CREATE_FAILED | ADDON_STATUS_DEGRADED | ADDON_STATUS_DELETING
        )
    }

    /// Record that the change an add-on update carried could not be made: the
    /// add-on keeps every value it had, reports `UPDATE_FAILED` and carries the
    /// issue that says why.
    pub fn fail_update(&mut self, at: SystemTime, issue: AddonIssue) {
        self.status = ADDON_STATUS_UPDATE_FAILED.to_string();
        self.health_issues = vec![issue];
        self.modified_at = at;
    }

    /// This record as the modelled `Addon` shape.
    pub fn to_addon(&self) -> Addon {
        Addon {
            addon_name: Some(self.addon_name.clone()),
            cluster_name: Some(self.cluster_name.clone()),
            status: Some(self.status.clone()),
            addon_version: Some(self.addon_version.clone()),
            // The add-on runs nowhere in this emulator, so nothing it does can go
            // wrong: the health object reports the issue list the record holds,
            // which on a healthy add-on is EMPTY rather than absent.
            health: Some(AddonHealth {
                issues: Some(self.health_issues.clone()),
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

/// The change an in-flight capability update will put on its capability when it
/// completes.
///
/// `UpdateCapability` is asynchronous — it answers with an `Update` rather than
/// with the capability — so the values the request asked for are held HERE until
/// the clock completes that update, and only then written onto the
/// [`CapabilityRecord`]. A member left `None` is one the request did not ask to
/// change: applying the change leaves whatever the capability already reports
/// for it.
#[derive(Debug, Clone, Default)]
pub struct PendingCapabilityUpdate {
    /// The id of the update that carries this change, so a caller tracking the
    /// update and the capability it changes sees one resource, not two copies.
    pub update_id: String,
    pub role_arn: Option<String>,
    /// The Argo CD role mappings to add or replace, keyed by the `role` they
    /// assign: "A mapping between an Argo CD role and IAM Identity Center
    /// identities" (`ArgoCdRoleMapping`), so one role carries one mapping and a
    /// mapping for a role the capability already maps REPLACES it.
    pub add_or_update_role_mappings: Vec<ArgoCdRoleMapping>,
    /// The Argo CD role mappings to drop, by the `role` they assign.
    pub remove_role_mappings: Vec<String>,
    pub network_access: Option<ArgoCdNetworkAccessConfigResponse>,
    pub enable_cross_namespace: Option<bool>,
    pub disabled_services: Option<Vec<String>>,
    pub delete_propagation_policy: Option<String>,
}

impl PendingCapabilityUpdate {
    /// Whether this change carries nothing for the `argoCd` half of a
    /// capability's configuration, which is what decides whether applying it
    /// touches that half at all.
    fn touches_argo_cd(&self) -> bool {
        !self.add_or_update_role_mappings.is_empty()
            || !self.remove_role_mappings.is_empty()
            || self.network_access.is_some()
    }

    /// Whether this change carries nothing for the `ack` half.
    fn touches_ack(&self) -> bool {
        self.enable_cross_namespace.is_some() || self.disabled_services.is_some()
    }

    /// Write this change onto `record`, which is what completing the update
    /// does. `at` is the instant the update was owed its completion, so
    /// `modifiedAt` is the same value whenever the read that materialises it
    /// comes.
    fn apply(self, record: &mut CapabilityRecord, at: SystemTime) {
        if let Some(role_arn) = self.role_arn.clone() {
            record.role_arn = role_arn;
        }
        if let Some(policy) = self.delete_propagation_policy.clone() {
            record.delete_propagation_policy = policy;
        }
        if self.touches_argo_cd() || self.touches_ack() {
            let mut configuration = record.configuration.clone().unwrap_or_default();
            if self.touches_argo_cd() {
                let mut argo_cd = configuration.argo_cd.clone().unwrap_or_default();
                let mut mappings = argo_cd.rbac_role_mappings.clone().unwrap_or_default();
                for requested in &self.add_or_update_role_mappings {
                    match mappings
                        .iter_mut()
                        .find(|mapping| mapping.role == requested.role)
                    {
                        Some(existing) => existing.identities = requested.identities.clone(),
                        None => mappings.push(requested.clone()),
                    }
                }
                mappings.retain(|mapping| !self.remove_role_mappings.contains(&mapping.role));
                argo_cd.rbac_role_mappings = Some(mappings);
                if let Some(network_access) = self.network_access.clone() {
                    argo_cd.network_access = Some(network_access);
                }
                configuration.argo_cd = Some(argo_cd);
            }
            if self.touches_ack() {
                let mut ack = configuration.ack.clone().unwrap_or_default();
                if let Some(enable_cross_namespace) = self.enable_cross_namespace {
                    ack.enable_cross_namespace = Some(enable_cross_namespace);
                }
                if let Some(disabled_services) = self.disabled_services.clone() {
                    ack.disabled_services = Some(disabled_services);
                }
                configuration.ack = Some(ack);
            }
            record.configuration = Some(configuration);
        }
        record.modified_at = at;
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
    /// The change the asynchronous update this capability is taking puts on it
    /// when the clock completes that update; `None` when it is taking none.
    ///
    /// The update itself is a record on the cluster ([`ClusterRecord::updates`]),
    /// which is where its own due instant and status live; this is the part of it
    /// that belongs to the capability, applied at [`Self::active_at`] — the
    /// instant the capability is owed its `ACTIVE` status back, which is the
    /// instant that update is owed its `Successful` one.
    pub pending_update: Option<PendingCapabilityUpdate>,
}

impl CapabilityRecord {
    /// Materialise the status move the clock already owes at `now`, and write the
    /// change an update owed this record at the instant it was owed.
    ///
    /// The only write a reporting operation may make: the transition was due at
    /// `active_at` whether or not anybody called, so applying it on read does not
    /// change what a later caller sees.
    pub fn materialize_due_transitions(&mut self, now: SystemTime) {
        let Some(due) = self.active_at.filter(|due| now >= *due) else {
            return;
        };
        self.active_at = None;
        if let Some(pending) = self.pending_update.take() {
            pending.apply(self, due);
        }
        self.status = CAPABILITY_STATUS_ACTIVE.to_string();
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
            // the catalog and the cluster slice cannot disagree.
            default_platform_version: Some(PLATFORM_VERSION.to_string()),
            default_version: self.default_version,
            release_date: Some(::aws_smithy_types::DateTime::from(self.release_date)),
            end_of_standard_support_date: Some(::aws_smithy_types::DateTime::from(
                self.end_of_standard_support,
            )),
            end_of_extended_support_date: Some(::aws_smithy_types::DateTime::from(
                self.end_of_extended_support,
            )),
            // Both members report the same lifecycle state, and on the WIRE both
            // report it in the `VersionStatus` spelling: `status` "is deprecated.
            // Use `versionStatus` instead, as that field matches for input and
            // output of this action" — the note says the CURRENT member is the one
            // whose tokens match in both directions, so the deprecated member's
            // output tokens are not the ones its own filter accepts. The filter
            // side keeps the `ClusterVersionStatus` spelling (see the handler).
            status: Some(status.version_status().to_string()),
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
    /// same way it publishes one platform revision ([`PLATFORM_VERSION`]) for
    /// every version it carries.
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
/// Each tier reports the component configuration a control plane on it runs with
/// as its `controlPlaneComponentConfigOverrides` ("The control plane component
/// configuration overrides specific to this scaling tier"). This service
/// publishes ONE set of component defaults and constraints
/// ([`control_plane_component_config`]), so every tier reports that same set
/// rather than a per-tier value this emulator would have to invent: a caller
/// reading a tier learns the window it may configure a cluster on that tier in
/// without having to merge the tier against the version.
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
                    control_plane_component_config_overrides: Some(
                        control_plane_component_config(),
                    ),
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

/// The processor architectures every build in the add-on catalog is published
/// for.
///
/// `AddonVersionInfo$architecture` is "The architectures that the version
/// supports", and Amazon EKS publishes each managed add-on for both the
/// architectures its node groups run ([`crate::ops::types::AMI_TYPES_VALUES`]
/// carries the same two in its AMI families: `x86_64` and `ARM_64`).
const ADDON_ARCHITECTURES: &[&str] = &["amd64", "arm64"];

/// The `platformVersions` entry a compatibility carries when the add-on build
/// runs on every platform version of the cluster version it supports.
///
/// `Compatibility$platformVersions` is "The supported Amazon EKS platform
/// versions", and this service publishes ONE platform version per cluster
/// version ([`PLATFORM_VERSION`]), so no published build is restricted to a
/// subset of them.
pub const ADDON_ANY_PLATFORM_VERSION: &str = "*";

/// The JSON Schema dialect `DescribeAddonConfiguration` publishes its
/// configuration schema in.
///
/// `DescribeAddonConfigurationResponse$configurationSchema` is "A JSON schema
/// that's used to validate the configuration values you provide when an add-on
/// is created or updated", so the document names the dialect it is written in,
/// as every JSON Schema does.
const ADDON_CONFIGURATION_SCHEMA_DIALECT: &str = "http://json-schema.org/draft-06/schema#";

/// One top-level property an add-on's configuration schema declares: the key a
/// `configurationValues` document may set, and the JSON Schema type that key
/// takes (`"boolean"`, `"string"`, `"integer"`, `"number"`, `"object"`,
/// `"array"`).
type AddonSchemaProperty = (&'static str, &'static str);

/// The configuration keys EVERY published add-on's schema declares, beside the
/// keys of its own ([`AddonCatalogSeed::configuration_properties`]).
///
/// Source: an add-on's configuration schema is the set of values its chart
/// exposes, and every Amazon EKS add-on chart exposes the Kubernetes placement
/// and pod-metadata values a cluster operator sets on any workload. They are
/// written once here rather than repeated per row, so one add-on's schema cannot
/// quietly declare fewer of them than another's.
const ADDON_COMMON_CONFIGURATION_PROPERTIES: &[AddonSchemaProperty] = &[
    ("affinity", "object"),
    ("imagePullSecrets", "array"),
    ("nodeSelector", "object"),
    ("podAnnotations", "object"),
    ("podDisruptionBudget", "object"),
    ("podLabels", "object"),
    ("priorityClassName", "string"),
    ("resources", "object"),
    ("tolerations", "array"),
    ("topologySpreadConstraints", "array"),
];

/// How an add-on's pods are placed on a cluster, which is what decides whether
/// its install has a replica to get scheduled at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddonWorkload {
    /// One pod per node, as a DaemonSet: a cluster with no nodes has no pod of
    /// it to place, so applying its resources is the whole install.
    PerNode,
    /// At least one replica of a Deployment, which Kubernetes schedules onto a
    /// node — so an install of it needs the cluster to have compute.
    Replicated,
}

/// One row of the add-on catalog seed: an Amazon EKS-managed add-on, and the
/// version line its published builds run along.
///
/// The catalog is a PUBLICATION, like the Kubernetes version calendar
/// ([`VERSION_CALENDAR`]): it is what Amazon EKS offers, not per-account data,
/// so it is seeded once and no operation writes it.
struct AddonCatalogSeed {
    /// The add-on's name, the value `CreateAddon$addonName` takes.
    addon_name: &'static str,
    /// Who owns the add-on ([`ADDON_OWNER_AWS`] or
    /// [`ADDON_OWNER_MARKETPLACE`]), which `DescribeAddonVersions$owners`
    /// filters on.
    owner: &'static str,
    /// Who publishes the add-on ([`ADDON_PUBLISHER_EKS`] for an Amazon
    /// EKS-managed add-on, the vendor for a Marketplace one), which
    /// `DescribeAddonVersions$publishers` filters on.
    publisher: &'static str,
    /// The category Amazon EKS publishes the add-on under
    /// (`AddonInfo$type`), which `DescribeAddonVersions$types` filters on.
    addon_type: &'static str,
    /// The Kubernetes namespace the add-on's workloads are installed into
    /// (`AddonInfo$defaultNamespace`).
    default_namespace: &'static str,
    /// How the add-on's pods are placed, which decides what its install needs
    /// from the cluster: a [`AddonWorkload::PerNode`] add-on has nothing to
    /// schedule on a cluster with no nodes, a [`AddonWorkload::Replicated`] one
    /// must get a replica scheduled somewhere.
    ///
    /// Source: the workloads each add-on's chart ships — the node agents and
    /// device plugins are DaemonSets, the controllers, operators and servers are
    /// Deployments.
    workload: AddonWorkload,
    /// The add-ons whose Kubernetes resources this add-on's own resources are
    /// applied against: the ones its documented install requires on the cluster
    /// FIRST, because its custom resources, certificates or webhooks come from
    /// them. EMPTY for an add-on that needs nothing but the cluster.
    ///
    /// Source: the prerequisites the Amazon EKS User Guide lists for the add-on.
    required_addons: &'static [&'static str],
    /// Whether the add-on serves an admission webhook the API server calls
    /// before it admits the resources of an add-on installed against this one.
    ///
    /// Source: the webhook key the add-on's own configuration schema declares
    /// ([`AddonCatalogSeed::configuration_properties`]).
    admission_webhook: bool,
    /// Whether the add-on needs IAM permissions of its own
    /// (`AddonVersionInfo$requiresIamPermissions`).
    requires_iam_permissions: bool,
    /// The configuration properties the add-on cannot be installed without.
    /// EMPTY for an add-on whose configuration values are optional, which is
    /// what makes `AddonVersionInfo$requiresConfiguration` false for it and
    /// what leaves the published configuration schema with no required
    /// property: the flag and the schema have this one source.
    required_configuration: &'static [&'static str],
    /// The top-level keys this add-on's OWN configuration schema declares, each
    /// with the JSON type that key takes — published beside the keys every
    /// add-on's schema carries ([`ADDON_COMMON_CONFIGURATION_PROPERTIES`]), and
    /// beside the ones the install itself takes (see
    /// [`AddonCatalogRecord::configuration_schema`]). A key named here with a
    /// type of its own OVERRIDES the common one.
    ///
    /// Source: the schema each add-on publishes, which is the set of values its
    /// own chart exposes — `aws eks describe-addon-configuration --addon-name
    /// <name> --addon-version <build>` reports it, and the Amazon EKS User
    /// Guide's configuration examples for an add-on set keys from this list
    /// (`aws-efs-csi-driver`'s `useFIPS` is the one the guide's FIPS install
    /// passes). Keys are written in the schema's own alphabetical order.
    configuration_properties: &'static [AddonSchemaProperty],
    /// The Kubernetes service accounts the add-on runs as, each with the
    /// managed policies Amazon EKS recommends for it — what
    /// `DescribeAddonConfiguration$podIdentityConfiguration` reports ("The
    /// Kubernetes service account name used by the add-on, and any suggested
    /// IAM policies"). Empty for an add-on that runs as no service account of
    /// its own.
    pod_identity: &'static [(&'static str, &'static [&'static str])],
    /// `(major, minor, patch)` of the build published for the OLDEST Kubernetes
    /// version in [`VERSION_CALENDAR`]. An add-on's version tracks the ADD-ON,
    /// not the control plane, so each add-on carries its own line.
    oldest_build: (u32, u32, u32),
}

/// The add-ons this service publishes.
///
/// The Amazon EKS-managed rows are the add-ons the "Amazon EKS add-ons" section
/// of the Amazon EKS User Guide publishes — the ones Amazon EKS builds itself
/// and the community projects it curates alongside them — with the name a caller
/// installs each by, the category it is published under, the namespace its
/// workloads run in, whether it needs IAM permissions of its own, the service
/// account it runs as with the managed policies the guide recommends for it, and
/// the keys its own configuration schema declares. They are owned by
/// [`ADDON_OWNER_AWS`] and published by [`ADDON_PUBLISHER_EKS`].
///
/// This table is what the catalog PUBLISHES, so an add-on missing from it cannot
/// be discovered or installed at all: a row here is the only way a caller reads
/// an add-on's builds, its compatibilities and its configuration surface.
///
/// The rest are AWS Marketplace add-ons ([`ADDON_OWNER_MARKETPLACE`]), published
/// by their VENDOR and carrying the `marketplaceInformation` of the listing they
/// are subscribed through. They are in the catalog because the owner, publisher
/// and `requiresConfiguration` an add-on reports are properties of the add-on,
/// not of Amazon EKS: a Marketplace add-on that cannot run without its vendor's
/// credentials is the kind that reports `requiresConfiguration` true, and the
/// configuration properties named here are the ones that vendor's own
/// documentation requires.
///
/// Rows are written in add-on NAME order, which is the order the catalog is
/// published in ([`seed_addon_catalog`]). The table is fixed, so it also bounds
/// this part of the store.
const ADDON_CATALOG: &[AddonCatalogSeed] = &[
    AddonCatalogSeed {
        addon_name: "adot",
        // The OpenTelemetry Operator, whose admission webhooks are served with
        // certificates cert-manager issues.
        workload: AddonWorkload::Replicated,
        required_addons: &["cert-manager"],
        admission_webhook: true,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "observability",
        default_namespace: "opentelemetry-operator-system",
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("admissionWebhooks", "object"),
            ("affinity", "object"),
            ("collector", "object"),
            ("manager", "object"),
            ("tolerations", "array"),
        ],
        oldest_build: (0, 102, 1),
    },
    AddonCatalogSeed {
        addon_name: "amazon-cloudwatch-observability",
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: true,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "observability",
        default_namespace: "amazon-cloudwatch",
        requires_iam_permissions: true,
        required_configuration: &[],
        pod_identity: &[(
            "cloudwatch-agent",
            &[
                "arn:aws:iam::aws:policy/CloudWatchAgentServerPolicy",
                "arn:aws:iam::aws:policy/AWSXrayWriteOnlyAccess",
            ],
        )],
        configuration_properties: &[
            ("admissionWebhooks", "object"),
            ("agent", "object"),
            ("containerLogs", "object"),
            ("manager", "object"),
            ("tolerations", "array"),
        ],
        oldest_build: (1, 5, 0),
    },
    AddonCatalogSeed {
        // Amazon SageMaker HyperPod's task governance, which schedules the
        // training and inference workloads of a HyperPod cluster whose
        // orchestration runs on this cluster.
        addon_name: "amazon-sagemaker-hyperpod-taskgovernance",
        workload: AddonWorkload::Replicated,
        required_addons: &["cert-manager"],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "machine-learning",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        // Its chart exposes the queue the governance controller schedules
        // through and how many replicas of that controller run, beside the
        // placement values every add-on's schema carries.
        configuration_properties: &[
            ("kueue", "object"),
            ("replicaCount", "integer"),
        ],
        oldest_build: (1, 0, 0),
    },
    AddonCatalogSeed {
        addon_name: "amazon-sagemaker-hyperpod-training-operator",
        workload: AddonWorkload::Replicated,
        required_addons: &["cert-manager"],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "machine-learning",
        default_namespace: "aws-hyperpod",
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("hpTrainingControllerManager", "object"),
            ("nodeSelector", "object"),
            ("replicaCount", "integer"),
            ("resources", "object"),
            ("tolerations", "array"),
        ],
        oldest_build: (1, 0, 0),
    },
    AddonCatalogSeed {
        addon_name: "aws-ebs-csi-driver",
        // The CSI drivers run a controller Deployment beside their node
        // DaemonSet, so an install of one has a replica to place.
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "storage",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: true,
        required_configuration: &[],
        pod_identity: &[(
            "ebs-csi-controller-sa",
            &["arn:aws:iam::aws:policy/service-role/AmazonEBSCSIDriverPolicy"],
        )],
        configuration_properties: &[
            ("additionalDaemonSets", "object"),
            ("controller", "object"),
            ("defaultStorageClass", "object"),
            ("node", "object"),
            ("replicaCount", "integer"),
            ("sidecars", "object"),
            ("storageClasses", "array"),
        ],
        oldest_build: (1, 31, 0),
    },
    AddonCatalogSeed {
        // The Elastic Fabric Adapter device plugin, which advertises the EFA
        // network interfaces of a node to the scheduler.
        addon_name: "aws-efa-k8s-device-plugin",
        workload: AddonWorkload::PerNode,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "networking",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        // The plugin is a DaemonSet that advertises a node's devices and takes
        // no settings of its own beyond the placement values every add-on's
        // schema carries.
        configuration_properties: &[],
        oldest_build: (0, 5, 3),
    },
    AddonCatalogSeed {
        addon_name: "aws-efs-csi-driver",
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "storage",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: true,
        required_configuration: &[],
        pod_identity: &[(
            "efs-csi-controller-sa",
            &["arn:aws:iam::aws:policy/service-role/AmazonEFSCSIDriverPolicy"],
        )],
        configuration_properties: &[
            ("controller", "object"),
            ("node", "object"),
            ("replicaCount", "integer"),
            ("sidecars", "object"),
            ("storageClasses", "array"),
            // The key the guide's FIPS install sets: an add-on's schema
            // declares the options that add-on itself takes.
            ("useFIPS", "boolean"),
        ],
        oldest_build: (2, 0, 7),
    },
    AddonCatalogSeed {
        addon_name: "aws-guardduty-agent",
        workload: AddonWorkload::PerNode,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "security",
        default_namespace: "amazon-guardduty",
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("affinity", "object"),
            ("priorityClassName", "string"),
            ("resources", "object"),
            ("tolerations", "array"),
        ],
        oldest_build: (1, 6, 1),
    },
    AddonCatalogSeed {
        addon_name: "aws-mountpoint-s3-csi-driver",
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "storage",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: true,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("controller", "object"),
            ("node", "object"),
            ("tolerations", "array"),
        ],
        oldest_build: (1, 7, 0),
    },
    AddonCatalogSeed {
        // The agent that reports the cluster's network flows to Amazon
        // CloudWatch Network Flow Monitor, which it publishes to with
        // permissions of its own.
        addon_name: "aws-network-flow-monitoring-agent",
        workload: AddonWorkload::PerNode,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "observability",
        default_namespace: "aws-network-flow-monitor",
        requires_iam_permissions: true,
        required_configuration: &[],
        pod_identity: &[(
            "aws-network-flow-monitor-agent",
            &["arn:aws:iam::aws:policy/CloudWatchNetworkFlowMonitorAgentPublishPolicy"],
        )],
        // Its chart exposes the agent itself; the rest of what a cluster
        // operator sets on it are the placement values every add-on's schema
        // carries.
        configuration_properties: &[("agent", "object")],
        oldest_build: (1, 0, 1),
    },
    AddonCatalogSeed {
        addon_name: "cert-manager",
        // Its controller, cainjector and webhook are Deployments, and the
        // webhook is the one the API server calls for the resources of the
        // add-ons installed against it.
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: true,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "security",
        default_namespace: "cert-manager",
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("cainjector", "object"),
            ("installCRDs", "boolean"),
            ("prometheus", "object"),
            ("replicaCount", "integer"),
            ("resources", "object"),
            ("tolerations", "array"),
            ("webhook", "object"),
        ],
        oldest_build: (1, 16, 2),
    },
    AddonCatalogSeed {
        addon_name: "coredns",
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "networking",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("autoScaling", "object"),
            ("computeType", "string"),
            ("corefile", "string"),
            ("replicaCount", "integer"),
        ],
        oldest_build: (1, 11, 1),
    },
    AddonCatalogSeed {
        addon_name: "datadog_operator",
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_MARKETPLACE,
        publisher: "datadog",
        addon_type: "monitoring",
        default_namespace: "datadog-agent",
        requires_iam_permissions: false,
        // The Datadog Operator sends telemetry to a Datadog organisation, which
        // it cannot do without that organisation's API key.
        required_configuration: &["apiKey"],
        pod_identity: &[],
        configuration_properties: &[
            ("clusterName", "string"),
            ("resources", "object"),
            ("site", "string"),
        ],
        oldest_build: (1, 8, 0),
    },
    AddonCatalogSeed {
        addon_name: "dynatrace_dynatrace-operator",
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_MARKETPLACE,
        publisher: "dynatrace",
        addon_type: "monitoring",
        default_namespace: "dynatrace",
        requires_iam_permissions: false,
        // The Dynatrace Operator connects to one Dynatrace environment, which it
        // cannot do without that environment's API URL and token.
        required_configuration: &["apiUrl", "apiToken"],
        pod_identity: &[],
        configuration_properties: &[("resources", "object"), ("tolerations", "array")],
        oldest_build: (1, 3, 0),
    },
    AddonCatalogSeed {
        addon_name: "eks-node-monitoring-agent",
        workload: AddonWorkload::PerNode,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "observability",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("affinity", "object"),
            ("nodeSelector", "object"),
            ("resources", "object"),
            ("tolerations", "array"),
        ],
        oldest_build: (1, 0, 0),
    },
    AddonCatalogSeed {
        addon_name: "eks-pod-identity-agent",
        workload: AddonWorkload::PerNode,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "security",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("affinity", "object"),
            ("agent", "object"),
            ("nodeSelector", "object"),
            ("tolerations", "array"),
        ],
        oldest_build: (1, 2, 0),
    },
    AddonCatalogSeed {
        addon_name: "external-dns",
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "networking",
        default_namespace: "external-dns",
        requires_iam_permissions: true,
        required_configuration: &[],
        pod_identity: &[(
            "external-dns",
            &["arn:aws:iam::aws:policy/AmazonRoute53FullAccess"],
        )],
        configuration_properties: &[
            ("domainFilters", "array"),
            ("policy", "string"),
            ("provider", "string"),
            ("resources", "object"),
            ("sources", "array"),
            ("txtOwnerId", "string"),
        ],
        oldest_build: (0, 15, 0),
    },
    AddonCatalogSeed {
        addon_name: "kube-proxy",
        workload: AddonWorkload::PerNode,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "networking",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        // kube-proxy is the one add-on whose version IS the Kubernetes version
        // it runs with, so its line starts at the oldest calendar row's version.
        configuration_properties: &[
            ("affinity", "object"),
            ("nodeSelector", "object"),
            ("resources", "object"),
            ("tolerations", "array"),
        ],
        oldest_build: (1, 30, 0),
    },
    AddonCatalogSeed {
        addon_name: "kube-state-metrics",
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "observability",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("collectors", "array"),
            ("replicas", "integer"),
            ("resources", "object"),
            ("tolerations", "array"),
        ],
        oldest_build: (2, 13, 0),
    },
    AddonCatalogSeed {
        addon_name: "kubecost_kubecost",
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_MARKETPLACE,
        publisher: "kubecost",
        addon_type: "cost-management",
        default_namespace: "kubecost",
        requires_iam_permissions: false,
        // Kubecost reports on the cluster it runs in, so it needs no vendor
        // credentials: a Marketplace add-on requires configuration only when it
        // cannot run without it.
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("global", "object"),
            ("kubecostToken", "string"),
            ("persistentVolume", "object"),
            ("prometheus", "object"),
        ],
        oldest_build: (2, 3, 0),
    },
    AddonCatalogSeed {
        addon_name: "metrics-server",
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "observability",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("args", "array"),
            ("nodeSelector", "object"),
            ("replicas", "integer"),
            ("resources", "object"),
            ("tolerations", "array"),
        ],
        oldest_build: (0, 7, 0),
    },
    AddonCatalogSeed {
        addon_name: "prometheus-node-exporter",
        workload: AddonWorkload::PerNode,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "observability",
        default_namespace: "prometheus-node-exporter",
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("hostNetwork", "boolean"),
            ("resources", "object"),
            ("service", "object"),
            ("tolerations", "array"),
        ],
        oldest_build: (1, 8, 2),
    },
    AddonCatalogSeed {
        addon_name: "snapshot-controller",
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "storage",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("replicaCount", "integer"),
            ("resources", "object"),
            ("tolerations", "array"),
            ("volumeSnapshotClasses", "array"),
        ],
        oldest_build: (8, 1, 0),
    },
    AddonCatalogSeed {
        addon_name: "vpc-cni",
        workload: AddonWorkload::PerNode,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "networking",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: true,
        required_configuration: &[],
        pod_identity: &[("aws-node", &["arn:aws:iam::aws:policy/AmazonEKS_CNI_Policy"])],
        configuration_properties: &[
            ("branchENICooldown", "integer"),
            // `enableNetworkPolicy` and `enableWindowsIpam` are STRINGS in this
            // add-on's schema, not booleans: the key a client sets is the one
            // the schema declares, with the type the schema gives it.
            ("enableNetworkPolicy", "string"),
            ("enableWindowsIpam", "string"),
            ("eniConfig", "object"),
            ("env", "object"),
            ("init", "object"),
            ("nodeAgent", "object"),
        ],
        oldest_build: (1, 18, 0),
    },
];

/// One Kubernetes version a published add-on build supports, as the catalog
/// holds it: the modelled `Compatibility`.
#[derive(Debug, Clone)]
pub struct AddonCompatibilityRecord {
    /// The Kubernetes minor version this build supports, a
    /// [`VERSION_CALENDAR`] row.
    pub cluster_version: String,
    /// The platform versions of that cluster version the build runs on
    /// ([`ADDON_ANY_PLATFORM_VERSION`]).
    pub platform_versions: Vec<String>,
    /// Whether this build is the one `CreateAddon` installs on a cluster of
    /// that Kubernetes version when the caller names no `addonVersion`.
    pub default_version: bool,
}

impl AddonCompatibilityRecord {
    /// This row as the modelled `Compatibility` shape.
    pub fn to_compatibility(&self) -> Compatibility {
        Compatibility {
            cluster_version: Some(self.cluster_version.clone()),
            platform_versions: Some(self.platform_versions.clone()),
            default_version: self.default_version,
        }
    }
}

/// One published build of one add-on, as the catalog holds it: the modelled
/// `AddonVersionInfo`.
#[derive(Debug, Clone)]
pub struct AddonCatalogVersionRecord {
    /// The build's version, in the `vX.Y.Z-eksbuild.N` form every Amazon EKS
    /// add-on version takes.
    pub addon_version: String,
    /// When Amazon EKS published this build — the release date of the
    /// Kubernetes version it was published for. A build the clock has not
    /// reached is not one the catalog offers yet, exactly as an unreleased
    /// Kubernetes version is not ([`ClusterVersionRecord::is_released`]).
    pub published_at: SystemTime,
    /// The architectures the build is published for ([`ADDON_ARCHITECTURES`]).
    pub architecture: Vec<String>,
    /// The Kubernetes versions the build supports, oldest first.
    pub compatibilities: Vec<AddonCompatibilityRecord>,
    /// The configuration properties the build cannot be installed without; see
    /// [`AddonCatalogSeed::required_configuration`].
    pub required_configuration: Vec<String>,
    /// Whether the build needs IAM permissions of its own.
    pub requires_iam_permissions: bool,
}

impl AddonCatalogVersionRecord {
    /// Whether Amazon EKS has published this build at `now`.
    pub fn is_published(&self, now: SystemTime) -> bool {
        now >= self.published_at
    }

    /// Whether this build supports a Kubernetes version.
    pub fn supports(&self, cluster_version: &str) -> bool {
        self.compatibilities
            .iter()
            .any(|entry| entry.cluster_version == cluster_version)
    }

    /// Whether this build cannot be installed without configuration values —
    /// `AddonVersionInfo$requiresConfiguration`, which is exactly "this build
    /// has a required configuration property".
    pub fn requires_configuration(&self) -> bool {
        !self.required_configuration.is_empty()
    }

    /// Whether this build is the default one for a Kubernetes version.
    pub fn is_default_for(&self, cluster_version: &str) -> bool {
        self.compatibilities
            .iter()
            .any(|entry| entry.cluster_version == cluster_version && entry.default_version)
    }

    /// This build as the modelled `AddonVersionInfo` shape.
    ///
    /// `cluster_version`, when given, is the Kubernetes version the listing was
    /// filtered to: the reported compatibilities are then the ones for that
    /// version alone, because a filtered listing answers about that version.
    pub fn to_addon_version_info(&self, cluster_version: Option<&str>) -> AddonVersionInfo {
        AddonVersionInfo {
            addon_version: Some(self.addon_version.clone()),
            architecture: Some(self.architecture.clone()),
            // `computeTypes` is "The type of compute associated with the
            // add-on version": no build this catalog publishes is restricted to
            // one, so there is nothing to report.
            compute_types: None,
            compatibilities: Some(
                self.compatibilities
                    .iter()
                    .filter(|entry| {
                        cluster_version.is_none_or(|wanted| wanted == entry.cluster_version)
                    })
                    .map(AddonCompatibilityRecord::to_compatibility)
                    .collect(),
            ),
            requires_configuration: self.requires_configuration(),
            requires_iam_permissions: self.requires_iam_permissions,
        }
    }
}

/// One add-on the catalog publishes, with every build of it: the modelled
/// `AddonInfo`.
#[derive(Debug, Clone)]
pub struct AddonCatalogRecord {
    pub addon_name: String,
    pub addon_type: String,
    pub owner: String,
    pub publisher: String,
    pub default_namespace: String,
    /// How this add-on's pods are placed; see [`AddonCatalogSeed::workload`].
    pub workload: AddonWorkload,
    /// The add-ons this one's resources are applied against; see
    /// [`AddonCatalogSeed::required_addons`].
    pub required_addons: Vec<String>,
    /// Whether this add-on serves an admission webhook; see
    /// [`AddonCatalogSeed::admission_webhook`].
    pub admission_webhook: bool,
    /// The AWS Marketplace listing this add-on is subscribed through, for an
    /// add-on owned by [`ADDON_OWNER_MARKETPLACE`]; `None` for an Amazon
    /// EKS-managed one, which is not listed on Marketplace at all.
    pub marketplace_information: Option<MarketplaceInformation>,
    /// The service accounts the add-on runs as, with the managed policies
    /// recommended for each; see [`AddonCatalogSeed::pod_identity`].
    pub pod_identity: Vec<(String, Vec<String>)>,
    /// The keys this add-on's configuration schema declares, each with the JSON
    /// type it takes: its own ([`AddonCatalogSeed::configuration_properties`])
    /// merged with the ones every add-on's schema carries
    /// ([`ADDON_COMMON_CONFIGURATION_PROPERTIES`]), in the schema's own
    /// alphabetical order.
    pub configuration_properties: Vec<(String, String)>,
    /// Every build of this add-on, NEWEST FIRST — the order the catalog
    /// publishes versions in, so a caller reading it to pick a version sees the
    /// newest build at the head of the list.
    pub versions: Vec<AddonCatalogVersionRecord>,
}

impl AddonCatalogRecord {
    /// The builds of this add-on Amazon EKS has published at `now`, newest
    /// first.
    pub fn published_versions(
        &self,
        now: SystemTime,
    ) -> impl Iterator<Item = &AddonCatalogVersionRecord> + '_ {
        self.versions
            .iter()
            .filter(move |version| version.is_published(now))
    }

    /// This add-on as the modelled `AddonInfo` shape, reporting `versions`.
    pub fn to_addon_info(&self, versions: Vec<AddonVersionInfo>) -> AddonInfo {
        AddonInfo {
            addon_name: Some(self.addon_name.clone()),
            r#type: Some(self.addon_type.clone()),
            addon_versions: Some(versions),
            publisher: Some(self.publisher.clone()),
            owner: Some(self.owner.clone()),
            marketplace_information: self.marketplace_information.clone(),
            default_namespace: Some(self.default_namespace.clone()),
        }
    }

    /// The service accounts this add-on runs as, as the modelled
    /// `AddonPodIdentityConfiguration` list.
    pub fn to_pod_identity_configuration(&self) -> Vec<AddonPodIdentityConfiguration> {
        self.pod_identity
            .iter()
            .map(|(service_account, policies)| AddonPodIdentityConfiguration {
                service_account: Some(service_account.clone()),
                recommended_managed_policies: Some(policies.clone()),
            })
            .collect()
    }

    /// The JSON schema `DescribeAddonConfiguration` publishes for one build of
    /// this add-on: "A JSON schema that's used to validate the configuration
    /// values you provide when an add-on is created or updated".
    ///
    /// The properties it declares are THIS add-on's own configurable keys
    /// ([`AddonCatalogSeed::configuration_properties`], each with the JSON type
    /// that key takes) and the ones every add-on's schema carries
    /// ([`ADDON_COMMON_CONFIGURATION_PROPERTIES`]), beside the two the install
    /// itself takes — the namespace the add-on is installed into and the service
    /// account it runs as — and the properties the build cannot be installed
    /// without. A document may set no other key, which is what
    /// `additionalProperties: false` states, so a client reading the schema
    /// learns exactly the configuration surface of this add-on rather than a
    /// document shared by every one of them. Its `required` list is the build's
    /// own
    /// [`AddonCatalogVersionRecord::required_configuration`], which is also what
    /// makes [`AddonCatalogVersionRecord::requires_configuration`] true: a build
    /// that requires configuration publishes a schema that requires it, and one
    /// that does not publishes a schema with nothing required.
    pub fn configuration_schema(&self, version: &AddonCatalogVersionRecord) -> String {
        self.configuration_schema_value(version).to_string()
    }

    /// That same published schema as the JSON document it is, which is what
    /// [`Self::configuration_violation`] holds a configuration document to:
    /// ONE source, with no serialise-and-parse round trip between the schema a
    /// client reads and the schema a create is judged by.
    fn configuration_schema_value(
        &self,
        version: &AddonCatalogVersionRecord,
    ) -> ::serde_json::Value {
        let mut properties = ::serde_json::Map::new();
        properties.insert(
            "namespace".to_string(),
            ::serde_json::json!({
                "type": "string",
                "default": self.default_namespace,
                "description": "The Kubernetes namespace the add-on is installed into.",
            }),
        );
        if let Some((service_account, _)) = self.pod_identity.first() {
            properties.insert(
                "serviceAccount".to_string(),
                ::serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "default": service_account,
                            "description":
                                "The Kubernetes service account the add-on runs as.",
                        },
                    },
                }),
            );
        }
        for (property, json_type) in &self.configuration_properties {
            properties.insert(
                property.clone(),
                ::serde_json::json!({ "type": json_type }),
            );
        }
        for property in &version.required_configuration {
            // A required key keeps the type the add-on's own schema gives it —
            // only the "you cannot install without this" description is added —
            // so a required property of a type other than string cannot be
            // mis-declared here and then refused when a caller sets it.
            let declared = properties
                .get(property)
                .and_then(|declared| declared.get("type"))
                .and_then(::serde_json::Value::as_str)
                .unwrap_or("string")
                .to_string();
            properties.insert(
                property.clone(),
                ::serde_json::json!({
                    "type": declared,
                    "description": format!(
                        "Required to install {}: the add-on cannot run without it.",
                        self.addon_name
                    ),
                }),
            );
        }
        ::serde_json::json!({
            "$schema": ADDON_CONFIGURATION_SCHEMA_DIALECT,
            "type": "object",
            "properties": ::serde_json::Value::Object(properties),
            "required": version.required_configuration,
            // A key the schema does not declare is not a key this add-on takes,
            // so a document that sets one does not satisfy the schema.
            "additionalProperties": false,
        })
    }

    /// How `document` fails the configuration schema this entry publishes for
    /// `version` — or `None` when it satisfies that schema.
    ///
    /// "The values that you provide are validated against the schema returned by
    /// DescribeAddonConfiguration" (`CreateAddon$configurationValues`), so this
    /// validates against THAT document — the one
    /// [`Self::configuration_schema_value`] builds and
    /// [`Self::configuration_schema`] publishes — which is what keeps what the
    /// schema declares and what a document is held to from ever disagreeing.
    pub fn configuration_violation(
        &self,
        version: &AddonCatalogVersionRecord,
        document: &::serde_json::Value,
    ) -> Option<String> {
        json_schema_violation(
            JSON_SCHEMA_ROOT_PATH,
            &self.configuration_schema_value(version),
            document,
        )
    }
}

/// The path a JSON Schema validator names the whole document by, which every
/// violation below is reported relative to.
const JSON_SCHEMA_ROOT_PATH: &str = "$";

/// How `value` fails `schema`, as a validator states it — the path at fault and
/// what is wrong with it — or `None` when it satisfies the schema.
///
/// It reads the four keywords the add-on configuration schemas above are written
/// with: `type`, `properties`, `required` and `additionalProperties`. The first
/// violation found is the one reported, and the walk is depth-first through the
/// properties the SCHEMA declares, so its depth is bounded by the nesting of
/// these schemas rather than by the document a caller sent; the document itself
/// is parsed by `serde_json`, not here.
fn json_schema_violation(
    path: &str,
    schema: &::serde_json::Value,
    value: &::serde_json::Value,
) -> Option<String> {
    if let Some(expected) = schema.get("type").and_then(::serde_json::Value::as_str) {
        if !json_value_has_type(expected, value) {
            return Some(format!(
                "{path}: {} found, {expected} expected",
                json_type_name(value)
            ));
        }
    }
    let members = value.as_object()?;
    let properties = schema
        .get("properties")
        .and_then(::serde_json::Value::as_object);
    // `additionalProperties: false` is what makes a key the schema does not
    // declare a violation rather than a value carried along unread.
    if schema.get("additionalProperties") == Some(&::serde_json::Value::Bool(false)) {
        if let Some(undeclared) = members.keys().find(|key| {
            properties.is_none_or(|declared| !declared.contains_key(key.as_str()))
        }) {
            return Some(format!(
                "{path}.{undeclared}: is not defined in the schema and the schema does not \
                 allow additional properties"
            ));
        }
    }
    for required in schema
        .get("required")
        .and_then(::serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        if let Some(property) = required
            .as_str()
            .filter(|property| !members.contains_key(*property))
        {
            return Some(format!("{path}.{property}: is missing but it is required"));
        }
    }
    let properties = properties?;
    members.iter().find_map(|(key, member)| {
        let declared = properties.get(key)?;
        json_schema_violation(&format!("{path}.{key}"), declared, member)
    })
}

/// Whether `value` is of the JSON Schema `type` named by `expected`.
///
/// A type the add-on configuration schemas do not use constrains nothing, which
/// is what an unknown keyword value means in JSON Schema.
fn json_value_has_type(expected: &str, value: &::serde_json::Value) -> bool {
    match expected {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        // In the dialect these schemas name, an integer is any number with a
        // zero fractional part, so `2.0` is an integer and `1.5` is not.
        "integer" => {
            value.is_i64()
                || value.is_u64()
                || value.as_f64().is_some_and(|number| number.fract() == 0.0)
        }
        "number" => value.is_number(),
        "null" => value.is_null(),
        _ => true,
    }
}

/// The JSON Schema type name of a value, as a violation names what it found.
fn json_type_name(value: &::serde_json::Value) -> &'static str {
    match value {
        ::serde_json::Value::Null => "null",
        ::serde_json::Value::Bool(_) => "boolean",
        ::serde_json::Value::Number(number) => {
            if number.is_f64() {
                "number"
            } else {
                "integer"
            }
        }
        ::serde_json::Value::String(_) => "string",
        ::serde_json::Value::Array(_) => "array",
        ::serde_json::Value::Object(_) => "object",
    }
}

/// The AWS Marketplace listing one Marketplace add-on is subscribed through.
///
/// `MarketplaceInformation` is the listing's `productId` and `productUrl`. This
/// emulator holds no AWS Marketplace subscription, so there is no real listing
/// to report: both are DERIVED from the add-on's name through the one
/// derivation this service's UUID-shaped identifiers use
/// ([`derive_uuid_from_seed`]), which makes them distinct per add-on and stable
/// for as long as the catalog publishes it — the properties a caller can rely
/// on — and the URL is the `prodview-<id>` form a Marketplace listing is
/// published at.
fn marketplace_listing(addon_name: &str) -> MarketplaceInformation {
    let product_id = derive_uuid_from_seed(addon_name);
    let view_id: String = product_id.chars().filter(char::is_ascii_alphanumeric).take(13).collect();
    MarketplaceInformation {
        product_id: Some(product_id),
        product_url: Some(format!(
            "https://aws.amazon.com/marketplace/pp/prodview-{view_id}"
        )),
    }
}

/// Build the add-on catalog this service publishes from [`ADDON_CATALOG`].
///
/// An add-on publishes one build per Kubernetes minor version the calendar
/// carries, along its own version line: the build published alongside a
/// Kubernetes version supports that version and the one before it — an add-on
/// build keeps working on the cluster version it was released on — and it is the
/// DEFAULT version for the Kubernetes version it was published for. So for any
/// Kubernetes version the catalog names exactly ONE default build per add-on,
/// while the newer build that still supports that version is offered as a
/// non-default choice, which is what `CreateAddon` resolves an absent
/// `addonVersion` through ([`ComAmazonawsEksAwswesleyfrontendState::default_addon_version`]).
///
/// Each add-on's builds come out NEWEST FIRST, the order the catalog publishes
/// them in; the add-ons themselves keep [`ADDON_CATALOG`]'s name order.
fn seed_addon_catalog() -> Vec<AddonCatalogRecord> {
    ADDON_CATALOG
        .iter()
        .map(|seed| {
            let (major, minor, patch) = seed.oldest_build;
            let mut versions: Vec<AddonCatalogVersionRecord> = VERSION_CALENDAR
                .iter()
                .enumerate()
                .map(|(index, (cluster_version, release, _, _))| {
                    let mut compatibilities = Vec::with_capacity(2);
                    if let Some((previous, _, _, _)) = index.checked_sub(1).and_then(|earlier| VERSION_CALENDAR.get(earlier))
                    {
                        compatibilities.push(AddonCompatibilityRecord {
                            cluster_version: (*previous).to_string(),
                            platform_versions: vec![ADDON_ANY_PLATFORM_VERSION.to_string()],
                            default_version: false,
                        });
                    }
                    compatibilities.push(AddonCompatibilityRecord {
                        cluster_version: (*cluster_version).to_string(),
                        platform_versions: vec![ADDON_ANY_PLATFORM_VERSION.to_string()],
                        default_version: true,
                    });
                    AddonCatalogVersionRecord {
                        addon_version: format!(
                            "v{major}.{}.{patch}-eksbuild.1",
                            minor + index as u32
                        ),
                        published_at: calendar_instant(release),
                        architecture: ADDON_ARCHITECTURES
                            .iter()
                            .map(|architecture| (*architecture).to_string())
                            .collect(),
                        compatibilities,
                        required_configuration: seed
                            .required_configuration
                            .iter()
                            .map(|property| (*property).to_string())
                            .collect(),
                        requires_iam_permissions: seed.requires_iam_permissions,
                    }
                })
                .collect();
            versions.reverse();
            AddonCatalogRecord {
                addon_name: seed.addon_name.to_string(),
                addon_type: seed.addon_type.to_string(),
                owner: seed.owner.to_string(),
                publisher: seed.publisher.to_string(),
                default_namespace: seed.default_namespace.to_string(),
                workload: seed.workload,
                required_addons: seed
                    .required_addons
                    .iter()
                    .map(|required| (*required).to_string())
                    .collect(),
                admission_webhook: seed.admission_webhook,
                marketplace_information: (seed.owner == ADDON_OWNER_MARKETPLACE)
                    .then(|| marketplace_listing(seed.addon_name)),
                pod_identity: seed
                    .pod_identity
                    .iter()
                    .map(|(service_account, policies)| {
                        (
                            (*service_account).to_string(),
                            policies.iter().map(|policy| (*policy).to_string()).collect(),
                        )
                    })
                    .collect(),
                configuration_properties: ADDON_COMMON_CONFIGURATION_PROPERTIES
                    .iter()
                    .chain(seed.configuration_properties)
                    .map(|(property, json_type)| {
                        ((*property).to_string(), (*json_type).to_string())
                    })
                    // A `BTreeMap` so a key the row declares a type of its own
                    // for overrides the common one, and the properties come out
                    // in the schema's alphabetical order with no duplicates.
                    .collect::<BTreeMap<String, String>>()
                    .into_iter()
                    .collect(),
                versions,
            }
        })
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
    /// Fargate profiles by `(region, cluster name, Fargate profile name)`.
    ///
    /// A Fargate profile is a resource INSIDE one cluster — it is addressed as
    /// `/clusters/{clusterName}/fargate-profiles/{fargateProfileName}` — and a
    /// cluster name identifies a cluster only within one region (see
    /// [`Self::clusters`]), so the key is the whole triple: the same profile name
    /// may exist in another cluster or another region. A `BTreeMap` so a listing
    /// walks one cluster's profiles as a contiguous, name-sorted range. Its size
    /// is bounded by [`MAX_STORED_FARGATE_PROFILES`], which
    /// `CreateFargateProfile` enforces.
    pub fargate_profiles: BTreeMap<(String, String, String), FargateProfileRecord>,
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
    /// The add-on catalog `DescribeAddonVersions` reports and
    /// `DescribeAddonConfiguration` answers from, seeded once from
    /// [`ADDON_CATALOG`] (see [`AddonCatalogRecord`]) and never written by a
    /// request: it is what Amazon EKS publishes, not per-account data. A `Vec`
    /// because the catalog has a publication ORDER (add-on name order, see
    /// [`seed_addon_catalog`]) that the listing reports it in.
    pub addon_catalog: Vec<AddonCatalogRecord>,
    /// EKS Anywhere subscriptions by `(region, id)`.
    ///
    /// A subscription is an ACCOUNT-level support contract rather than a resource
    /// of a cluster, and it is addressed by the id the create mints
    /// (`DescribeEksAnywhereSubscription$id`), so the key is the Region the
    /// request is signed for plus that id — the Region because the name must be
    /// "unique in your Amazon Web Services account in the Amazon Web Services
    /// Region you're creating the subscription in"
    /// (`CreateEksAnywhereSubscriptionRequest$name`), so a subscription belongs to
    /// one Region as every other record here does. A `BTreeMap` so a listing walks
    /// one Region's subscriptions as a contiguous, sorted range. Its size is
    /// bounded by [`MAX_STORED_SUBSCRIPTIONS`], which
    /// `CreateEksAnywhereSubscription` enforces.
    pub eks_anywhere_subscriptions: BTreeMap<(String, String), EksAnywhereSubscriptionRecord>,
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
            fargate_profiles: BTreeMap::new(),
            addons: BTreeMap::new(),
            capabilities: BTreeMap::new(),
            pod_identity_associations: BTreeMap::new(),
            eks_anywhere_subscriptions: BTreeMap::new(),
            minted_associations: 0,
            cluster_versions: seed_cluster_versions(),
            addon_catalog: seed_addon_catalog(),
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
        // The ADD-ONS go first: completing an add-on's update may FAIL it (see
        // [`Self::complete_addon_update`]), and the cluster pass below is what
        // would otherwise already have reported that same update `Successful`.
        let mut completed: Vec<((String, String, String), SystemTime, PendingAddonUpdate)> =
            Vec::new();
        for (key, addon) in self.addons.iter_mut() {
            if let Some((at, pending)) = addon.materialize_due_transitions(now) {
                completed.push((key.clone(), at, pending));
            }
        }
        for (key, at, pending) in completed {
            self.complete_addon_update(&key, at, pending);
        }
        for cluster in self.clusters.values_mut() {
            cluster.materialize_due_transitions(now);
        }
        for nodegroup in self.nodegroups.values_mut() {
            nodegroup.materialize_due_transitions(now);
        }
        for profile in self.fargate_profiles.values_mut() {
            profile.materialize_due_transitions(now);
        }
        for capability in self.capabilities.values_mut() {
            capability.materialize_due_transitions(now);
        }
        for subscription in self.eks_anywhere_subscriptions.values_mut() {
            subscription.materialize_due_transitions(now);
        }
        self.resettle_addon_replicas(now);
    }

    /// Bring every settled add-on's status and health back in line with what the
    /// cluster can run NOW — the last pass, so it reads the node groups and
    /// Fargate profiles the passes above have just moved.
    ///
    /// Whether an add-on's replicas are running is a value DERIVED from the
    /// cluster's compute, not a fact of the add-on: a replica-less add-on
    /// recovers once the cluster has somewhere to schedule its pod (a node group
    /// finishing, a Fargate profile that selects its namespace, Auto Mode), and
    /// one whose compute has gone is short of replicas again. Recomputing it is
    /// therefore the same kind of work as materialising a clock-owed move: the
    /// answer was already true at this instant whether or not anybody called.
    ///
    /// Only the two states that are ABOUT the replicas are recomputed. An
    /// install that never came up is terminal — such an add-on is removed and
    /// installed again rather than recovering on its own — and one still
    /// settling, taking an update or being deleted has its own pass above.
    fn resettle_addon_replicas(&mut self, now: SystemTime) {
        let settled: Vec<((String, String, String), String)> = self
            .addons
            .iter()
            .filter(|(_, addon)| {
                addon.pending_update.is_none()
                    && addon.active_at.is_none()
                    && matches!(
                        addon.status.as_str(),
                        ADDON_STATUS_ACTIVE | ADDON_STATUS_DEGRADED
                    )
            })
            .map(|(key, addon)| (key.clone(), addon.namespace.clone()))
            .collect();
        for (key, namespace) in settled {
            let outcome = self.addon_replica_outcome(&key.0, &key.1, &key.2, &namespace, now);
            if let Some(addon) = self.addons.get_mut(&key) {
                if addon.status != outcome.status {
                    addon.status = outcome.status;
                    addon.health_issues = outcome.issues;
                }
            }
        }
    }

    /// Complete the add-on update the clock owed at `at`: write the change it
    /// carries, or fail it where that change can no longer be made.
    ///
    /// An add-on update is asynchronous, so the cluster can have taken another
    /// association for a `(namespace, service account)` pair the change asks for
    /// between the call that started the update and this instant. Mapping that
    /// pair twice is the ambiguity the pod identity slice refuses, so the update
    /// FAILS — the add-on keeps every value it had, reports `UPDATE_FAILED` with
    /// the issue that says why, and the update reports `Failed` rather than the
    /// `Successful` the clock owed it.
    ///
    /// Both sides of that — the add-on, and the associations and the update record
    /// — are only reachable here, which is why this lives on the store rather than
    /// on [`AddonRecord`].
    fn complete_addon_update(
        &mut self,
        key: &(String, String, String),
        at: SystemTime,
        pending: PendingAddonUpdate,
    ) {
        let (region, cluster_name, _) = key.clone();
        let Some(addon) = self.addons.get(key) else {
            return;
        };
        let owner_arn = addon.arn.clone();
        let namespace = addon.namespace.clone();
        // Only an association ANOTHER owner holds can conflict: the ones this
        // add-on owns are the ones the requested set replaces.
        let taken: ::std::collections::BTreeSet<String> = self
            .pod_identity_associations_in(&region, &cluster_name)
            .filter(|record| {
                record.namespace == namespace
                    && record.owner_arn.as_deref() != Some(owner_arn.as_str())
            })
            .map(|record| record.service_account.clone())
            .collect();
        let conflict = pending
            .pod_identity_associations
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|association| association.service_account.clone())
            .find(|service_account| taken.contains(service_account));

        if let Some(service_account) = conflict {
            if let Some(addon) = self.addons.get_mut(key) {
                addon.fail_update(
                    at,
                    AddonIssue {
                        code: Some(ADDON_ISSUE_CONFIGURATION_CONFLICT.to_string()),
                        message: Some(association_already_exists(&namespace, &service_account)),
                        resource_ids: Some(vec![service_account]),
                    },
                );
            }
            if let Some(update) = self
                .clusters
                .get_mut(&(region, cluster_name))
                .and_then(|cluster| cluster.updates.get_mut(&pending.update_id))
            {
                update.status = UPDATE_FAILED.to_string();
                update.succeeds_at = None;
            }
            return;
        }

        if let Some(addon) = self.addons.get_mut(key) {
            addon.apply_update(at, &pending);
        }
        if let Some(requested) = pending.pod_identity_associations.as_deref() {
            self.replace_addon_associations(key, requested, at);
        }
    }

    /// Make the EKS Pod Identity associations one add-on owns be exactly
    /// `requested`, which is what completing an add-on update that named them
    /// does.
    ///
    /// "An array of EKS Pod Identity associations to be updated... If an empty
    /// array is provided, existing associations owned by the add-on are deleted"
    /// (`UpdateAddonRequest$podIdentityAssociations`), so the requested list is
    /// the SET the add-on owns afterwards: the ones it owned are dropped and one
    /// record is created per requested entry — which is also what deletes them
    /// all when the list is empty.
    fn replace_addon_associations(
        &mut self,
        key: &(String, String, String),
        requested: &[AddonPodIdentityAssociations],
        at: SystemTime,
    ) {
        let (region, cluster_name, _) = key.clone();
        let Some(addon) = self.addons.get(key) else {
            return;
        };
        let owner_arn = addon.arn.clone();
        let namespace = addon.namespace.clone();
        self.remove_associations_owned_by(&owner_arn);
        let mut association_arns = Vec::with_capacity(requested.len());
        for association in requested {
            association_arns.push(self.create_addon_owned_association(
                &region,
                &cluster_name,
                &namespace,
                &owner_arn,
                association,
                at,
            ));
        }
        if let Some(addon) = self.addons.get_mut(key) {
            addon.pod_identity_association_arns = association_arns;
        }
    }

    /// Create the EKS Pod Identity association an add-on owns for one entry of
    /// the `podIdentityAssociations` it was asked for, and answer that
    /// association's ARN.
    ///
    /// The id is minted through [`Self::mint_association_id`] — the same path
    /// `CreatePodIdentityAssociation` takes — so an association an add-on owns is
    /// unique within its cluster and has the form every other association id has,
    /// and it carries the add-on's ARN as its `ownerArn`: "If defined, the EKS Pod
    /// Identity association is owned by an Amazon EKS add-on"
    /// (`PodIdentityAssociation$ownerArn`). One path for both the install that
    /// creates such an association and the update that replaces it, so the two
    /// cannot write different records for the same request member.
    pub fn create_addon_owned_association(
        &mut self,
        region: &str,
        cluster_name: &str,
        namespace: &str,
        owner_arn: &str,
        association: &AddonPodIdentityAssociations,
        at: SystemTime,
    ) -> String {
        let association_id =
            self.mint_association_id(region, cluster_name, namespace, &association.service_account);
        let association_arn = derive_association_arn(region, cluster_name, &association_id);
        let record = PodIdentityAssociationRecord {
            cluster_name: cluster_name.to_string(),
            association_id: association_id.clone(),
            association_arn: association_arn.clone(),
            namespace: namespace.to_string(),
            service_account: association.service_account.clone(),
            role_arn: association.role_arn.clone(),
            // An add-on asks only for a service account and a role, so an
            // association it owns names no target role — and `externalId` exists
            // to be matched in a TARGET role's trust policy, so there is nothing
            // to condition — no permissions policy and no session-tag override.
            target_role_arn: None,
            external_id: None,
            policy: None,
            disable_session_tags: None,
            // "Tags don't propagate to any other cluster or Amazon Web Services
            // resources" (`CreateAddon$tags`), so the add-on's tags are not
            // copied onto the associations it owns.
            tags: BTreeMap::new(),
            created_at: at,
            modified_at: at,
            // The association is created by the add-on rather than by a call of
            // the pod identity slice, so no association-level idempotency token
            // applies to it.
            client_request_token: None,
            update_request_token: None,
            owner_arn: Some(owner_arn.to_string()),
        };
        self.pod_identity_associations.insert(
            (region.to_string(), cluster_name.to_string(), association_id),
            record,
        );
        association_arn
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

    /// The add-ons the catalog publishes at `now`, in the catalog's own
    /// publication order (add-on name order).
    ///
    /// An add-on none of whose builds Amazon EKS has published yet is not one
    /// the catalog offers, so it is not reported at all.
    pub fn published_addons(
        &self,
        now: SystemTime,
    ) -> impl Iterator<Item = &AddonCatalogRecord> + '_ {
        self.addon_catalog
            .iter()
            .filter(move |addon| addon.published_versions(now).next().is_some())
    }

    /// The catalog entry for one add-on name, when the catalog publishes it at
    /// `now`.
    pub fn published_addon(
        &self,
        addon_name: &str,
        now: SystemTime,
    ) -> Option<&AddonCatalogRecord> {
        self.published_addons(now)
            .find(|addon| addon.addon_name == addon_name)
    }

    /// The catalog entry and the published build one `(addonName, addonVersion)`
    /// pair names, or `None` when the catalog publishes no such build at `now`.
    ///
    /// It is the pair `DescribeAddonConfiguration` resolves, so whatever reads a
    /// build's configuration schema reads the same build that operation
    /// publishes it for.
    pub fn published_addon_build(
        &self,
        addon_name: &str,
        addon_version: &str,
        now: SystemTime,
    ) -> Option<(&AddonCatalogRecord, &AddonCatalogVersionRecord)> {
        let addon = self.published_addon(addon_name, now)?;
        let version = addon
            .published_versions(now)
            .find(|version| version.addon_version == addon_version)?;
        Some((addon, version))
    }

    /// Whether the catalog publishes any build of one add-on that supports one
    /// Kubernetes version at `now`.
    ///
    /// This is what "the name must match one of the names returned by
    /// `DescribeAddonVersions`" (`CreateAddon$addonName`) means for a request
    /// against one cluster: the names that listing reports when it is narrowed
    /// to that cluster's Kubernetes version, which is exactly the add-ons that
    /// have a build for it.
    pub fn publishes_addon_for(
        &self,
        addon_name: &str,
        kubernetes_version: &str,
        now: SystemTime,
    ) -> bool {
        self.published_addon(addon_name, now).is_some_and(|addon| {
            addon
                .published_versions(now)
                .any(|version| version.supports(kubernetes_version))
        })
    }

    /// Whether the catalog publishes the build one `(addonName, addonVersion)`
    /// pair names AND lists one Kubernetes version among that build's
    /// compatibilities at `now`.
    ///
    /// This is what "the version must match one of the versions returned by
    /// `DescribeAddonVersions`" (`CreateAddon$addonVersion`) means for a request
    /// against one cluster: a build the catalog does not publish at all, and a
    /// published build whose `compatibilities` do not carry the cluster's
    /// Kubernetes version, are both builds that listing does not offer for it.
    pub fn publishes_addon_build_for(
        &self,
        addon_name: &str,
        addon_version: &str,
        kubernetes_version: &str,
        now: SystemTime,
    ) -> bool {
        self.published_addon_build(addon_name, addon_version, now)
            .is_some_and(|(_, build)| build.supports(kubernetes_version))
    }

    /// The version the catalog publishes as the DEFAULT build of one add-on for
    /// one Kubernetes version — the version `CreateAddon` installs when the
    /// caller names none — or `None` when this catalog publishes no build of
    /// that add-on for that Kubernetes version.
    pub fn default_addon_version(
        &self,
        addon_name: &str,
        kubernetes_version: &str,
        now: SystemTime,
    ) -> Option<String> {
        self.published_addon(addon_name, now)?
            .published_versions(now)
            .find(|version| version.is_default_for(kubernetes_version))
            .map(|version| version.addon_version.clone())
    }

    /// The add-on one `(region, cluster, addonName)` triple names, when that
    /// cluster holds it.
    pub fn installed_addon(
        &self,
        region: &str,
        cluster_name: &str,
        addon_name: &str,
    ) -> Option<&AddonRecord> {
        self.addons.get(&(
            region.to_string(),
            cluster_name.to_string(),
            addon_name.to_string(),
        ))
    }

    /// Whether `cluster_name` has compute a pod in `namespace` can be scheduled
    /// onto.
    ///
    /// Three things give a cluster somewhere to run a pod: EKS Auto Mode, which
    /// provisions nodes for the pods the cluster is asked to run; a managed node
    /// group that is RUNNING and scaled to at least one node; and a Fargate
    /// profile that is running and SELECTS the pod's namespace, since Fargate
    /// runs only the pods a profile selects. A cluster with none of them runs
    /// nothing.
    ///
    /// The status matters as much as the count on both: a node group or profile
    /// the cluster is still creating carries no pod yet, which is why this is
    /// re-read rather than answered once (see [`Self::materialize_due_transitions`]).
    pub fn cluster_can_place_pods(
        &self,
        region: &str,
        cluster_name: &str,
        namespace: &str,
    ) -> bool {
        if self
            .clusters
            .get(&(region.to_string(), cluster_name.to_string()))
            .is_some_and(ClusterRecord::is_auto_mode)
        {
            return true;
        }
        if self.nodegroups_of(region, cluster_name).any(|nodegroup| {
            nodegroup.scaling_config.desired_size.unwrap_or_default() > 0
                && matches!(nodegroup.status.as_str(), STATUS_ACTIVE | STATUS_UPDATING)
        }) {
            return true;
        }
        self.fargate_profiles_of(region, cluster_name)
            .any(|profile| {
                profile.status == FARGATE_PROFILE_STATUS_ACTIVE
                    && profile
                        .selectors
                        .iter()
                        .any(|selector| selector.namespace.as_deref() == Some(namespace))
            })
    }

    /// The terminal state an install of `addon_name` into `namespace` on one
    /// cluster is owed, decided from that cluster's own state.
    ///
    /// An add-on install is asynchronous and settles in the state that reflects
    /// what the cluster could actually do with it, which is three questions in
    /// order, each about one thing the install needs:
    ///
    /// 1. the add-ons its resources are applied against must be ON the cluster,
    ///    otherwise the Kubernetes resources it needs are not there and the
    ///    install fails ([`ADDON_ISSUE_K8S_RESOURCE_NOT_FOUND`]);
    /// 2. one that is installed but is NOT RUNNING cannot answer the admission
    ///    review the API server sends its webhook, so every resource that
    ///    webhook gates is refused and the install fails
    ///    ([`ADDON_ISSUE_ADMISSION_REQUEST_DENIED`]);
    /// 3. an add-on with a replica to schedule needs the cluster to have compute
    ///    to schedule it onto; without it the resources are applied but no pod
    ///    of the add-on runs, which is a running add-on that is not healthy
    ///    ([`ADDON_ISSUE_INSUFFICIENT_REPLICAS`]).
    ///
    /// An add-on this catalog does not publish carries none of these
    /// requirements, so its install runs.
    pub fn addon_install_outcome(
        &self,
        region: &str,
        cluster_name: &str,
        addon_name: &str,
        namespace: &str,
        now: SystemTime,
    ) -> AddonInstallOutcome {
        let Some(published) = self.published_addon(addon_name, now) else {
            return AddonInstallOutcome::running();
        };
        let missing: Vec<String> = published
            .required_addons
            .iter()
            .filter(|required| {
                self.installed_addon(region, cluster_name, required)
                    .is_none()
            })
            .cloned()
            .collect();
        if !missing.is_empty() {
            return AddonInstallOutcome::failed(AddonIssue {
                code: Some(ADDON_ISSUE_K8S_RESOURCE_NOT_FOUND.to_string()),
                message: Some(format!(
                    "The Kubernetes resources add-on {addon_name} is installed against are \
                     not on cluster {cluster_name}: {} must be installed first.",
                    missing.join(", ")
                )),
                resource_ids: Some(self.addon_resource_ids(&missing, now)),
            });
        }
        let denied: Vec<String> = published
            .required_addons
            .iter()
            .filter(|required| {
                self.published_addon(required, now)
                    .is_some_and(|entry| entry.admission_webhook)
                    && self
                        .installed_addon(region, cluster_name, required)
                        .is_some_and(|record| !record.is_running())
            })
            .cloned()
            .collect();
        if !denied.is_empty() {
            return AddonInstallOutcome::failed(AddonIssue {
                code: Some(ADDON_ISSUE_ADMISSION_REQUEST_DENIED.to_string()),
                message: Some(format!(
                    "The admission webhook of add-on {} on cluster {cluster_name} is not \
                     running, so the Kubernetes resources of add-on {addon_name} were denied \
                     at admission.",
                    denied.join(", ")
                )),
                resource_ids: Some(self.addon_resource_ids(&denied, now)),
            });
        }
        self.addon_replica_outcome(region, cluster_name, addon_name, namespace, now)
    }

    /// The Kubernetes resources of each of `addon_names`, as an `AddonIssue`
    /// names the resources an issue is about: `<namespace>/<add-on>`, the
    /// namespace being the one the catalog installs that add-on into.
    fn addon_resource_ids(&self, addon_names: &[String], now: SystemTime) -> Vec<String> {
        addon_names
            .iter()
            .map(|addon_name| {
                let namespace = self
                    .published_addon(addon_name, now)
                    .map_or(ADDON_DEFAULT_NAMESPACE, |published| {
                        published.default_namespace.as_str()
                    });
                format!("{namespace}/{addon_name}")
            })
            .collect()
    }

    /// The state an add-on whose resources ARE on the cluster is in: running, or
    /// running without the replica it needs.
    ///
    /// An add-on with a replica to schedule needs the cluster to have compute to
    /// schedule it onto; without it the resources are applied but no pod of the
    /// add-on runs, which is a running add-on that is not healthy. An add-on that
    /// runs one pod per node has nothing to place, and so is healthy on a cluster
    /// with no nodes.
    ///
    /// It is the question an install asks last, and the one every later read asks
    /// again ([`Self::resettle_addon_replicas`]): an add-on reports what the
    /// cluster can do with the resources it carries, at the instant it is read.
    pub fn addon_replica_outcome(
        &self,
        region: &str,
        cluster_name: &str,
        addon_name: &str,
        namespace: &str,
        now: SystemTime,
    ) -> AddonInstallOutcome {
        let replicated = self
            .published_addon(addon_name, now)
            .is_some_and(|published| published.workload == AddonWorkload::Replicated);
        if replicated && !self.cluster_can_place_pods(region, cluster_name, namespace) {
            return AddonInstallOutcome::degraded(AddonIssue {
                code: Some(ADDON_ISSUE_INSUFFICIENT_REPLICAS.to_string()),
                message: Some(format!(
                    "No replica of add-on {addon_name} is running: cluster {cluster_name} has \
                     no compute to schedule a pod in namespace {namespace} onto."
                )),
                resource_ids: Some(vec![format!("{namespace}/{addon_name}")]),
            });
        }
        AddonInstallOutcome::running()
    }

    /// The instant an install of `addon_name` on one cluster is owed its
    /// terminal state.
    ///
    /// One span ([`ADDON_ACTIVATION_DELAY`]) from now for an add-on that waits
    /// for nothing. An add-on applied against another add-on's resources cannot
    /// settle before THAT add-on has, so when one it is installed against is
    /// still settling, its own span starts from the last of their due instants:
    /// a dependency level costs a span, which is what makes the install of a
    /// chain take one span per level.
    pub fn addon_install_settles_at(
        &self,
        region: &str,
        cluster_name: &str,
        addon_name: &str,
        now: SystemTime,
    ) -> SystemTime {
        let required = self
            .published_addon(addon_name, now)
            .map(|published| published.required_addons.as_slice())
            .unwrap_or_default();
        let last_prerequisite = required
            .iter()
            .filter_map(|required| self.installed_addon(region, cluster_name, required))
            .filter_map(|record| record.active_at)
            .max();
        last_prerequisite.unwrap_or(now).max(now) + ADDON_ACTIVATION_DELAY
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

    /// Drop the EKS Pod Identity associations one owner holds.
    ///
    /// An association an add-on owns exists only for that add-on
    /// (`PodIdentityAssociation$ownerArn`), so whatever stops the add-on owning
    /// it — the update that replaces the set it owns, or the delete that takes
    /// the add-on off its cluster — removes it through here, so both write the
    /// same thing.
    pub fn remove_associations_owned_by(&mut self, owner_arn: &str) {
        self.pod_identity_associations
            .retain(|_, record| record.owner_arn.as_deref() != Some(owner_arn));
    }

    /// Take one add-on off its cluster and answer the record that left, carrying
    /// the final status the delete reports it in, or `None` when the cluster
    /// holds no add-on under that key.
    ///
    /// Removing an add-on removes what Amazon EKS manages FOR it, which is the
    /// EKS Pod Identity associations it owns. `preserve` is the request member
    /// that asks for the add-on software to stay on the cluster with Amazon EKS
    /// no longer managing its settings, and then "if an IAM account is associated
    /// with the add-on, it isn't removed" — so those associations are left in
    /// place. Either way the add-on itself stops being an Amazon EKS-managed
    /// add-on of this cluster, which is what this store holds.
    ///
    /// A change an update still in flight carries can no longer be made once the
    /// add-on it would change is gone, so that update stops being owed its
    /// completion and reports the same status as any other change that could not
    /// be made (see [`Self::complete_addon_update`]).
    pub fn remove_addon(
        &mut self,
        key: &(String, String, String),
        preserve: bool,
        at: SystemTime,
    ) -> Option<AddonRecord> {
        let mut record = self.addons.remove(key)?;
        if !preserve {
            self.remove_associations_owned_by(&record.arn);
        }
        if let Some(pending) = record.pending_update.take() {
            if let Some(update) = self
                .clusters
                .get_mut(&(key.0.clone(), key.1.clone()))
                .and_then(|cluster| cluster.updates.get_mut(&pending.update_id))
            {
                update.status = UPDATE_FAILED.to_string();
                update.succeeds_at = None;
            }
        }
        record.status = ADDON_STATUS_DELETING.to_string();
        // The add-on has left the store, so the status move the clock owed it is
        // no longer owed to anybody.
        record.active_at = None;
        record.modified_at = at;
        Some(record)
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

    /// The node groups this store holds in `cluster_name` in `region`, in
    /// node-group-name order.
    ///
    /// The map is keyed `(region, cluster, nodegroup)`, so one cluster's node
    /// groups are a contiguous, name-sorted RANGE of it — the same walk
    /// [`Self::capabilities_of`] makes over its own child resource. This is the
    /// records; [`Self::nodegroup_names_in`] is the names.
    pub fn nodegroups_of<'a>(
        &'a self,
        region: &'a str,
        cluster_name: &'a str,
    ) -> impl Iterator<Item = &'a NodegroupRecord> + 'a {
        self.nodegroups
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

    /// The add-ons installed on one cluster, in add-on-name order.
    ///
    /// The map is keyed `(region, cluster, addon)`, so one cluster's add-ons are a
    /// contiguous, name-sorted RANGE of it, the same walk [`Self::nodegroups_of`]
    /// makes over its own child resource. This is the records;
    /// [`Self::addon_names_in`] is the names.
    pub fn addons_of<'a>(
        &'a self,
        region: &'a str,
        cluster_name: &'a str,
    ) -> impl Iterator<Item = &'a AddonRecord> + 'a {
        self.addons
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

    /// The managed node groups of one cluster whose Kubernetes version is not
    /// `cluster_version`, each as `name (version)`.
    ///
    /// "If your cluster has managed node groups attached to it, all of your node
    /// groups' Kubernetes versions must match the cluster's Kubernetes version in
    /// order to update the cluster to a new Kubernetes version"
    /// (`UpdateClusterVersion`), so this is the one walk both the operation that
    /// REFUSES such an update and the cluster's own upgrade-readiness insight read
    /// the mismatch from — rather than two copies of one rule that can drift.
    pub fn mismatched_nodegroup_versions(
        &self,
        region: &str,
        cluster_name: &str,
        cluster_version: &str,
    ) -> Vec<String> {
        self.nodegroups_of(region, cluster_name)
            .filter(|nodegroup| nodegroup.version != cluster_version)
            .map(|nodegroup| format!("{} ({})", nodegroup.nodegroup_name, nodegroup.version))
            .collect()
    }

    /// Every insight Amazon EKS checks for against one cluster, in
    /// [`InsightCheck::all`] order.
    ///
    /// COMPUTED from what this store holds — the cluster record, the node groups and
    /// add-ons inside it and the add-on catalog — and writing nothing: an insight is
    /// the result of a check, so `ListInsights` and `DescribeInsight` both report
    /// this and cannot disagree. A cluster this store does not hold has no insights
    /// at all, which is what the empty vector says.
    pub fn cluster_insights(&self, region: &str, cluster_name: &str) -> Vec<InsightView> {
        let Some(cluster) = self
            .clusters
            .get(&(region.to_string(), cluster_name.to_string()))
        else {
            return Vec::new();
        };
        let last_refresh_time = cluster.insights_refreshed_at();
        InsightCheck::all()
            .into_iter()
            .map(|check| {
                let evaluated = self.evaluate_insight(check, region, cluster);
                InsightView {
                    id: derive_insight_id(&cluster.arn, check.key()),
                    check,
                    kubernetes_version: evaluated.kubernetes_version,
                    status: evaluated.status.to_string(),
                    reason: evaluated.reason,
                    last_refresh_time,
                    last_transition_time: evaluated.last_transition_time,
                }
            })
            .collect()
    }

    /// The resources one check of one cluster examined, each with the status it has
    /// against that check — `Insight$resources`, "The details about each resource
    /// listed in the insight check result".
    ///
    /// Only a check whose subject is a SET of resources inside the cluster lists any:
    /// the node group check lists the cluster's managed node groups and the add-on
    /// check its installed add-ons, each with its own ARN and its own status, so a
    /// caller reading one insight sees which resource the finding is about. A check
    /// whose subject is the cluster itself lists none — the cluster is not a resource
    /// listed INSIDE its own insight.
    pub fn insight_resources(
        &self,
        check: InsightCheck,
        region: &str,
        cluster_name: &str,
    ) -> Vec<InsightResourceDetail> {
        let Some(cluster) = self
            .clusters
            .get(&(region.to_string(), cluster_name.to_string()))
        else {
            return Vec::new();
        };
        // `kubernetesResourceUri` is "The Kubernetes resource URI if applicable": a
        // node group and an add-on are Amazon EKS resources addressed by ARN, not
        // objects of the Kubernetes API, so neither carries one.
        match check {
            InsightCheck::NodegroupVersions => {
                let Some(cluster_version) = cluster.version.as_deref() else {
                    return Vec::new();
                };
                self.nodegroups_of(region, &cluster.name)
                    .map(|nodegroup| {
                        let matches = nodegroup.version == cluster_version;
                        InsightResourceDetail {
                            insight_status: Some(InsightStatus {
                                status: Some(
                                    if matches {
                                        INSIGHT_STATUS_PASSING
                                    } else {
                                        INSIGHT_STATUS_ERROR
                                    }
                                    .to_string(),
                                ),
                                reason: Some(if matches {
                                    format!(
                                        "The node group runs the cluster's Kubernetes version \
                                         {cluster_version}."
                                    )
                                } else {
                                    format!(
                                        "The node group runs Kubernetes version {} instead of the \
                                         cluster's {cluster_version}.",
                                        nodegroup.version
                                    )
                                }),
                            }),
                            kubernetes_resource_uri: None,
                            arn: Some(nodegroup.arn.clone()),
                        }
                    })
                    .collect()
            }
            InsightCheck::AddonVersions => {
                let Some(target) = cluster
                    .version
                    .as_deref()
                    .and_then(next_kubernetes_minor_version)
                else {
                    return Vec::new();
                };
                self.addons_of(region, &cluster.name)
                    .map(|addon| {
                        let published = self
                            .addon_catalog
                            .iter()
                            .find(|entry| entry.addon_name == addon.addon_name)
                            .and_then(|entry| {
                                entry
                                    .versions
                                    .iter()
                                    .find(|build| build.addon_version == addon.addon_version)
                            });
                        let (status, reason) = match published {
                            Some(build) if !build.supports(&target) => (
                                INSIGHT_STATUS_ERROR,
                                format!(
                                    "Version {} of the add-on is not published for Kubernetes \
                                     version {target}.",
                                    addon.addon_version
                                ),
                            ),
                            Some(_) => (
                                INSIGHT_STATUS_PASSING,
                                format!(
                                    "Version {} of the add-on is published for Kubernetes version \
                                     {target}.",
                                    addon.addon_version
                                ),
                            ),
                            // A build the catalog does not publish is one whose
                            // compatibilities this service does not know, which is
                            // what `UNKNOWN` says.
                            None => (
                                INSIGHT_STATUS_UNKNOWN,
                                format!(
                                    "Version {} of the add-on is not one Amazon EKS publishes, so \
                                     the versions it supports are not known.",
                                    addon.addon_version
                                ),
                            ),
                        };
                        InsightResourceDetail {
                            insight_status: Some(InsightStatus {
                                status: Some(status.to_string()),
                                reason: Some(reason),
                            }),
                            kubernetes_resource_uri: None,
                            arn: Some(addon.arn.clone()),
                        }
                    })
                    .collect()
            }
            InsightCheck::HybridNodeNetworks | InsightCheck::VersionRollback => Vec::new(),
        }
    }

    /// Evaluate ONE check against one cluster.
    ///
    /// Each check reports the status its own subject justifies, the reason naming
    /// what it found in this store, the Kubernetes version the check is ABOUT (the
    /// version the cluster would move to for an upgrade check, the version it would
    /// return to for the rollback check, and none for a check that is about no
    /// version), and the instant that subject last changed — which is when the
    /// check's status can last have changed.
    fn evaluate_insight(
        &self,
        check: InsightCheck,
        region: &str,
        cluster: &ClusterRecord,
    ) -> InsightEvaluation {
        // "A cluster moves one minor version per update", so the version an upgrade
        // check is about is the minor version right after the cluster's own.
        let upgrade_target = cluster
            .version
            .as_deref()
            .and_then(next_kubernetes_minor_version);
        match check {
            InsightCheck::NodegroupVersions => {
                // The node groups are the subject, so the check's status can only
                // have changed when one of them was created or last changed.
                let last_transition_time = self
                    .nodegroups_of(region, &cluster.name)
                    .map(|nodegroup| nodegroup.modified_at)
                    .chain(std::iter::once(cluster.created_at))
                    .max()
                    .unwrap_or(cluster.created_at);
                let Some(cluster_version) = cluster.version.as_deref() else {
                    return InsightEvaluation {
                        status: INSIGHT_STATUS_UNKNOWN,
                        reason: "The cluster reports no Kubernetes version, so its node groups \
                                 could not be compared against one."
                            .to_string(),
                        kubernetes_version: None,
                        last_transition_time,
                    };
                };
                let mismatched =
                    self.mismatched_nodegroup_versions(region, &cluster.name, cluster_version);
                if mismatched.is_empty() {
                    InsightEvaluation {
                        status: INSIGHT_STATUS_PASSING,
                        reason: format!(
                            "Every managed node group of the cluster runs Kubernetes version \
                             {cluster_version}."
                        ),
                        kubernetes_version: upgrade_target,
                        last_transition_time,
                    }
                } else {
                    InsightEvaluation {
                        status: INSIGHT_STATUS_ERROR,
                        reason: format!(
                            "Managed node groups whose Kubernetes version does not match the \
                             cluster's {cluster_version}: {}.",
                            mismatched.join(", ")
                        ),
                        kubernetes_version: upgrade_target,
                        last_transition_time,
                    }
                }
            }
            InsightCheck::AddonVersions => {
                let last_transition_time = self
                    .addons_of(region, &cluster.name)
                    .map(|addon| addon.modified_at)
                    .chain(std::iter::once(cluster.created_at))
                    .max()
                    .unwrap_or(cluster.created_at);
                let Some(target) = upgrade_target else {
                    return InsightEvaluation {
                        status: INSIGHT_STATUS_UNKNOWN,
                        reason: "The cluster reports no Kubernetes version, so no add-on could be \
                                 checked against the version it would be updated to."
                            .to_string(),
                        kubernetes_version: None,
                        last_transition_time,
                    };
                };
                // An installed build the catalog does not publish is not one this
                // service knows the compatibilities of, so it is not reported as
                // incompatible: only a build the catalog carries can fail the check.
                let incompatible: Vec<String> = self
                    .addons_of(region, &cluster.name)
                    .filter(|addon| {
                        self.addon_catalog
                            .iter()
                            .find(|entry| entry.addon_name == addon.addon_name)
                            .and_then(|entry| {
                                entry
                                    .versions
                                    .iter()
                                    .find(|build| build.addon_version == addon.addon_version)
                            })
                            .is_some_and(|build| !build.supports(&target))
                    })
                    .map(|addon| format!("{} ({})", addon.addon_name, addon.addon_version))
                    .collect();
                if incompatible.is_empty() {
                    InsightEvaluation {
                        status: INSIGHT_STATUS_PASSING,
                        reason: format!(
                            "Every Amazon EKS add-on installed on the cluster is published for \
                             Kubernetes version {target}."
                        ),
                        kubernetes_version: Some(target),
                        last_transition_time,
                    }
                } else {
                    InsightEvaluation {
                        status: INSIGHT_STATUS_ERROR,
                        reason: format!(
                            "Amazon EKS add-ons installed at a version that is not published for \
                             Kubernetes version {target}: {}.",
                            incompatible.join(", ")
                        ),
                        kubernetes_version: Some(target),
                        last_transition_time,
                    }
                }
            }
            InsightCheck::HybridNodeNetworks => {
                let last_transition_time =
                    cluster.configuration_changed_at(UPDATE_TYPE_REMOTE_NETWORK_CONFIG);
                let remote = cluster.remote_network_config.as_ref();
                let has_node_networks = remote
                    .and_then(|config| config.remote_node_networks.as_ref())
                    .is_some_and(|networks| !networks.is_empty());
                let has_pod_networks = remote
                    .and_then(|config| config.remote_pod_networks.as_ref())
                    .is_some_and(|networks| !networks.is_empty());
                // No remote node networks means the cluster runs no hybrid nodes, so
                // there is no setup to find a misconfiguration in: the check has no
                // subject rather than a passing one.
                let (status, reason) = if !has_node_networks {
                    (
                        INSIGHT_STATUS_UNKNOWN,
                        "The cluster has no remote node networks, so no hybrid nodes setup was \
                         evaluated."
                            .to_string(),
                    )
                } else if !has_pod_networks {
                    (
                        INSIGHT_STATUS_WARNING,
                        "The cluster has remote node networks for hybrid nodes but no remote pod \
                         networks, so pods running on those nodes are not reachable from the rest \
                         of the cluster."
                            .to_string(),
                    )
                } else {
                    (
                        INSIGHT_STATUS_PASSING,
                        "The cluster has both remote node networks and remote pod networks \
                         configured for its hybrid nodes."
                            .to_string(),
                    )
                };
                InsightEvaluation {
                    status,
                    reason,
                    // A hybrid nodes misconfiguration is about the cluster's network
                    // configuration, not about a Kubernetes version.
                    kubernetes_version: None,
                    last_transition_time,
                }
            }
            InsightCheck::VersionRollback => {
                let latest_change = cluster.latest_version_change();
                let last_transition_time = latest_change
                    .map(|(_, changed_at)| changed_at.max(cluster.created_at))
                    .unwrap_or(cluster.created_at);
                let rollback_target = cluster
                    .version
                    .as_deref()
                    .and_then(previous_kubernetes_minor_version);
                match (latest_change, rollback_target) {
                    (Some((UPDATE_TYPE_VERSION, _)), Some(target)) => InsightEvaluation {
                        status: INSIGHT_STATUS_PASSING,
                        reason: format!(
                            "The cluster completed a Kubernetes version update, so its version \
                             can be rolled back to {target}."
                        ),
                        kubernetes_version: Some(target),
                        last_transition_time,
                    },
                    (Some((UPDATE_TYPE_VERSION_ROLLBACK, _)), _) => InsightEvaluation {
                        status: INSIGHT_STATUS_UNKNOWN,
                        reason: "The cluster's last completed Kubernetes version change was a \
                                 rollback, so it has no version update to roll back."
                            .to_string(),
                        kubernetes_version: None,
                        last_transition_time,
                    },
                    _ => InsightEvaluation {
                        status: INSIGHT_STATUS_UNKNOWN,
                        reason: "The cluster has not completed a Kubernetes version update, so it \
                                 has no earlier version to roll back to."
                            .to_string(),
                        kubernetes_version: None,
                        last_transition_time,
                    },
                }
            }
        }
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

    /// The EKS Anywhere subscriptions one Region holds, in subscription-id order.
    ///
    /// The map is keyed `(region, id)`, so one Region's subscriptions are a
    /// contiguous, id-sorted RANGE of it — the walk
    /// `ListEksAnywhereSubscriptions` pages and `CreateEksAnywhereSubscription`
    /// checks a name against.
    pub fn eks_anywhere_subscriptions_in<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = &'a EksAnywhereSubscriptionRecord> + 'a {
        self.eks_anywhere_subscriptions
            .range((
                std::ops::Bound::Included((region.to_string(), String::new())),
                std::ops::Bound::Unbounded,
            ))
            .take_while(move |((key_region, _), _)| key_region == region)
            .map(|(_, record)| record)
    }

    /// The names `ListClusters` projects for `region`, in sorted order.    ///
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

    /// The Fargate profiles this store holds in `cluster_name` in `region`, in
    /// profile-name order.
    ///
    /// The map is keyed `(region, cluster, profile)`, so one cluster's profiles
    /// are a contiguous, name-sorted RANGE of it — the same walk
    /// [`Self::nodegroups_of`] makes over its own child resource.
    pub fn fargate_profiles_of<'a>(
        &'a self,
        region: &'a str,
        cluster_name: &'a str,
    ) -> impl Iterator<Item = &'a FargateProfileRecord> + 'a {
        self.fargate_profiles
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

    /// Drop every Fargate profile of one cluster.
    ///
    /// A Fargate profile is a child of its cluster, so once the cluster is gone
    /// its profiles are gone with it, and a cluster later created with the same
    /// name in the same Region does not inherit them.
    pub fn remove_fargate_profiles_for_cluster(&mut self, region: &str, cluster_name: &str) {
        self.fargate_profiles
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
/// `sequence` is a counter the store hands out
/// ([`ComAmazonawsEksAwswesleyfrontendState::next_update_sequence`]), so two ids
/// minted in the same nanosecond cannot collide. An update id names a resource for
/// correlation, never a credential.
pub fn new_update_id(sequence: u64) -> String {
    new_uuid_v4(sequence)
}

/// The id of the call this service made to ANOTHER service on the caller's
/// behalf, as it is reported back to that caller.
///
/// A request identifier is generated per call — it is what correlates one answer
/// with the call behind it — so it is drawn afresh here and derived from nothing
/// in the request.
pub fn new_peer_request_id() -> String {
    new_uuid_v4(0)
}

/// A fresh version-4 UUID, `8-4-4-4-12` lowercase hex with the version and
/// variant bits RFC 4122 §4.4 fixes — the one minter behind every identifier this
/// service gives that shape to.
///
/// The 128 bits come from three sources hashed together with SHA-256 (the `sha2`
/// crate) and hex-encoded (the `hex` crate): the OS-seeded randomness a
/// `RandomState` draws, the system clock in nanoseconds, and `sequence`.
fn new_uuid_v4(sequence: u64) -> String {
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

/// The Fargate profile's own identifier, the last segment of its
/// `fargateProfileArn`.
///
/// An EKS Fargate profile ARN is
/// `arn:aws:eks:<region>:<account>:fargateprofile/<cluster>/<profile>/<id>`,
/// where the trailing id distinguishes two profiles that carried the same name
/// at different times. It is derived from the region, the two names and the
/// creation instant — so it is distinct per creation and stable — through
/// [`derive_uuid_from_seed`], the one renderer for this service's UUID-shaped
/// identifiers. It is computed once, when the profile is created, and stored on
/// the record; no read recomputes it.
pub fn derive_fargate_profile_id(
    region: &str,
    cluster_name: &str,
    fargate_profile_name: &str,
    created_at: SystemTime,
) -> String {
    let created_nanos = created_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_nanos());
    derive_uuid_from_seed(&format!(
        "{region}/{cluster_name}/{fargate_profile_name}/{created_nanos}"
    ))
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

/// The version `CreateAddon` assigns when the caller names none and the add-on
/// catalog publishes no build of that add-on for the cluster's Kubernetes
/// version.
///
/// "The version of the add-on. The version must match one of the versions
/// returned by `DescribeAddonVersions`" (`CreateAddon$addonVersion`) — and when
/// the member is absent the real service installs the default version for the
/// cluster's Kubernetes version. That default comes from the catalog
/// ([`ComAmazonawsEksAwswesleyfrontendState::default_addon_version`]), which is
/// the ONE source of a published add-on's versions, so the catalog and an
/// installed add-on cannot disagree.
///
/// This is the fallback for a request the catalog cannot answer: the published
/// catalog is the add-ons [`ADDON_CATALOG`] carries for the Kubernetes versions
/// the calendar carries ([`VERSION_CALENDAR`]), so a cluster whose record
/// somehow reports a Kubernetes version outside that calendar still reports a
/// version for its add-ons. It builds one in the `vX.Y.Z-eksbuild.N`
/// form every EKS add-on version takes out of the cluster's own Kubernetes
/// version, so a cluster on one Kubernetes version always reports the same
/// default and the value a caller reads back is the one the record was created
/// with.
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
/// and this is the check that refuses a value no add-on name can be. It is the
/// FORM check that runs BEFORE the catalog one
/// ([`ComAmazonawsEksAwswesleyfrontendState::publishes_addon_for`]), which is
/// what "the name must match one of the names returned by
/// `DescribeAddonVersions`" states: a value no add-on name can be is reported as
/// the form it broke rather than as a catalog miss. The character set is the one
/// every published add-on name uses, EKS-managed (`vpc-cni`) and Marketplace
/// (`<publisher>_<product>`) alike.
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

/// The sentence Amazon EKS refuses an `addonName` the catalog does not publish
/// for the cluster's Kubernetes version with.
///
/// "The name must match one of the names returned by `DescribeAddonVersions`"
/// (`CreateAddon$addonName`), and that listing is per Kubernetes version, so the
/// refusal names both the add-on asked for and the Kubernetes version of the
/// cluster it was asked for — the version held on the cluster record, not
/// anything the request carried.
#[must_use]
pub fn addon_not_supported_message(addon_name: &str, kubernetes_version: &str) -> String {
    format!(
        "Addon {addon_name} specified is not supported in {kubernetes_version} kubernetes \
         version"
    )
}

/// The sentence Amazon EKS refuses an `addonVersion` the catalog does not offer
/// for the cluster's Kubernetes version with.
///
/// It names no value: the build asked for and the cluster's Kubernetes version
/// are both in the request and the record the caller already has, so the
/// refusal is one fixed sentence for every build the catalog does not offer —
/// one it does not publish at all, and one whose `compatibilities` do not carry
/// that Kubernetes version.
pub const ADDON_VERSION_NOT_SUPPORTED_MESSAGE: &str = "Addon version specified is not supported";

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
/// The scope `ListAddons`'s tokens carry, over one cluster's add-on names. See
/// [`CLUSTERS_LISTING`].
pub const ADDONS_LISTING: &str = "addons";
/// The scope `ListNodegroups`'s tokens carry, over one cluster's node group
/// names. See [`CLUSTERS_LISTING`].
pub const NODEGROUPS_LISTING: &str = "node-groups";
/// The scope `ListCertificateAuthorities`'s tokens carry, over one cluster's
/// certificate-authority order keys
/// ([`CertificateAuthorityRecord::page_key`]). See [`CLUSTERS_LISTING`].
pub const CERTIFICATE_AUTHORITIES_LISTING: &str = "certificate-authorities";
/// The scope `ListUpdates`'s tokens carry, over the update ids of one resource
/// of one cluster. See [`CLUSTERS_LISTING`].
pub const UPDATES_LISTING: &str = "updates";
/// The scope `ListInsights`'s tokens carry, over the ids of the insights of one
/// cluster. See [`CLUSTERS_LISTING`].
pub const INSIGHTS_LISTING: &str = "insights";
/// The scope `ListEksAnywhereSubscriptions`'s tokens carry, over the ids of one
/// Region's subscriptions. See [`CLUSTERS_LISTING`].
pub const EKS_ANYWHERE_SUBSCRIPTIONS_LISTING: &str = "eks-anywhere-subscriptions";

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
    ActiveCertificateAuthority, CertificateAuthority, CertificateAuthorityScheduledEvents,
    CertificateAuthoritySummary, CertificateAuthorityValidity,
};

/// The `CertificateAuthoritySigningStatus` members this slice uses. The model's
/// enum (`smithy_query query="shape" shape="CertificateAuthoritySigningStatus"`)
/// is `NOT_USED | ACTIVATING | IN_USE`; `CertificateAuthority$signingStatus` is a
/// plain string member, so the status is stored as the wire token. `ACTIVATING`
/// is the status a CA reports while the activation update that promotes it to
/// the cluster's signer is in flight.
pub const CA_SIGNING_NOT_USED: &str = "NOT_USED";
/// See [`CA_SIGNING_NOT_USED`].
pub const CA_SIGNING_IN_USE: &str = "IN_USE";
/// See [`CA_SIGNING_NOT_USED`].
pub const CA_SIGNING_ACTIVATING: &str = "ACTIVATING";

/// The `CertificateAuthorityDistributionStatus` members this slice uses, from
/// `smithy_query query="shape" shape="CertificateAuthorityDistributionStatus"`
/// (`IN_PROGRESS | COMPLETE | FAILED | DELETING`). `FAILED` is written by no
/// operation here: this emulator's trust distribution never fails. `DELETING` is
/// what a certificate authority reports while the update that removes it from the
/// cluster's trust bundle is in flight.
pub const CA_DISTRIBUTION_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`CA_DISTRIBUTION_IN_PROGRESS`].
pub const CA_DISTRIBUTION_COMPLETE: &str = "COMPLETE";
/// See [`CA_DISTRIBUTION_IN_PROGRESS`].
pub const CA_DISTRIBUTION_DELETING: &str = "DELETING";

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
/// The `UpdateParamType` member that names the signing status an update moves a
/// certificate authority to, from the same enum. It is what distinguishes an
/// activation update from the append update that put the CA on the cluster:
/// both carry the one certificate-authority `UpdateType`.
pub const UPDATE_PARAM_SIGNING_STATUS: &str = "SigningStatus";

/// How long after an activation reverting to the outgoing certificate authority
/// is still available.
///
/// "For a limited period after activation, CA rollback is available to revert to
/// the outgoing CA if needed" (`ActivateCertificateAuthority`) — no length is
/// published for that period, so this span is CHOSEN rather than cited: non-zero,
/// so a caller reading the retired CA straight after the activation still sees
/// `rollbackAvailable` true, and in the same band as the other chosen spans here.
/// The closing instant is stored on the record; nothing sleeps for it.
pub const CA_ROLLBACK_WINDOW: Duration = Duration::from_millis(750);

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
    /// retires an outgoing CA, so this is set on the CA an activation RETIRED,
    /// [`CA_ROLLBACK_WINDOW`] past the instant the activation was owed. A CA that
    /// nothing has superseded reports `rollbackAvailable` false, which is what
    /// this `None` means.
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

    /// The key this certificate authority sorts and pages under in
    /// `ListCertificateAuthorities`, which reports a cluster's certificate
    /// authorities most recently created FIRST.
    ///
    /// One definition serves both the order the listing walks
    /// ([`ClusterRecord::certificate_authorities_newest_first`]) and the key a page
    /// token names ([`encode_scoped_page_token`]), so a token always resumes the
    /// order the listing walks. The key is a string whose BYTE order is the listing
    /// order: the creation instant goes in as the complement of its nanoseconds
    /// since the epoch, zero-padded to the 39 digits of `u128::MAX`, so a later
    /// instant sorts earlier; the id follows it, so two certificate authorities
    /// created within one clock tick still have a total, stable order. An instant
    /// before the epoch has no nanoseconds since it and takes the largest
    /// complement, which sorts it last — where the oldest creation belongs.
    pub fn page_key(&self) -> String {
        let nanos = self
            .created_at
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|since_epoch| since_epoch.as_nanos())
            .unwrap_or(0);
        format!("{:039}/{}", u128::MAX - nanos, self.id)
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
            // This update's work is the trust distribution onto the CA record,
            // not a change to the cluster's own configuration.
            pending_config: None,
            // Nothing has attempted to cancel this update yet; `CancelUpdate`
            // writes both of these.
            cancellation: None,
            cancellation_request_token: None,
            // The trust bundle is the cluster's own, so this is an update of the
            // cluster rather than of a resource inside it.
            scope: UpdateScope::Cluster,
        }
    }
}

/// The promotion an in-flight activation update makes when it completes: which
/// certificate authority becomes the cluster's signer, when, and at whose
/// request.
///
/// It is held on the update ([`PendingClusterConfig::certificate_authority_activation`])
/// rather than written straight away because "this is an asynchronous operation
/// that returns an update object you can track with DescribeUpdate"
/// (`ActivateCertificateAuthority`): the promotion is owed at the instant the
/// update is owed its `Successful` status. `activated_at` is that DUE instant
/// rather than the instant of the call that noticed it, so every caller reading
/// the promoted CA sees the same activation time.
#[derive(Debug, Clone)]
pub struct CertificateAuthorityActivation {
    /// The certificate authority being promoted to signer.
    pub certificate_authority_id: String,
    /// The instant the promotion is owed, which is what the promoted CA reports
    /// as `activatedAt`.
    pub activated_at: SystemTime,
    /// A `CertificateAuthorityActivatedBy` wire token: who asked for it.
    pub activated_by: String,
}

impl CertificateAuthorityActivation {
    /// Promote the named certificate authority to the cluster's signer and retire
    /// whichever certificate authority was signing, which is what completing an
    /// activation update does: "Amazon EKS promotes it to be the cluster's signer
    /// (its signingStatus becomes IN_USE) and the outgoing CA is retired
    /// (NOT_USED). The outgoing CA remains in the cluster's trust bundle but no
    /// longer signs certificates" — so the retired record stays on the cluster,
    /// with the rollback window the activation opens on it.
    pub fn apply(self, record: &mut ClusterRecord) {
        for (id, authority) in record.certificate_authorities.iter_mut() {
            if *id == self.certificate_authority_id {
                authority.signing_status = CA_SIGNING_IN_USE.to_string();
                authority.activated_at = Some(self.activated_at);
                authority.activated_by = Some(self.activated_by.clone());
                // The CA that signs now is not one to revert TO: a rollback
                // window belongs to the CA this promotion retires.
                authority.rollback_available_until = None;
            } else if authority.signing_status != CA_SIGNING_NOT_USED {
                authority.signing_status = CA_SIGNING_NOT_USED.to_string();
                authority.rollback_available_until =
                    self.activated_at.checked_add(CA_ROLLBACK_WINDOW);
            }
        }
    }
}

impl ClusterRecord {
    /// This cluster's certificate authority with `id`, if it holds one.
    pub fn certificate_authority(&self, id: &str) -> Option<&CertificateAuthorityRecord> {
        self.certificate_authorities.get(id)
    }

    /// This cluster's certificate authorities in the order
    /// `ListCertificateAuthorities` reports them: most recently created first, ties
    /// broken by id — [`CertificateAuthorityRecord::page_key`]'s order.
    ///
    /// The records are STORED in a map keyed by certificate-authority id, which is
    /// a lookup index ([`Self::certificate_authority`]) and not a reporting order:
    /// an id is derived from the cluster and a counter, so its byte order says
    /// nothing about when the certificate authority was created.
    pub fn certificate_authorities_newest_first(&self) -> Vec<&CertificateAuthorityRecord> {
        let mut ordered: Vec<&CertificateAuthorityRecord> =
            self.certificate_authorities.values().collect();
        ordered.sort_by_key(|authority| authority.page_key());
        ordered
    }

    /// "An object identifying the certificate authority that is currently signing
    /// certificates for the cluster" (`Certificate$active`).
    ///
    /// DERIVED from the certificate-authority records rather than stored beside
    /// them, so the cluster and `DescribeCertificateAuthority` can never disagree
    /// about which CA signs: an activation moves the signing status on the records
    /// and this follows. `None` while no record signs, which is what a cluster
    /// with no Amazon EKS control plane behind it reports.
    pub fn active_certificate_authority(&self) -> Option<ActiveCertificateAuthority> {
        self.certificate_authorities
            .values()
            .find(|authority| authority.signing_status == CA_SIGNING_IN_USE)
            .map(|authority| ActiveCertificateAuthority {
                id: Some(authority.id.clone()),
                activated_by: authority.activated_by.clone(),
            })
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

// ---------------------------------------------------------------------------
// The account's IAM roles and EC2 network: facts this service RESOLVES rather
// than holds.
//
// A request can name an IAM role Amazon EKS is asked to assume, and the subnets
// the network interfaces of a cluster or a node group are placed in. Neither is
// a resource of this service: a role lives in IAM and a subnet in EC2, so what
// a request names is resolved against THOSE services — reached the way any
// emulated service reaches another, through the in-process callback
// ([`smithy_server_runtime::OperationContext::callback`]), so this crate names
// no other service's crate — and a value that resolves to nothing is what a
// request is refused for.
//
// Every refusal here is grounded in what the other service ANSWERED. An
// emulator with no IAM or no EC2 mounted knows nothing about the role or the
// subnet, and nothing is what it refuses on: a refusal is never assumed from
// silence.
// ---------------------------------------------------------------------------

/// The refusal a request that asks Amazon EKS to pass a role that is not a role
/// of the caller's OWN account earns.
///
/// A fixed sentence tied to the rule — Amazon EKS authorizes the `iam:PassRole`
/// of the value a request names, and a value that does not name a role of this
/// account can never be passed, whatever its name or path — so it carries
/// nothing from the request. It is the sentence the recorded service answers,
/// including for a `roleArn` that is not an ARN at all: the pass-role is
/// evaluated against the value AS A ROLE OF SOME OTHER ACCOUNT, not parsed as a
/// parameter.
pub const CROSS_ACCOUNT_PASS_ROLE_REFUSAL: &str = "Cross-account pass role is not allowed.";

/// The refusal a `RegisterCluster` whose `connectorConfig.roleArn` is not a role
/// of this account earns.
///
/// A registration does NOT answer the pass-role refusal above, and this is the
/// one place in this service where that differs: the recorded service refuses an
/// unusable `connectorConfig.roleArn` with HTTP 400 `InvalidParameterException`
/// and this fixed sentence, while the same value in `CreateCluster$roleArn` is
/// refused 403 with [`CROSS_ACCOUNT_PASS_ROLE_REFUSAL`]. The QUESTION is the same
/// one [`pass_role_refusal`] answers — does the value name a role of this
/// account? — so one reading serves both; only how each operation reports it
/// differs, and each reports it as its own recording shows.
pub const CONNECTOR_ROLE_ARN_REFUSAL: &str = "The provided roleArn is invalid.";

/// The service principal the EKS connector role must let assume it: the cluster
/// is registered as a Systems Manager managed instance, so SSM is what assumes
/// the role `connectorConfig.roleArn` names (Amazon EKS User Guide, "Connecting
/// an external cluster"), which is also the principal the recorded refusal for
/// an unusable connector role names.
pub const CONNECTOR_SERVICE_PRINCIPAL: &str = "ssm.amazonaws.com";

/// The Amazon EKS service principal, for a role the model says Amazon EKS itself
/// assumes: "This role must have a trust policy that allows the EKS service
/// principal to assume it" (`CreateCapabilityRequest$roleArn`).
pub const EKS_SERVICE_PRINCIPAL: &str = "eks.amazonaws.com";

/// The EC2 error code for a subnet id no subnet of the account carries.
pub const EC2_INVALID_SUBNET_ID_NOT_FOUND: &str = "InvalidSubnetID.NotFound";

/// The partition every ARN this emulator mints carries, read off one it mints
/// rather than written out again.
pub fn emulator_partition() -> String {
    ::engine::emulator_arn("iam", "", "")
        .split(':')
        .nth(1)
        .unwrap_or_default()
        .to_string()
}

/// Why `role_arn` is not a role this caller may have Amazon EKS assume, or
/// `None` when it is one.
///
/// A role a request hands to Amazon EKS is passed to the service with the
/// caller's own `iam:PassRole`, so the question is whether the value names a
/// role THIS caller can pass: a well-formed IAM role ARN of the caller's own
/// partition and account. The `region` field of such an ARN is empty — IAM is
/// global, so an IAM ARN carries no Region.
///
/// EVERY failure of that question is one answer, a PERMISSION failure
/// (`AccessDeniedException`, 403), never a malformed parameter: the recorded
/// service refuses a `roleArn` of `not-an-arn` with
/// [`CROSS_ACCOUNT_PASS_ROLE_REFUSAL`] and not as a bad parameter, because the
/// authorization is evaluated against the value as the role of an account that
/// is not this one. The message says which: the fixed sentence for a value that
/// names no role of this account at all, and what else was wrong for a
/// well-formed IAM ARN of this account that names something other than a role,
/// or a role of another partition.
///
/// What is NOT answered here is whether such a role EXISTS and trusts the
/// principal that would assume it: that lives in IAM, and
/// [`service_principal_trust_violation`] is what asks it, for the roles whose
/// contract names the principal.
pub fn pass_role_refusal(role_arn: &str) -> Option<String> {
    let own_account = ::engine::emulator_account_id();
    let own_partition = emulator_partition();
    // `splitn(6, ':')` leaves the whole resource — colons included — as the last field.
    let fields: Vec<&str> = role_arn.splitn(6, ':').collect();
    let [literal, partition, vendor, region, account, resource] = fields.as_slice() else {
        return Some(CROSS_ACCOUNT_PASS_ROLE_REFUSAL.to_string());
    };
    // The IAM ARN form, `arn:<partition>:iam::<12-digit account>:<resource>` — IAM is a
    // global service, so the Region segment is empty. This is the same reading
    // [`is_iam_role_arn`] applies, minus the question of WHAT the resource names, which
    // is the next step rather than part of the form.
    let is_iam_arn = *literal == "arn"
        && !partition.is_empty()
        && *vendor == "iam"
        && region.is_empty()
        && account.len() == 12
        && account.bytes().all(|byte| byte.is_ascii_digit())
        && !resource.is_empty();
    // A value that is not an IAM ARN of this account names no role of it, which is the
    // same answer a role of another account gets.
    if !is_iam_arn || *account != own_account {
        return Some(CROSS_ACCOUNT_PASS_ROLE_REFUSAL.to_string());
    }
    // An IAM role's resource field is `role/<path><name>`; any other identity
    // (`user/`, `group/`, `instance-profile/`) is not something a service assumes.
    if resource.strip_prefix("role/").unwrap_or_default().is_empty() {
        return Some(format!("{role_arn} does not name an IAM role"));
    }
    if *partition != own_partition.as_str() {
        return Some(format!(
            "{role_arn} is in partition {partition}, not {own_partition}"
        ));
    }
    None
}

/// The role NAME `role_arn` names in the account this emulator serves, or `None`
/// when it is not an in-account IAM role ARN at all — the case
/// [`pass_role_refusal`] already reports, so this answers nothing about it.
///
/// IAM addresses a role by name, not by path, so the path a role sits under is
/// dropped: `role/service-role/Connector` is the role `Connector`.
fn iam_role_name(role_arn: &str) -> Option<&str> {
    if pass_role_refusal(role_arn).is_some() {
        return None;
    }
    let resource = role_arn.splitn(6, ':').nth(5)?;
    let name = resource.strip_prefix("role/")?.split('/').next_back()?;
    (!name.is_empty()).then_some(name)
}

/// An AWS SDK client over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated
/// service: it routes in process, so no network client is configured here and
/// this service names no other service's crate. `region` is the request's own
/// signing region.
macro_rules! callback_client {
    ($sdk:ident, $ctx:expr, $endpoint:expr, $region:expr) => {{
        let config = ::$sdk::Config::builder()
            .behavior_version(::$sdk::config::BehaviorVersion::latest())
            .http_client($ctx.callback().http_client())
            .credentials_provider($ctx.callback().aws_credentials_provider())
            .region(::$sdk::config::Region::new($region.to_string()))
            .endpoint_url($endpoint)
            // The transport is in-process: there is nothing to retry, time out
            // or watch for a stalled stream, and each of those would need a
            // sleep implementation this client is not given. The same goes for
            // caching the identity: the credentials are handed over directly.
            .retry_config(::$sdk::config::retry::RetryConfig::disabled())
            .timeout_config(::$sdk::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::$sdk::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                ::$sdk::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        ::$sdk::Client::from_conf(config)
    }};
}

/// The endpoint the in-process callback reaches `service` in `region` at, or
/// `None` when `region` is not a Region LABEL.
///
/// The Region a request is signed for is CALLER-SUPPLIED text, and here it ends
/// up in an endpoint URL's authority rather than in a response, so it is read as
/// a Region label (`service_common::arn::region_label`, the one place this tree
/// states that grammar) instead of being interpolated as given: a value carrying
/// a host or path delimiter would name an authority the caller chose rather than
/// the peer service this emulator asks. `None` means there is no peer endpoint to
/// ask, which the two callers below answer the way they answer any unreachable
/// peer — by grounding no refusal in it.
fn peer_endpoint(service: &str, region: &str) -> Option<String> {
    let Some(region) = ::service_common::arn::region_label(region) else {
        ::tracing::warn!(
            peer = %service,
            "the request names no Region label, so {service} cannot be asked"
        );
        return None;
    };
    Some(format!("https://{service}.{region}.amazonaws.com"))
}

/// The refusal a role whose trust policy does not admit `principal` earns, or
/// `None` when IAM holds the role and its trust policy lets `principal` assume
/// it — and also when this emulator has no IAM to ask, because a refusal has to
/// be grounded in what IAM answered rather than assumed from silence.
///
/// The trust document is read through the ONE evaluator every service that
/// resolves a caller-supplied role shares
/// (`service_common::iam_trust_policy::admits_service_principal`), so two
/// services asked about the same document cannot answer differently. A document
/// this emulator cannot read as a policy is not one Amazon EKS can act on, so it
/// does not admit the assume.
pub async fn service_principal_trust_violation(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
    principal: &str,
) -> Option<String> {
    // Not an in-account role at all: `pass_role_refusal` is what reports that,
    // and it has already run by the time a handler asks this.
    let role_name = iam_role_name(role_arn)?;
    let answer = callback_client!(aws_sdk_iam, ctx, peer_endpoint("iam", region)?, region)
    .get_role()
    .role_name(role_name)
    .send()
    .await;
    match answer {
        Ok(output) => {
            let trusted = output
                .role()
                .and_then(::aws_sdk_iam::types::Role::assume_role_policy_document)
                .is_some_and(|document| {
                    ::service_common::iam_trust_policy::admits_service_principal(
                        document, principal,
                    )
                    .unwrap_or(false)
                });
            (!trusted).then(|| trust_policy_refusal(role_arn, principal))
        }
        // IAM says this account holds no such role: the refusal is grounded.
        Err(error) if iam_role_is_absent(&error) => {
            Some(trust_policy_refusal(role_arn, principal))
        }
        // Any other answer is this emulator failing to ask, not IAM reporting a
        // bad role, so it refuses nothing — and it is worth a warning, because a
        // deployment with no IAM to ask is one where this gate does not run.
        Err(error) => {
            ::tracing::warn!(
                %role_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the role could not be verified against IAM"
            );
            None
        }
    }
}

/// The message a role that IAM does not hold, or whose trust policy does not let
/// `principal` assume it, is refused with: the two cases are one refusal,
/// because a caller cannot tell them apart without a role it may not be allowed
/// to read, and it names the role at fault.
fn trust_policy_refusal(role_arn: &str, principal: &str) -> String {
    let service = principal.split('.').next().unwrap_or(principal);
    format!("Nonexistent role or missing {service} service principal in trust policy: {role_arn}")
}

/// Whether an IAM `GetRole` failure is IAM reporting that the role does not
/// exist (`NoSuchEntity`), as opposed to the call not reaching IAM.
fn iam_role_is_absent(
    error: &::aws_sdk_iam::error::SdkError<
        ::aws_sdk_iam::operation::get_role::GetRoleError,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool {
    error
        .as_service_error()
        .is_some_and(::aws_sdk_iam::operation::get_role::GetRoleError::is_no_such_entity_exception)
}

/// The refusal a `subnetIds` list earns when one of the subnets it names does
/// not exist, or `None` when EC2 resolved every one of them — and also when this
/// emulator has no EC2 to ask, for the same reason
/// [`service_principal_trust_violation`] refuses nothing then.
///
/// Amazon EKS places the control plane's (or a node group's) network interfaces
/// in the subnets a request names, so it resolves them before it provisions
/// anything. The subnets of the account are EC2's, so EC2 is asked: it answers
/// `InvalidSubnetID.NotFound` for an id no subnet of the account carries, and
/// the message it gives — which names that id — is carried through to the
/// caller with the service, status, error code, request id and proxy of the call
/// it came from, the way Amazon EKS reports a failure of a call it made on the
/// caller's behalf.
pub async fn unresolved_subnet_refusal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[String],
) -> Option<String> {
    resolve_subnets(ctx, region, subnet_ids).await.refusal
}

/// What EC2 answered about the subnets a request names.
///
/// One `DescribeSubnets` answers both of the questions a create asks about them:
/// whether every id resolves, and — because Amazon EKS provisions the cluster's
/// own EC2 resources in the network those subnets sit in — which VPC that is.
#[derive(Debug, Clone, Default)]
pub struct ResolvedSubnets {
    /// Why the list does not resolve, in the wrapped form Amazon EKS reports a
    /// failure of a call it made on the caller's behalf in. `None` when EC2
    /// resolved every id, and also when this emulator has no EC2 to ask.
    pub refusal: Option<String>,
    /// The VPC every subnet EC2 reported belongs to. `None` when nothing
    /// answered, and when the reported subnets do not agree on one VPC.
    pub vpc_id: Option<String>,
}

/// Resolve `subnet_ids` against EC2: the one reading of a request's subnets, which
/// [`unresolved_subnet_refusal`] is the refusal half of.
pub async fn resolve_subnets(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[String],
) -> ResolvedSubnets {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    if subnet_ids.is_empty() {
        return ResolvedSubnets::default();
    }
    // No Region label, so no endpoint to ask EC2 at: the same answer an
    // unreachable EC2 gets, which grounds no refusal and names no VPC.
    let Some(endpoint) = peer_endpoint("ec2", region) else {
        return ResolvedSubnets::default();
    };
    let answer = callback_client!(aws_sdk_ec2, ctx, endpoint, region)
        .describe_subnets()
        .set_subnet_ids(Some(subnet_ids.to_vec()))
        .send()
        .await;
    // Every requested id resolved: EC2 reports the subnets rather than refusing.
    // A subnet belongs to exactly one VPC, so the VPC a cluster is created in is
    // the one every subnet it names agrees on; a list that spans two names no
    // single network to provision in, and none is reported.
    let error = match answer {
        Ok(output) => {
            let mut reported = output
                .subnets()
                .iter()
                .filter_map(::aws_sdk_ec2::types::Subnet::vpc_id);
            let first = reported.next().map(str::to_string);
            let vpc_id = first.filter(|vpc_id| reported.all(|other| other == vpc_id.as_str()));
            return ResolvedSubnets {
                refusal: None,
                vpc_id,
            };
        }
        Err(error) => error,
    };
    let code = error
        .as_service_error()
        .and_then(ProvideErrorMetadata::code)
        .unwrap_or_default()
        .to_string();
    if code != EC2_INVALID_SUBNET_ID_NOT_FOUND {
        ::tracing::warn!(
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "the requested subnets could not be resolved against EC2"
        );
        return ResolvedSubnets::default();
    }
    let status = error
        .raw_response()
        .map(|response| format!("Status Code: {}; ", response.status().as_u16()))
        .unwrap_or_default();
    let detail = error
        .as_service_error()
        .and_then(ProvideErrorMetadata::message)
        // EC2 names the offending id in its message; with no message to carry,
        // the ids the request named are what is reported instead, so the caller
        // is still told which list did not resolve.
        .map_or_else(
            || format!("One of the subnet IDs {subnet_ids:?} does not exist"),
            str::to_string,
        );
    // The wrapped form AWS reports an inner service's failure in names the service,
    // the status and the error code it answered, the id of the call that was made
    // and the proxy it went through — there is none, so that field is `null`. The
    // tail is what a caller (or its tooling) correlates this refusal with the call
    // behind it by, so it is part of the message rather than an addition to it.
    let request_id = new_peer_request_id();
    ResolvedSubnets {
        refusal: Some(format!(
            "{detail} (Service: AmazonEC2; {status}Error Code: {code}; \
             Request ID: {request_id}; Proxy: null)"
        )),
        vpc_id: None,
    }
}

// ---------------------------------------------------------------------------
// The cluster security group slice.
//
// Creating a cluster provisions one security group of its own in the VPC the
// cluster's subnets sit in: the group every node group, add-on and
// cluster-to-node rule is authorised against. It is EC2's resource, so EC2 is
// where it is created and deleted — over the same in-process callback every
// cross-service reading above goes through — and deleting the cluster releases
// it, because the group exists only for that cluster.
// ---------------------------------------------------------------------------

/// The name prefix Amazon EKS gives the security group it creates with a cluster:
/// `eks-cluster-sg-<cluster name>-<suffix>`.
pub const CLUSTER_SECURITY_GROUP_NAME_PREFIX: &str = "eks-cluster-sg-";

/// The description that group carries, which says what it is attached to.
pub const CLUSTER_SECURITY_GROUP_DESCRIPTION: &str =
    "EKS created security group applied to ENI that is attached to EKS Control \
     Plane master nodes, as well as any managed workloads.";

/// The tag Amazon EKS stamps on the EC2 resources it owns for a cluster, naming
/// that cluster. It sits in AWS's own `aws:` tag namespace, so no caller can set
/// it — which is what makes it the handle a client finds the group by.
pub const CLUSTER_NAME_TAG: &str = "aws:eks:cluster-name";

/// The prefix of the Kubernetes cluster-ownership tag,
/// `kubernetes.io/cluster/<cluster name>`: the tag the in-cluster AWS cloud
/// provider reads to decide which resources belong to the cluster.
pub const KUBERNETES_CLUSTER_TAG_PREFIX: &str = "kubernetes.io/cluster/";

/// That tag's value on a resource the cluster itself OWNS, as opposed to one it
/// only shares.
pub const KUBERNETES_CLUSTER_OWNED: &str = "owned";

/// The tag an EC2 resource's display name is read from.
const NAME_TAG: &str = "Name";

/// The number of decimal digits the numeric suffix of a cluster security group's
/// name carries.
const CLUSTER_SECURITY_GROUP_SUFFIX_DIGITS: u32 = 10;

/// The name of the security group a cluster owns.
///
/// The numeric suffix is derived from the cluster's own stable resource id
/// ([`derive_cluster_resource_id`]), so it is distinct per cluster, the same on
/// every read, and never borrowed from the request. `resource_id` is therefore
/// expected to be that 32-hex-character id; a value that is not — which nothing
/// in this crate produces — draws its suffix from a zero seed rather than
/// refusing a name, because a group name is not what a create is refused over.
pub fn cluster_security_group_name(cluster_name: &str, resource_id: &str) -> String {
    // The low bound is the first number of this width, so the suffix never
    // renders narrower than the width Amazon EKS publishes it at.
    let floor = 10_u64.pow(CLUSTER_SECURITY_GROUP_SUFFIX_DIGITS - 1);
    let span = 10_u64.pow(CLUSTER_SECURITY_GROUP_SUFFIX_DIGITS) - floor;
    // 15 hex digits is the widest slice of the id that cannot overflow a `u64`.
    let seed = resource_id
        .get(..15)
        .and_then(|head| u64::from_str_radix(head, 16).ok())
        .unwrap_or_default();
    let suffix = floor + seed % span;
    format!("{CLUSTER_SECURITY_GROUP_NAME_PREFIX}{cluster_name}-{suffix}")
}

/// Mint the security group a new cluster owns, in `vpc_id`, and report its id.
///
/// `None` when EC2 did not create it — including when this emulator has no EC2 to
/// create it in, which is the same silence [`resolve_subnets`] refuses nothing on:
/// the cluster is still created, it just owns no group.
pub async fn mint_cluster_security_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    cluster_name: &str,
    resource_id: &str,
    vpc_id: &str,
) -> Option<String> {
    let ec2 = callback_client!(aws_sdk_ec2, ctx, peer_endpoint("ec2", region)?, region);
    let group_name = cluster_security_group_name(cluster_name, resource_id);
    let created = ec2
        .create_security_group()
        .group_name(&group_name)
        .description(CLUSTER_SECURITY_GROUP_DESCRIPTION)
        .vpc_id(vpc_id)
        .send()
        .await;
    let group_id = match created {
        Ok(output) => output.group_id,
        Err(error) => {
            ::tracing::warn!(
                %cluster_name,
                %vpc_id,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the cluster security group could not be created in EC2"
            );
            None
        }
    }?;
    // The group is tagged as the cluster's: the `aws:` key naming the cluster that
    // owns it, the Kubernetes ownership tag the in-cluster cloud provider reads,
    // and the display name the group was created under.
    let tags: Vec<::aws_sdk_ec2::types::Tag> = [
        (CLUSTER_NAME_TAG.to_string(), cluster_name.to_string()),
        (
            format!("{KUBERNETES_CLUSTER_TAG_PREFIX}{cluster_name}"),
            KUBERNETES_CLUSTER_OWNED.to_string(),
        ),
        (NAME_TAG.to_string(), group_name),
    ]
    .into_iter()
    .map(|(key, value)| {
        ::aws_sdk_ec2::types::Tag::builder()
            .key(key)
            .value(value)
            .build()
    })
    .collect();
    let tagged = ec2
        .create_tags()
        .resources(group_id.clone())
        .set_tags(Some(tags))
        .send()
        .await;
    if let Err(error) = tagged {
        // The group exists and is the cluster's, so it is reported and released
        // with the cluster either way; what a failure here costs is the tags a
        // client finds it by, which is worth a warning.
        ::tracing::warn!(
            %cluster_name,
            %group_id,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "the cluster security group could not be tagged in EC2"
        );
    }
    Some(group_id)
}

/// Release the security group a deleted cluster owned.
///
/// A group this emulator cannot delete is reported rather than refused: the
/// cluster is already gone from this service's store, and `DeleteCluster` has no
/// failure to answer the caller with for a resource of another service.
pub async fn release_cluster_security_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    group_id: &str,
) {
    // No Region label, so no endpoint to reach EC2 at: the same silence an
    // unreachable EC2 leaves, which a deleted cluster is not refused over.
    let Some(endpoint) = peer_endpoint("ec2", region) else {
        return;
    };
    let released = callback_client!(aws_sdk_ec2, ctx, endpoint, region)
        .delete_security_group()
        .group_id(group_id)
        .send()
        .await;
    if let Err(error) = released {
        ::tracing::warn!(
            %group_id,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "the cluster security group could not be deleted from EC2"
        );
    }
}

// ---------------------------------------------------------------------------
// The EKS Anywhere subscription slice.
//
// A subscription is NOT a resource of a cluster: it is an account-level support
// contract ("a contract agreement for the length of the term specified in the
// request", `CreateEksAnywhereSubscription`), so everything about it — its
// spans, its record, its projection and the rules its term and license count are
// held to — is collected here and the records live in a map of their own on the
// store.
// ---------------------------------------------------------------------------

/// The `EksAnywhereSubscriptionStatus` members this slice uses. The model's enum
/// (`smithy_query query="shape" shape="EksAnywhereSubscriptionStatus"`) is
/// `CREATING | ACTIVE | UPDATING | EXPIRING | EXPIRED | DELETING`;
/// `EksAnywhereSubscription$status` is a plain string member, so the status is
/// stored as the wire token. `UPDATING`, `EXPIRING` and `DELETING` belong to the
/// operations that update and delete a subscription, each with its own gap
/// issue.
pub const SUBSCRIPTION_STATUS_CREATING: &str = "CREATING";
/// See [`SUBSCRIPTION_STATUS_CREATING`].
pub const SUBSCRIPTION_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`SUBSCRIPTION_STATUS_CREATING`] — the status a subscription reports once
/// its term has run out, which the clock owes it at its expiration date.
pub const SUBSCRIPTION_STATUS_EXPIRED: &str = "EXPIRED";

/// The only `EksAnywhereSubscriptionTermUnit` the API defines: "The term unit of
/// the subscription. Valid value is `MONTHS`"
/// (`EksAnywhereSubscriptionTerm$unit`).
pub const SUBSCRIPTION_TERM_UNIT_MONTHS: &str = "MONTHS";

/// The term durations a subscription may be created for: "Valid values are 12
/// and 36, indicating a 12 month or 36 month subscription"
/// (`EksAnywhereSubscriptionTerm$duration`).
pub const SUBSCRIPTION_TERM_DURATIONS: &[i32] = &[12, 36];

/// The license counts a subscription may be created with: "The number of
/// licenses to purchase with the subscription. Valid values are between 1 and
/// 100" (`CreateEksAnywhereSubscriptionRequest$licenseQuantity`). The member
/// carries a `@default` of 0 and no modelled range, so a count outside the
/// published one reaches the handler.
pub const MIN_SUBSCRIPTION_LICENSE_QUANTITY: i32 = 1;
/// See [`MIN_SUBSCRIPTION_LICENSE_QUANTITY`].
pub const MAX_SUBSCRIPTION_LICENSE_QUANTITY: i32 = 100;

/// The only `EksAnywhereSubscriptionLicenseType` the API defines: "The license
/// type for all licenses in the subscription. Valid value is CLUSTER"
/// (`CreateEksAnywhereSubscriptionRequest$licenseType`). The model's enum member
/// is `Cluster` (`smithy_query query="shape"
/// shape="EksAnywhereSubscriptionLicenseType"`), which is the token this slice
/// stores and reports.
pub const SUBSCRIPTION_LICENSE_TYPE_CLUSTER: &str = "Cluster";

/// How long a new subscription reports `CREATING` before the clock owes it
/// `ACTIVE`.
///
/// `CreateEksAnywhereSubscription` publishes no duration for the provisioning it
/// starts (the licenses "are provisioned in Amazon Web Services License
/// Manager"), so this span is CHOSEN rather than cited, and matches
/// [`CLUSTER_ACTIVATION_DELAY`]: long enough that a caller which describes the
/// subscription straight after creating it still observes `CREATING`, short
/// enough to be worth waiting for. The due instant is stored on the record;
/// nothing sleeps for it.
pub const SUBSCRIPTION_ACTIVATION_DELAY: Duration = CLUSTER_ACTIVATION_DELAY;

/// The number of days in the month a subscription term is counted in.
///
/// A term is a count of MONTHS ([`SUBSCRIPTION_TERM_UNIT_MONTHS`]) and the
/// expiration date is "the Unix timestamp in seconds for when the subscription
/// will expire or auto renew" (`EksAnywhereSubscription$expirationDate`), so the
/// term has to be turned into a duration. Calendar months differ in length and
/// the API publishes no rule for which ones a term covers, so one month is taken
/// as 30 days — the span this emulator states once here rather than in each
/// caller.
pub const SUBSCRIPTION_MONTH: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// The number of subscriptions this store holds, across every Region, before
/// `CreateEksAnywhereSubscription` answers `ResourceLimitExceededException`.
///
/// AWS publishes no quota for EKS Anywhere subscriptions, so the ceiling is
/// CHOSEN: it bounds the store, so repeated creates cannot grow it without
/// limit, and it is far above the handful of support contracts an account holds.
pub const MAX_STORED_SUBSCRIPTIONS: usize = 100;

/// One EKS Anywhere subscription, as this service stores it.
///
/// Everything an `EksAnywhereSubscription` reports is held here — including the
/// server-generated parts (`id`, `arn`, `createdAt`, the effective and
/// expiration dates and the licenses) — so a later read returns exactly what the
/// create wrote rather than recomputing anything per request.
#[derive(Debug, Clone)]
pub struct EksAnywhereSubscriptionRecord {
    pub id: String,
    pub name: String,
    pub arn: String,
    pub created_at: SystemTime,
    /// "The Unix timestamp in seconds for when the subscription is effective"
    /// (`EksAnywhereSubscription$effectiveDate`): a subscription is a contract
    /// that starts when it is bought, so this is the instant of the create.
    pub effective_date: SystemTime,
    /// "The Unix timestamp in seconds for when the subscription will expire or
    /// auto renew": the effective date plus the term the request asked for.
    pub expiration_date: SystemTime,
    pub license_quantity: i32,
    pub license_type: String,
    pub term_duration: i32,
    pub term_unit: String,
    pub status: String,
    /// When set, the instant at which the clock owes this subscription the move
    /// from `CREATING` to `ACTIVE`. Cleared once the move is materialised.
    pub active_at: Option<SystemTime>,
    pub auto_renew: bool,
    pub tags: BTreeMap<String, String>,
    pub client_request_token: Option<String>,
}

impl EksAnywhereSubscriptionRecord {
    /// Materialise the status moves the clock already owes at `now`: the move out
    /// of `CREATING` once the licenses are provisioned, and the move to `EXPIRED`
    /// once the term has run out on a subscription that does not auto renew.
    ///
    /// Both were due at their stored instant whether or not anybody called, so
    /// applying them on read does not change what a later caller sees. A
    /// subscription that DOES auto renew is renewed at the same instant — "a
    /// boolean indicating whether or not a subscription will auto renew when it
    /// expires" (`EksAnywhereSubscription$autoRenew`) — so its term rolls
    /// forward instead of ending.
    pub fn materialize_due_transitions(&mut self, now: SystemTime) {
        if self.active_at.is_some_and(|due| now >= due) {
            self.status = SUBSCRIPTION_STATUS_ACTIVE.to_string();
            self.active_at = None;
        }
        if now < self.expiration_date {
            return;
        }
        let term = self.term();
        // A term of no length cannot be rolled forward — it would name the same
        // instant again — so such a subscription is reported as expired whatever its
        // auto-renew says. The term of every record this slice writes is one of the
        // published durations ([`SUBSCRIPTION_TERM_DURATIONS`]), so this is the
        // guard rather than the case.
        if !self.auto_renew || term.is_zero() {
            self.status = SUBSCRIPTION_STATUS_EXPIRED.to_string();
            return;
        }
        // The renewal the clock owes is computed rather than looped: however many
        // whole terms have passed since the last expiration, the next one is the
        // first instant after `now`. A clock so far past the representable range
        // that it cannot be placed leaves the date where it is rather than wrapping.
        let Ok(overdue) = now.duration_since(self.expiration_date) else {
            return;
        };
        let terms_passed = overdue.as_secs() / term.as_secs() + 1;
        let skipped = u32::try_from(terms_passed)
            .ok()
            .and_then(|terms| term.checked_mul(terms));
        if let Some(renewed) = skipped.and_then(|skipped| self.expiration_date.checked_add(skipped))
        {
            self.expiration_date = renewed;
        }
    }

    /// The duration this subscription's term covers.
    ///
    /// Zero for a term whose duration is not a positive count of months, which no
    /// record this slice writes carries: `CreateEksAnywhereSubscription` refuses
    /// every duration outside [`SUBSCRIPTION_TERM_DURATIONS`] before it writes one.
    fn term(&self) -> Duration {
        let months = u32::try_from(self.term_duration.max(0)).unwrap_or(0);
        SUBSCRIPTION_MONTH
            .checked_mul(months)
            .unwrap_or(Duration::ZERO)
    }

    /// This record as the modelled `EksAnywhereSubscription` shape.
    pub fn to_subscription(&self) -> EksAnywhereSubscription {
        EksAnywhereSubscription {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            effective_date: Some(::aws_smithy_types::DateTime::from(self.effective_date)),
            expiration_date: Some(::aws_smithy_types::DateTime::from(self.expiration_date)),
            license_quantity: self.license_quantity,
            license_type: Some(self.license_type.clone()),
            term: Some(EksAnywhereSubscriptionTerm {
                duration: self.term_duration,
                unit: Some(self.term_unit.clone()),
            }),
            status: Some(self.status.clone()),
            auto_renew: self.auto_renew,
            // `licenseArns` names the Amazon Web Services License Manager ARN
            // the subscription's licenses live under and `licenses` carries "all
            // of the claims in the license token necessary to validate the
            // license for extended support". This emulator provisions nothing in
            // License Manager and signs no license token, so it reports neither
            // rather than naming a license nothing issued. Both are optional in
            // the model.
            license_arns: None,
            licenses: None,
            // `TagMap` is bounded 1..50, so a subscription with no tags reports
            // no map rather than an empty one.
            tags: (!self.tags.is_empty()).then(|| {
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            }),
        }
    }
}

/// The ARN of the subscription `id` in `region`: "The Amazon Resource Name (ARN)
/// for the subscription" (`EksAnywhereSubscription$arn`), in the same
/// `eks`-namespace form every other ARN this service mints takes.
pub fn eks_anywhere_subscription_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("eks", region, &format!("eksanywheresubscription/{id}"))
}

/// Why `term` is not a term a subscription may be created for, or `None` when it
/// is one.
///
/// The rule is the one the model states for the member: "Valid values are MONTHS
/// for term unit and 12 or 36 for term duration"
/// (`EksAnywhereSubscriptionTerm`). The `unit` member targets an enum, so a
/// value outside it is already refused before a handler runs, and an ABSENT unit
/// is not: a term that names no unit names no length. The duration carries no
/// modelled range at all.
pub fn subscription_term_refusal(term: &EksAnywhereSubscriptionTerm) -> Option<String> {
    if !SUBSCRIPTION_TERM_DURATIONS.contains(&term.duration) {
        return Some(format!(
            "term duration not one of: {}",
            SUBSCRIPTION_TERM_DURATIONS
                .iter()
                .map(i32::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    match term.unit.as_deref() {
        Some(unit) if unit == SUBSCRIPTION_TERM_UNIT_MONTHS => None,
        Some(unit) => Some(format!(
            "term unit not one of: {SUBSCRIPTION_TERM_UNIT_MONTHS}, got {}",
            elided_value(unit, MAX_ECHOED_VALUE)
        )),
        None => Some(format!(
            "term unit must be {SUBSCRIPTION_TERM_UNIT_MONTHS}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pass-role rule over the ARN forms it generalises over: which account and
    /// partition the role belongs to, what the IAM ARN names, the role's path, and the
    /// forms that are not IAM ARNs at all. The rows differ along exactly those axes.
    ///
    /// Every failure is ONE answer — the role may not be passed — and the message says
    /// which: the fixed cross-account sentence for a value that names no role of this
    /// account (the recorded service answers it even for a value that is not an ARN), or
    /// what else was wrong for an in-account IAM ARN.
    #[test]
    fn the_pass_role_rule_refuses_every_value_that_is_not_a_role_of_this_account() {
        let own = ::engine::emulator_account_id();
        /// What the row's message must be: the fixed sentence, a message naming the
        /// value, or nothing at all (the role may be passed).
        #[derive(Debug, PartialEq, Eq)]
        enum Expected {
            Passed,
            CrossAccount,
            Named,
        }
        let rows: Vec<(&str, String, Expected)> = vec![
            (
                "a role of this account",
                format!("arn:aws:iam::{own}:role/eksClusterRole"),
                Expected::Passed,
            ),
            (
                "a role of this account under a path",
                format!("arn:aws:iam::{own}:role/service-role/eksClusterRole"),
                Expected::Passed,
            ),
            (
                "a role of another account",
                "arn:aws:iam::111122223333:role/eksClusterRole".to_string(),
                Expected::CrossAccount,
            ),
            (
                "a role of another account under a path",
                "arn:aws:iam::210987654321:role/service-role/anything".to_string(),
                Expected::CrossAccount,
            ),
            (
                "a role of another partition",
                format!("arn:aws-cn:iam::{own}:role/eksClusterRole"),
                Expected::Named,
            ),
            (
                "an IAM user of this account, which no service assumes",
                format!("arn:aws:iam::{own}:user/operator"),
                Expected::Named,
            ),
            (
                "an IAM ARN of this account naming no role at all",
                format!("arn:aws:iam::{own}:role/"),
                Expected::Named,
            ),
            (
                "another vendor's ARN",
                format!("arn:aws:eks:us-west-2:{own}:cluster/other"),
                Expected::CrossAccount,
            ),
            (
                "an IAM ARN carrying a Region, which IAM ARNs do not",
                format!("arn:aws:iam:us-west-2:{own}:role/eksClusterRole"),
                Expected::CrossAccount,
            ),
            (
                "an account that is not a 12-digit account id",
                "arn:aws:iam::12345:role/short-account".to_string(),
                Expected::CrossAccount,
            ),
            (
                "an ARN that stops before its resource",
                format!("arn:aws:iam::{own}"),
                Expected::CrossAccount,
            ),
            (
                "a value that is not an ARN at all",
                "not-an-arn".to_string(),
                Expected::CrossAccount,
            ),
            ("the empty string", String::new(), Expected::CrossAccount),
        ];

        for (case, role_arn, expected) in rows {
            let refusal = pass_role_refusal(&role_arn);
            let actual = match refusal.as_deref() {
                None => Expected::Passed,
                Some(CROSS_ACCOUNT_PASS_ROLE_REFUSAL) => Expected::CrossAccount,
                Some(_) => Expected::Named,
            };
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {refusal:?} for {role_arn}"
            );
            if actual == Expected::Named {
                let message = refusal.clone().unwrap_or_default();
                assert!(
                    message.contains(&role_arn),
                    "case {case:?}: a refusal that is not the fixed sentence must name the \
                     value at fault, got {message:?}"
                );
            }
            // A well-formed IAM ROLE ARN of this account is always passable: the two
            // readings of that form in this crate cannot disagree.
            if is_iam_role_arn(&role_arn) && role_arn.contains(&format!(":{own}:")) {
                assert_eq!(
                    pass_role_refusal(&role_arn).is_none(),
                    emulator_partition() == role_arn.split(':').nth(1).unwrap_or_default(),
                    "case {case:?}: {role_arn} is an IAM role ARN of this account, so only \
                     its partition may refuse it"
                );
            }
        }
    }

    /// IAM addresses a role by NAME, so the path is dropped — and a value that is not
    /// an in-account role ARN names no role to ask IAM about.
    #[test]
    fn the_iam_role_name_is_the_last_segment_of_the_resource() {
        let own = ::engine::emulator_account_id();
        let rows: Vec<(&str, String, Option<&str>)> = vec![
            (
                "the default path",
                format!("arn:aws:iam::{own}:role/Connector"),
                Some("Connector"),
            ),
            (
                "a role under a path",
                format!("arn:aws:iam::{own}:role/service-role/Connector"),
                Some("Connector"),
            ),
            (
                "a role under a nested path",
                format!("arn:aws:iam::{own}:role/team/dept/Connector"),
                Some("Connector"),
            ),
            (
                "a role of another account",
                "arn:aws:iam::111122223333:role/Connector".to_string(),
                None,
            ),
            ("not an ARN at all", "Connector".to_string(), None),
        ];
        for (case, role_arn, expected) in rows {
            let name = iam_role_name(&role_arn);
            assert_eq!(
                name, expected,
                "case {case:?}: expected {expected:?}, got {name:?} for {role_arn}"
            );
        }
    }

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
