// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-eks-awswesleyfrontend service.
//!
//! This holds the cluster lifecycle slice: `CreateCluster` and `RegisterCluster`
//! write a [`ClusterRecord`] here, and `DescribeCluster`, `ListClusters`,
//! `TagResource`, `ListTagsForResource` and `DeleteCluster` all compute their
//! responses from it. Nothing in `src/ops/` keeps a second copy of a cluster.
//!
//! One record covers both kinds of cluster this API knows: an Amazon EKS cluster
//! `CreateCluster` provisions a control plane for, and an external Kubernetes
//! cluster `RegisterCluster` connects. The members only a provisioned control
//! plane has (`endpoint`, `roleArn`, `resourcesVpcConfig`, `version`,
//! `platformVersion`) are optional on the record, because a connected cluster
//! reports none of them — it reports [`ClusterRecord::connector_config`] instead.
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
    AccessConfigResponse, AccessEntry, AccessScope, AssociatedAccessPolicy, Cancellation,
    Certificate, Cluster, ClusterVersionInformation, ComputeConfigResponse,
    ConnectorConfigResponse,
    ControlPlaneScalingConfig, EncryptionConfig, Identity, KubeApiServerConfigResponse,
    KubeControllerManagerConfigResponse, KubeSchedulerConfigResponse,
    KubernetesNetworkConfigResponse, Logging, OidcIdentityProviderConfig, OutpostConfigResponse,
    PodIdentityAssociation, PodIdentityAssociationSummary, RemoteNetworkConfigResponse,
    StorageConfigResponse, Update, UpdateParam, UpgradePolicyResponse, VpcConfigResponse,
    ZonalShiftConfigResponse,
};

