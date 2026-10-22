// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-eks-awswesleyfrontend service.
//!
//! This holds the cluster lifecycle slice: `CreateCluster` writes a
//! [`ClusterRecord`] here, and `DescribeCluster`, `ListClusters`, `TagResource`,
//! `ListTagsForResource` and `DeleteCluster` all compute their responses from it.
//! Nothing in `src/ops/` keeps a second copy of a cluster.
//!
//! An asynchronous cluster update lives ON the cluster it updates
//! ([`ClusterRecord::updates`]), so `AssociateEncryptionConfig` writes one there
//! and `DescribeUpdate` reads it back; deleting the cluster drops its updates
//! with it rather than leaving them behind in a second map.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    AccessConfigResponse, Certificate, Cluster, ComputeConfigResponse, ControlPlaneScalingConfig,
    EncryptionConfig, Identity, KubeApiServerConfigResponse, KubeControllerManagerConfigResponse,
    KubeSchedulerConfigResponse, KubernetesNetworkConfigResponse, Logging, OutpostConfigResponse,
    RemoteNetworkConfigResponse, StorageConfigResponse, Update, UpdateParam, UpgradePolicyResponse,
    VpcConfigResponse, ZonalShiftConfigResponse,
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

/// The Kubernetes version a cluster gets when `CreateCluster` is called without
/// `version`. The model documents the member as "The desired Kubernetes version
/// for your cluster. If you don't specify a value here, the default version
/// available in Amazon EKS is used" without naming it, so this emulator pins one
/// recent minor version instead of inventing a value per request.
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

/// The number of clusters `ListClusters` returns when the caller gives no
/// `maxResults`. The model bounds `maxResults` at 1..100, so the full page is the
/// natural default.
pub const DEFAULT_LIST_PAGE_SIZE: i32 = 100;

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
    pub version: String,
    pub platform_version: String,
    pub endpoint: String,
    pub role_arn: String,
    pub resources_vpc_config: VpcConfigResponse,
    pub kubernetes_network_config: Option<KubernetesNetworkConfigResponse>,
    pub logging: Option<Logging>,
    pub identity: Option<Identity>,
    pub certificate_authority: Option<Certificate>,
    pub status: String,
    /// When set, the instant at which the clock owes this cluster the move from
    /// `CREATING` to `ACTIVE`. Cleared once the move is materialised.
    pub active_at: Option<SystemTime>,
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
    /// The asynchronous updates started against this cluster, by update id.
    ///
    /// They live on the cluster because that is what scopes them: `DescribeUpdate`
    /// and `ListUpdates` both name a cluster, and deleting the cluster must drop
    /// its updates. A `BTreeMap` so a listing walks a stable, sorted order. Each
    /// operation that mints an update refuses to start a second one while one is
    /// in flight, and `AssociateEncryptionConfig` — the only writer in this slice
    /// — is refused once encryption is enabled, so this map holds at most one
    /// entry per cluster and [`MAX_STORED_CLUSTERS`] bounds it.
    pub updates: BTreeMap<String, UpdateRecord>,
}

/// One asynchronous update of a cluster, as this service stores it.
///
/// Everything the modelled `Update` reports is held here, so `DescribeUpdate`
/// returns what the call that started the update wrote rather than recomputing
/// it. The two extra fields are not reported: `succeeds_at` is the instant the
/// clock owes this update its `Successful` status, and
/// `pending_encryption_config` is the change applied to the cluster at that same
/// instant.
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
}