/// The `ClusterStatus` members this slice uses. The model's enum
/// (`smithy_query query="shape" shape="ClusterStatus"`) is
/// `CREATING | ACTIVE | DELETING | FAILED | UPDATING | PENDING`; `Cluster$status`
/// is a plain string member, so the status is stored as the wire token.
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
pub const DEFAULT_KUBERNETES_VERSION: &str = "1.31";

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
/// natural default.
pub const DEFAULT_LIST_PAGE_SIZE: i32 = 100;

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
/// of Amazon EKS access policies lists the cluster- and namespace-scoped policies
/// below — the ones whose permissions mirror the Kubernetes `cluster-admin`,
/// `admin`, `edit` and `view` cluster roles that `AccessPolicy`'s own
/// documentation points at. Sorted, so a page token can name the entry to resume
/// after.
pub const ACCESS_POLICY_NAMES: &[&str] = &[
    "AmazonEKSAdminPolicy",
    "AmazonEKSAdminViewPolicy",
    "AmazonEKSClusterAdminPolicy",
    "AmazonEKSEditPolicy",
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
    /// enabled, and `AssociateIdentityProviderConfig` mints at most one update per
    /// configuration it associates — so this map holds at most
    /// [`MAX_IDENTITY_PROVIDER_CONFIGS_PER_CLUSTER`] + 1 entries per cluster and
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
    /// read does not change what a later caller sees. Three are owed here — a new
    /// cluster's `CREATING` -> `ACTIVE` move, the completion of an asynchronous
    /// update, which flips the update to `Successful`, applies the change it
    /// carries and hands the cluster back to `ACTIVE`, and the
    /// `CREATING` -> `ACTIVE` move of every identity provider configuration
    /// hanging off this record.
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
            // the caller, so it reports no owner.
            owner_arn: None,
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
            owner_arn: None,
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
            // Left unset, each because this emulator holds nothing to compute it
            // from rather than because the member is uninteresting:
            // `kubernetesPatchVersion` is the patch level of the control-plane
            // build actually running a version, and this emulator runs no
            // Kubernetes control plane; `controlPlaneScalingTiers` and
            // `controlPlaneComponentConfig` are the provisioned control-plane
            // tier and component-default catalogs, which belong to the
            // provisioned-control-plane slice (no operation here publishes them).
            kubernetes_patch_version: None,
            control_plane_scaling_tiers: None,
            control_plane_component_config: None,
        }
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
/// months, and extended support runs for a further 12 months — but every row
/// below is the PUBLISHED date rather than one computed from that policy.
///
/// Rows are in ascending Kubernetes version order, which is the order
/// `DescribeClusterVersions` reports and pages them in. The calendar is a fixed
/// table, so it also bounds this part of the store.
const VERSION_CALENDAR: &[(&str, &str, &str, &str)] = &[
    ("1.26", "2023-04-11", "2024-06-11", "2025-06-11"),
    ("1.27", "2023-05-24", "2024-07-24", "2025-07-24"),
    ("1.28", "2023-09-26", "2024-11-26", "2025-11-26"),
    ("1.29", "2024-01-23", "2025-03-23", "2026-03-23"),
    ("1.30", "2024-05-23", "2025-07-23", "2026-07-23"),
    ("1.31", "2024-09-26", "2025-11-26", "2026-11-26"),
    ("1.32", "2025-01-23", "2026-03-23", "2027-03-23"),
    ("1.33", "2025-05-28", "2026-07-28", "2027-07-28"),
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
fn seed_cluster_versions() -> Vec<ClusterVersionRecord> {
    VERSION_CALENDAR
        .iter()
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
    /// data. A `Vec` because the calendar has a publication ORDER (ascending
    /// Kubernetes version) that the listing and its page tokens follow.
    pub cluster_versions: Vec<ClusterVersionRecord>,
    /// How many update ids this store has minted. Mixed into the next one (see
    /// [`new_update_id`]) so two updates minted in the same nanosecond cannot
    /// collide.
    pub minted_updates: u64,
    // The nodegroup map the next slice needs is deliberately NOT declared yet: a
    // field no operation reads is dead weight that every `Debug`/`Default` and
    // every reviewer has to account for. It is added by the slice that first
    // writes it (`CreateNodegroup`, still 501 with its own gap issue). An
    // asynchronous update is not here either — it lives on the cluster it
    // updates ([`ClusterRecord::updates`]).
}

impl Default for ComAmazonawsEksAwswesleyfrontendState {
    /// A fresh store holds no clusters and the whole published version calendar:
    /// `DescribeClusterVersions` answers before anything has been created.
    fn default() -> Self {
        Self {
            clusters: BTreeMap::new(),
            arn_index: BTreeMap::new(),
            pod_identity_associations: BTreeMap::new(),
            minted_associations: 0,
            cluster_versions: seed_cluster_versions(),
            minted_updates: 0,
        }
    }
}

impl ComAmazonawsEksAwswesleyfrontendState {
    /// The calendar rows that are released at `now`, in the calendar's own
    /// ascending-version order.
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

/// The cluster's `certificateAuthority.data`.
///
/// A real cluster reports the base64 of the PEM bundle its API server is fronted
/// by. This emulator runs no certificate authority, so rather than synthesise
/// bytes that look like a certificate it stores an explicitly labelled
/// placeholder: a caller that base64-decodes it reads, in plain text, that no CA
/// exists here. It is written once at create time and read back unchanged.
pub fn derive_certificate_authority_data(resource_id: &str) -> String {
    let placeholder = format!(
        "-----BEGIN CERTIFICATE-----\n\
         EMULATOR PLACEHOLDER: this emulator runs no certificate authority, so this\n\
         is not a usable certificate for cluster id {resource_id}.\n\
         -----END CERTIFICATE-----\n"
    );
    ::aws_smithy_types::base64::encode(placeholder)
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
            seen.push(row.cluster_version.as_str());
        }
        assert!(
            seen.windows(2).all(|pair| pair[0] < pair[1]),
            "case ordering: calendar rows must ascend by version, got {seen:?}"
        );
    }

    /// Exactly one row is the default version, and it is the version
    /// `CreateCluster` creates when the caller names none — so the catalog and the
    /// cluster slice cannot disagree about the default.
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