impl UpdateRecord {
    /// This record as the modelled `Update` shape.
    pub fn to_update(&self) -> Update {
        Update {
            id: Some(self.id.clone()),
            status: Some(self.status.clone()),
            r#type: Some(self.r#type.clone()),
            params: Some(self.params.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            // `errors` carries "any errors associated with a `Failed` update" and
            // `cancellation` "is present only if any cancellation is attempted",
            // so neither is reported for an update that is running or succeeded.
            // The slices that can fail or cancel an update (`CancelUpdate` and
            // friends) fill them in; each is still 501 with its own gap issue.
            errors: None,
            cancellation: None,
        }
    }
}

impl ClusterRecord {
    /// Materialise the status moves the clock already owes at `now`.
    ///
    /// This is the only write a reporting operation may make: each transition was
    /// due at its stored instant whether or not anybody called, so applying it on
    /// read does not change what a later caller sees. Two are owed here — a new
    /// cluster's `CREATING` -> `ACTIVE` move, and the completion of an
    /// asynchronous update, which flips the update to `Successful`, applies the
    /// change it carries and hands the cluster back to `ACTIVE`.
    pub fn materialize_due_transitions(&mut self, now: SystemTime) {
        if self.active_at.is_some_and(|due| now >= due) {
            self.status = STATUS_ACTIVE.to_string();
            self.active_at = None;
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

    /// The update this cluster is still running, if any.
    ///
    /// A cluster runs at most one update at a time, so this is what a mutating
    /// operation checks before starting another.
    pub fn in_flight_update(&self) -> Option<&UpdateRecord> {
        self.updates
            .values()
            .find(|update| update.status == UPDATE_IN_PROGRESS)
    }

    /// The update this cluster already recorded for `client_request_token`, if
    /// any — what makes a retry of the call that started it idempotent.
    pub fn update_for_token(&self, token: &str) -> Option<&UpdateRecord> {
        self.updates
            .values()
            .find(|update| update.client_request_token.as_deref() == Some(token))
    }

    /// This record as the modelled `Cluster` shape.
    pub fn to_cluster(&self) -> Cluster {
        Cluster {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            version: Some(self.version.clone()),
            endpoint: Some(self.endpoint.clone()),
            role_arn: Some(self.role_arn.clone()),
            resources_vpc_config: Some(self.resources_vpc_config.clone()),
            kubernetes_network_config: self.kubernetes_network_config.clone(),
            logging: self.logging.clone(),
            identity: self.identity.clone(),
            status: Some(self.status.clone()),
            certificate_authority: self.certificate_authority.clone(),
            client_request_token: self.client_request_token.clone(),
            platform_version: Some(self.platform_version.clone()),
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
            // Left unset by this slice, each with its own gap issue: `id` is the
            // id of a local cluster on an Outpost, `health` is filled in by
            // cluster insights, and `connector_config` only by `RegisterCluster`.
            id: None,
            health: None,
            connector_config: None,
        }
    }
}

/// Per-service state for com-amazonaws-eks-awswesleyfrontend. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
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

impl ComAmazonawsEksAwswesleyfrontendState {
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
    /// each region's names; this only drops the other regions.
    pub fn cluster_names_in<'a>(&'a self, region: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.clusters
            .keys()
            .filter(move |(key_region, _)| key_region == region)
            .map(|(_, name)| name.as_str())
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

/// The 32-hex-character resource identifier EKS gives a cluster, which appears in
/// the cluster's API server endpoint host and in its OIDC issuer URL.
///
/// Derived from the cluster ARN — which already carries the account, region and
/// name — so it is distinct per cluster and stable for the record's whole life.
/// It is computed once, when the cluster is created, and stored on the record;
/// no read recomputes it.
pub fn derive_cluster_resource_id(arn: &str) -> String {
    let digest = Sha256::digest(arn.as_bytes());
    hex::encode_upper(&digest[..16])
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

/// How many bytes of the name's own digest a page token carries, so a token this
/// service did not mint is refused instead of being resumed from. Four bytes is
/// the usual width for a checksum of this kind; it is a provenance check, not a
/// secret (the name travels in the token, as AWS's own opaque-but-unsigned
/// pagination tokens do).
pub const PAGE_TOKEN_TAG_LEN: usize = 4;

/// The listing a page token belongs to, mixed into its provenance tag so a token
/// one listing minted is not resumed from by another.
///
/// `ListClusters` pages the cluster names and is the listing whose tokens are
/// computed with the EMPTY scope, which is what [`encode_page_token`] passes — so
/// its tokens are exactly what they were before the scope was introduced.
pub const CLUSTERS_LISTING: &str = "";
/// The scope `ListAccessPolicies`'s tokens carry. See [`CLUSTERS_LISTING`].
pub const ACCESS_POLICIES_LISTING: &str = "access-policies";

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
fn page_token_tag(listing: &str, last_returned: &str) -> [u8; PAGE_TOKEN_TAG_LEN] {
    let mut digest = Sha256::new();
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
