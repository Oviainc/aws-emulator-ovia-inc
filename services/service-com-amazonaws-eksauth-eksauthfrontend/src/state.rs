// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-eksauth-eksauthfrontend service.
//!
//! EKS Auth binds exactly one operation, `AssumeRoleForPodIdentity`, and models no
//! create/list/delete calls: the cluster and the EKS Pod Identity association that
//! operation authenticates against are created through the EKS control plane
//! (`CreateCluster` / `CreatePodIdentityAssociation`), which is a different service.
//! So this module owns the records that operation reads, plus the registration seam
//! that puts them there:
//!
//! * [`ComAmazonawsEksauthEksauthfrontendState::register_cluster`] registers a cluster,
//!   as the EKS control plane would.
//! * [`Cluster::associate_pod_identity`] registers one pod identity association on it.
//! * [`Cluster::issue_service_account_token`] is the emulated cluster's Kubernetes API
//!   server minting a projected service account token for a pod — the token the EKS Pod
//!   Identity Agent presents to `AssumeRoleForPodIdentity`.
//! * [`Cluster::verify_service_account_token`] is the other half: only a token THIS
//!   cluster issued, for the audience pod identity tokens are issued to, and still
//!   within its lifetime, names a service account.
//!
//! ## Why the token is a record rather than a signature
//!
//! A pod identity token is minted by a cluster's own Kubernetes API server, and only
//! that cluster can say whose service account it names. This emulator IS the cluster, so
//! each cluster keeps the tokens it issued ([`Cluster::issued_tokens`]) with the identity,
//! audience and lifetime it issued them for, and verification is a lookup in that record
//! followed by the clock — never an inspection of the request. A token a cluster did not
//! issue therefore names nobody, which is what the real service answers for a token
//! signed by keys it does not hold, and no claim the caller writes can change who a token
//! names.

use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The identity a pod identity service account token is issued to, and the only value
/// `AssumeRoleForPodIdentity` reports in its `audience` output member: the model
/// documents it as "always `pods.eks.amazonaws.com`".
pub const PODS_AUDIENCE: &str = "pods.eks.amazonaws.com";

/// The region every ARN this service mints is scoped to when a caller of the
/// registration seam names no other. EKS Auth is a regional service; emulators in this
/// project default to `us-east-1`.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long a minted pod identity session stays valid.
///
/// CHOSEN, not documented: AWS does not publish the exact lifetime of an EKS Pod
/// Identity session, so this is the emulator's own span, picked long enough that a
/// caller sees a plainly future `expiration` and short enough that an emulator left
/// running recycles its sessions.
pub const SESSION_DURATION: Duration = Duration::from_secs(6 * 60 * 60);

/// Per-service state for com-amazonaws-eksauth-eksauthfrontend. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsEksauthEksauthfrontendState {
    /// Registered clusters, by cluster name (the `clusterName` http label).
    pub clusters: HashMap<String, Cluster>,
    /// Every pod identity session this service minted, by the access key id that
    /// identifies it.
    pub sessions: HashMap<String, PodIdentitySession>,
    /// The id IAM gave each role this service has seen, by role ARN.
    ///
    /// IAM mints a role's unique id when the role is created and that id stays with the
    /// role; this service holds no IAM records, so it mints one the first time a role is
    /// associated and keeps it here. That is what makes the id a property of the ROLE:
    /// the same role associated in two clusters reports one id, and re-pointing an
    /// association at a role it was already associated with does not change it.
    pub role_ids: HashMap<String, String>,
}

impl ComAmazonawsEksauthEksauthfrontendState {
    /// Register a cluster under `name` in `region`, or return the one already there.
    ///
    /// The registration seam the EKS control plane would drive. `clusterName` is the only
    /// cluster identity a request to this service carries, so a cluster name identifies
    /// one cluster here: the FIRST registration of a name fixes its region, and `region`
    /// is ignored on any later registration of that same name.
    ///
    /// Refused (`None`) for a name EKS itself refuses: the modelled `ClusterName` admits
    /// 1 to 100 characters matching `^[0-9A-Za-z][A-Za-z0-9\-_]*$`, so a name outside that
    /// never names a cluster a request could reach.
    pub fn register_cluster(&mut self, name: &str, region: &str) -> Option<&mut Cluster> {
        if !is_cluster_name(name) {
            return None;
        }
        Some(
            self.clusters
                .entry(name.to_string())
                .or_insert_with(|| Cluster::new(name, region)),
        )
    }

    /// The cluster registered under `name`, if any.
    pub fn cluster(&self, name: &str) -> Option<&Cluster> {
        self.clusters.get(name)
    }

    /// The cluster registered under `name` for modification — what the emulated
    /// Kubernetes API server needs to record a token it issues.
    pub fn cluster_mut(&mut self, name: &str) -> Option<&mut Cluster> {
        self.clusters.get_mut(name)
    }

    /// Register a pod identity association on the cluster named `cluster_name`, binding
    /// `namespace`/`service_account` in it to `role_arn`.
    ///
    /// A (namespace, service account) pair carries at most one association in a cluster,
    /// so registering the pair again re-points THAT association at `role_arn` — what
    /// `UpdatePodIdentityAssociation` does — rather than silently keeping the role it was
    /// first registered with, which no caller could tell from a successful create.
    ///
    /// The association id and ARN take the shapes EKS uses (`a-` plus 17 characters;
    /// `podidentityassociation/<cluster>/<id>`), and the role's id comes from
    /// [`Self::role_ids`], so one role carries one id wherever it is associated.
    ///
    /// Refused with the reason the EKS control plane refuses on: an unregistered cluster,
    /// a `roleArn` that is not an IAM role ARN, or a namespace or service account name
    /// Kubernetes would not accept. Nothing is recorded for a refused registration — not
    /// even the role's id.
    pub fn associate_pod_identity(
        &mut self,
        cluster_name: &str,
        namespace: &str,
        service_account: &str,
        role_arn: &str,
    ) -> Result<&PodIdentityAssociationRecord, AssociationRefusal> {
        if !self.clusters.contains_key(cluster_name) {
            return Err(AssociationRefusal::UnknownCluster);
        }
        if !is_role_arn(role_arn) {
            return Err(AssociationRefusal::RoleArn);
        }
        if !is_dns_label(namespace) {
            return Err(AssociationRefusal::Namespace);
        }
        if !is_dns_subdomain(service_account) {
            return Err(AssociationRefusal::ServiceAccount);
        }
        // The role's id is state the whole service shares, so it is resolved — and minted
        // if this is the first association of the role — only once the registration is
        // going to be recorded.
        let role_id = self.role_id(role_arn);
        let cluster = self
            .clusters
            .get_mut(cluster_name)
            .expect("the cluster checked above is present");
        Ok(cluster.upsert_association(namespace, service_account, role_arn, role_id))
    }

    /// The id IAM gave the role named by `role_arn`, minting and recording one the first
    /// time this service sees the role.
    pub fn role_id(&mut self, role_arn: &str) -> String {
        self.role_ids
            .entry(role_arn.to_string())
            .or_insert_with(|| unique_id("AROA"))
            .clone()
    }

    /// Materialise everything the clock has already made due, and report the access key
    /// ids whose credentials the caller must now delete.
    ///
    /// Two records fall due with time, whether or not anybody called:
    ///
    /// * a session past its `expiration` is over, so it is forgotten and its credential
    ///   named for deletion;
    /// * a token record far enough past the instant its token expired that no caller can
    ///   still be owed [`TokenRejection::Expired`] for it is forgotten too, which is what
    ///   keeps a long-running emulator's token records bounded.
    pub fn expire(&mut self, now: SystemTime) -> Vec<String> {
        // Collected and removed by the MAP KEY, which is the access key id a later
        // request presents: a record keyed under anything else would otherwise survive
        // its own expiry for ever. One pass: `retain` collects as it drops.
        let mut expired = Vec::new();
        self.sessions.retain(|access_key_id, session| {
            let live = session.expiration > now;
            if !live {
                expired.push(access_key_id.clone());
            }
            live
        });
        for cluster in self.clusters.values_mut() {
            cluster.forget_stale_tokens(now);
        }
        expired
    }
}

/// A registered EKS cluster: its identity, the projected service account tokens its
/// Kubernetes API server issued, and its pod identity associations.
#[derive(Debug)]
pub struct Cluster {
    pub name: String,
    pub arn: String,
    /// The region this cluster's ARNs are scoped to.
    pub region: String,
    /// Every projected service account token this cluster's Kubernetes API server issued,
    /// by the token itself: who it names, what it was issued to, and when it expires.
    ///
    /// Kept per cluster, so a token one cluster issued names nobody in another. A record
    /// outlives its own expiry on purpose: a token presented after it expired has to be
    /// answered as expired, not as unknown.
    pub issued_tokens: HashMap<String, IssuedToken>,
    pub associations: Vec<PodIdentityAssociationRecord>,
}

impl Cluster {
    /// A cluster named `name` in `region`, with no issued tokens and no associations.
    fn new(name: &str, region: &str) -> Self {
        Self {
            name: name.to_string(),
            arn: ::engine::emulator_arn("eks", region, &format!("cluster/{name}")),
            region: region.to_string(),
            issued_tokens: HashMap::new(),
            associations: Vec::new(),
        }
    }

    /// Record the association binding `namespace`/`service_account` in this cluster to
    /// `role_arn`, whose IAM role id is `role_id`.
    ///
    /// Private: an association's role id is state the whole service shares, so
    /// registration goes through
    /// [`ComAmazonawsEksauthEksauthfrontendState::associate_pod_identity`], which
    /// resolves the role id first.
    fn upsert_association(
        &mut self,
        namespace: &str,
        service_account: &str,
        role_arn: &str,
        role_id: String,
    ) -> &PodIdentityAssociationRecord {
        if let Some(index) = self
            .associations
            .iter()
            .position(|a| a.namespace == namespace && a.service_account == service_account)
        {
            let existing = &mut self.associations[index];
            if existing.role_arn != role_arn {
                existing.role_arn = role_arn.to_string();
                existing.role_id = role_id;
            }
            return &self.associations[index];
        }
        let association_id = format!("a-{}", random_hex(ASSOCIATION_ID_SUFFIX_LEN));
        let association_arn = ::engine::emulator_arn(
            "eks",
            &self.region,
            &format!("podidentityassociation/{}/{association_id}", self.name),
        );
        self.associations.push(PodIdentityAssociationRecord {
            association_id,
            association_arn,
            namespace: namespace.to_string(),
            service_account: service_account.to_string(),
            role_arn: role_arn.to_string(),
            role_id,
        });
        self.associations
            .last()
            .expect("the association just pushed is present")
    }

    /// The association this cluster holds for `namespace`/`service_account`, if any.
    ///
    /// A pod identity association is unique per (namespace, service account) pair within
    /// a cluster, so at most one can match.
    pub fn association_for(
        &self,
        namespace: &str,
        service_account: &str,
    ) -> Option<&PodIdentityAssociationRecord> {
        self.associations
            .iter()
            .find(|a| a.namespace == namespace && a.service_account == service_account)
    }

    /// Mint a projected service account token for `request` and record what it names —
    /// what this cluster's Kubernetes API server hands a pod, and what the EKS Pod
    /// Identity Agent presents to `AssumeRoleForPodIdentity`.
    ///
    /// The token carries the three dot-separated segments the modelled `JwtToken` shape
    /// requires; its value is opaque, because what it names lives in this cluster's
    /// record rather than in anything the holder can read or rewrite.
    ///
    /// Returns `None` for a request a Kubernetes API server would not serve: one naming
    /// no service account it could issue for (a namespace that is not a DNS label or a
    /// service account that is not a DNS subdomain, which includes the empty name), or
    /// one whose validity window is empty (`expires_at` at or before `issued_at`).
    pub fn issue_service_account_token(
        &mut self,
        request: &ServiceAccountTokenRequest,
    ) -> Option<String> {
        if !is_dns_label(&request.namespace)
            || !is_dns_subdomain(&request.service_account)
            || request.expires_at <= request.issued_at
        {
            return None;
        }
        let token = format!(
            "{}.{}.{}",
            random_hex(TOKEN_SEGMENT_LEN),
            random_hex(TOKEN_SEGMENT_LEN),
            random_hex(TOKEN_SEGMENT_LEN)
        );
        self.issued_tokens.insert(
            token.clone(),
            IssuedToken {
                namespace: request.namespace.clone(),
                service_account: request.service_account.clone(),
                pod_name: request.pod_name.clone(),
                audience: request.audience.clone(),
                issued_at: request.issued_at,
                expires_at: request.expires_at,
            },
        );
        Some(token)
    }

    /// Report whose service account `token` names, as of `now`.
    ///
    /// A token is accepted only when THIS cluster issued it, it was issued to the pod
    /// identity audience, and `now` lies inside the window it was issued for. Anything
    /// else — a token another cluster issued, one no cluster issued, one whose characters
    /// were changed, one issued to a different audience, one presented before its own
    /// issue instant — names nobody, because only the issuing cluster's record can say
    /// who a token belongs to.
    pub fn verify_service_account_token(
        &self,
        token: &str,
        now: SystemTime,
    ) -> Result<ServiceAccountIdentity, TokenRejection> {
        let issued = self
            .issued_tokens
            .get(token)
            .ok_or(TokenRejection::Invalid)?;
        if !issued
            .audience
            .iter()
            .any(|audience| audience == PODS_AUDIENCE)
        {
            return Err(TokenRejection::Invalid);
        }
        // A token is not yet a token before the instant it was issued at, the way a
        // projected token carries a not-before claim its holder cannot anticipate.
        if now < issued.issued_at {
            return Err(TokenRejection::Invalid);
        }
        // No leeway: a token whose lifetime has passed is expired, not nearly expired.
        if issued.expires_at <= now {
            return Err(TokenRejection::Expired);
        }
        Ok(ServiceAccountIdentity {
            namespace: issued.namespace.clone(),
            service_account: issued.service_account.clone(),
            pod_name: issued.pod_name.clone(),
        })
    }

    /// Forget the token records whose tokens expired longer ago than
    /// [`TOKEN_RECORD_GRACE`], so the records this cluster keeps stay bounded while a
    /// token that has only just expired is still answered as expired.
    fn forget_stale_tokens(&mut self, now: SystemTime) {
        // `checked_add`: a record whose expiry plus the grace period is not representable
        // is one no clock reaches, so it is kept rather than panicking the sweep.
        self.issued_tokens.retain(|_, issued| {
            issued
                .expires_at
                .checked_add(TOKEN_RECORD_GRACE)
                .is_none_or(|forget_at| forget_at > now)
        });
    }
}

/// How long a cluster keeps the record of a token after that token expired.
///
/// CHOSEN, not documented: the record only has to outlive the token long enough that a
/// holder presenting a just-expired token is told it expired rather than that it was
/// never issued, so this is the emulator's own span.
pub const TOKEN_RECORD_GRACE: Duration = Duration::from_secs(60 * 60);

/// Why the registration seam refused to record a pod identity association — the reasons
/// the EKS control plane refuses on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssociationRefusal {
    /// No cluster is registered under that name.
    UnknownCluster,
    /// `roleArn` is not the ARN of an IAM role.
    RoleArn,
    /// The namespace is not a Kubernetes DNS label.
    Namespace,
    /// The service account name is not a Kubernetes DNS subdomain.
    ServiceAccount,
}

/// Whether `name` is a cluster name a request to this service could carry: the modelled
/// `ClusterName` admits 1 to 100 characters of `^[0-9A-Za-z][A-Za-z0-9\-_]*$`.
fn is_cluster_name(name: &str) -> bool {
    /// The modelled `ClusterName` length bound.
    const MAX_LEN: usize = 100;
    let mut characters = name.chars();
    // One pass: the length is taken from the same iterator the pattern walks, and the
    // non-empty lower bound is what `next()` yielding a character already says.
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        && characters
            .enumerate()
            .all(|(index, c)| {
                index + 2 <= MAX_LEN && (c.is_ascii_alphanumeric() || c == '-' || c == '_')
            })
}

/// Whether `value` is an RFC 1123 DNS label, which is what Kubernetes requires of a
/// namespace name: 1 to 63 characters of lowercase alphanumerics and `-`, starting and
/// ending alphanumeric.
fn is_dns_label(value: &str) -> bool {
    const MAX_LABEL: usize = 63;
    !value.is_empty()
        && value.len() <= MAX_LABEL
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && value
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && value
            .bytes()
            .next_back()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// Whether `value` is an RFC 1123 DNS subdomain, which is what Kubernetes requires of a
/// service account name: up to 253 characters of DNS labels joined by `.`.
fn is_dns_subdomain(value: &str) -> bool {
    const MAX_SUBDOMAIN: usize = 253;
    !value.is_empty() && value.len() <= MAX_SUBDOMAIN && value.split('.').all(is_dns_label)
}

/// Whether `arn` is the ARN of an IAM role: `arn:<partition>:iam::<account>:role/<path
/// and name>`, the form EKS requires of a pod identity association's `roleArn`. IAM is a
/// global service, so the region field is empty, and the account is twelve digits.
fn is_role_arn(arn: &str) -> bool {
    let mut fields = arn.split(':');
    let (
        Some("arn"),
        Some(partition),
        Some("iam"),
        Some(""),
        Some(account),
        Some(resource),
        None,
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return false;
    };
    !partition.is_empty()
        && account.len() == 12
        && account.bytes().all(|b| b.is_ascii_digit())
        && resource
            .strip_prefix("role/")
            .is_some_and(|name| !name.is_empty() && !name.ends_with('/'))
}

/// One projected service account token a cluster issued: who it names, what it was
/// issued to, and the window it is valid in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedToken {
    pub namespace: String,
    pub service_account: String,
    /// The pod the token was projected into, when it was issued for one.
    pub pod_name: Option<String>,
    /// The audiences the token was issued to; only [`PODS_AUDIENCE`] is the one
    /// `AssumeRoleForPodIdentity` accepts.
    pub audience: Vec<String>,
    pub issued_at: SystemTime,
    pub expires_at: SystemTime,
}

/// The characters in one segment of an issued token: 32 hex characters of
/// [`random_hex`], which keeps the whole token inside the modelled `JwtToken` character
/// set.
const TOKEN_SEGMENT_LEN: usize = 32;

/// The characters after the `a-` prefix of an EKS pod identity association id.
const ASSOCIATION_ID_SUFFIX_LEN: usize = 17;

/// Why a cluster refused a service account token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenRejection {
    /// The instant the token expires has passed.
    Expired,
    /// The token is not one this cluster issued for pod identity: a token it never
    /// issued, or one it issued to a different audience.
    Invalid,
}

/// Whose service account a verified token names, and the pod it was projected into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceAccountIdentity {
    pub namespace: String,
    pub service_account: String,
    /// The pod the token was projected into, when it was issued for one.
    pub pod_name: Option<String>,
}

/// What [`Cluster::issue_service_account_token`] mints a token for.
#[derive(Debug, Clone)]
pub struct ServiceAccountTokenRequest {
    pub namespace: String,
    pub service_account: String,
    pub pod_name: Option<String>,
    pub audience: Vec<String>,
    pub issued_at: SystemTime,
    pub expires_at: SystemTime,
}

impl ServiceAccountTokenRequest {
    /// A token for `service_account` in `namespace`, projected into pod `pod_name`, with
    /// the pod identity audience and a one-hour lifetime starting now — what the
    /// Kubernetes API server issues for the EKS Pod Identity Agent.
    pub fn for_pod(namespace: &str, service_account: &str, pod_name: &str) -> Self {
        let now = SystemTime::now();
        Self {
            namespace: namespace.to_string(),
            service_account: service_account.to_string(),
            pod_name: Some(pod_name.to_string()),
            audience: vec![PODS_AUDIENCE.to_string()],
            issued_at: now,
            expires_at: now + Duration::from_secs(60 * 60),
        }
    }

    /// The same token, expiring at `expires_at` instead.
    pub fn expiring_at(mut self, expires_at: SystemTime) -> Self {
        self.expires_at = expires_at;
        self
    }

    /// The same token, issued at `issued_at` instead — what a cluster whose clock runs
    /// ahead of the one reading the token hands out.
    pub fn issued_at(mut self, issued_at: SystemTime) -> Self {
        self.issued_at = issued_at;
        self
    }

    /// The same token, issued to `audience` instead of the pod identity audience — what
    /// a service account token projected for some other consumer carries.
    pub fn for_audience(mut self, audience: &[&str]) -> Self {
        self.audience = audience.iter().map(|a| (*a).to_string()).collect();
        self
    }
}

/// One pod identity association: a (namespace, service account) pair in a cluster bound
/// to an IAM role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PodIdentityAssociationRecord {
    pub association_id: String,
    pub association_arn: String,
    pub namespace: String,
    pub service_account: String,
    pub role_arn: String,
    /// The IAM role's own id — the first half of `assumedRoleUser.assumeRoleId`.
    pub role_id: String,
}

/// One session `AssumeRoleForPodIdentity` minted: the credentials it returned, when they
/// expire, and which association and pod they were minted for.
#[derive(Debug, Clone)]
pub struct PodIdentitySession {
    pub access_key_id: String,
    pub expiration: SystemTime,
    pub cluster_name: String,
    pub association_id: String,
    pub assumed_role_arn: String,
    pub assume_role_id: String,
    pub namespace: String,
    pub service_account: String,
    pub eks_node_name: Option<String>,
    pub instance_id: Option<String>,
    pub zone: Option<String>,
}

/// The alphabet AWS unique ids (`AIDA…`, `AROA…`) are drawn from: uppercase letters and
/// the digits 2-7, which is what the modelled `idType` (`^[\w]+$`) admits.
const UNIQUE_ID_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// The number of characters after an AWS unique id's four-character type prefix, so that
/// `AROA` plus this is the 21-character id IAM mints for a role.
const UNIQUE_ID_SUFFIX_LEN: usize = 17;

/// An AWS unique id: `prefix` (`AROA` for a role) plus [`UNIQUE_ID_SUFFIX_LEN`]
/// characters of [`UNIQUE_ID_ALPHABET`].
///
/// IAM mints such an id when the resource is created, so this mints one per resource and
/// the caller stores it — see [`ComAmazonawsEksauthEksauthfrontendState::role_ids`]. It
/// is deliberately NOT derived from the resource's name or ARN: a derived id would be the
/// same for two different accounts' roles of the same name, and would change for a role
/// that is renamed, neither of which is true of IAM.
fn unique_id(prefix: &str) -> String {
    let suffix = random_bytes(UNIQUE_ID_SUFFIX_LEN)
        .into_iter()
        .map(|byte| char::from(UNIQUE_ID_ALPHABET[usize::from(byte) % UNIQUE_ID_ALPHABET.len()]));
    prefix.chars().chain(suffix).collect()
}

/// A UUID in the random (version 4) form, which is what the model documents at the end of
/// `assumedRoleUser.assumeRoleId`: `eks-<clustername>-<podname>-<random UUID>`.
///
/// Rendered here from [`random_bytes`] rather than taken from a generator crate, because
/// this crate's ids need to be unique within one emulator process, not reproducible, and
/// the version and variant nibbles are the only structure a v4 UUID has.
pub fn random_uuid() -> String {
    let bytes = random_bytes(16);
    let mut hex = String::with_capacity(36);
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            hex.push('-');
        }
        // Version 4 in the high nibble of byte 6, variant 0b10 in the high bits of
        // byte 8, as a random UUID renders them.
        let byte = match index {
            6 => (byte & 0x0f) | 0x40,
            8 => (byte & 0x3f) | 0x80,
            _ => *byte,
        };
        write!(&mut hex, "{byte:02x}").expect("writing to a String cannot fail");
    }
    hex
}

/// `len` lowercase hex characters of [`random_bytes`] — the form EKS association ids and
/// this service's issued tokens take.
fn random_hex(len: usize) -> String {
    let mut hex = String::with_capacity(len + 1);
    for byte in random_bytes(len.div_ceil(2)) {
        write!(&mut hex, "{byte:02x}").expect("writing to a String cannot fail");
    }
    hex.truncate(len);
    hex
}

/// `len` bytes for an identifier this service mints.
///
/// The standard library is the whole source: `RandomState` seeds its hasher keys from the
/// operating system, so hashing with a FRESH one yields bits that come from that seed,
/// and a process-wide counter is mixed in so two identifiers minted in the same instant
/// differ even if two `RandomState`s ever repeat their keys. Nothing here is derived from
/// a request, and no identifier is reused.
///
/// This is an identifier source, not a key generator: the one value it produces that
/// grants anything — the token a cluster issues — is only ever minted through this
/// in-process seam, never from a request, and the credentials the operation returns come
/// from the emulator's own credential store instead.
fn random_bytes(len: usize) -> Vec<u8> {
    static MINTED: AtomicU64 = AtomicU64::new(0);
    let mut bytes = Vec::with_capacity(len);
    while bytes.len() < len {
        let word = RandomState::new().hash_one(MINTED.fetch_add(1, Ordering::Relaxed));
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes.truncate(len);
    bytes
}

/// Whole seconds since the Unix epoch, which is how the operation's access record states
/// a session's expiry.
///
/// A time before the epoch is reported as 0: the record states an expiry, and no session
/// expires before the epoch.
pub(crate) fn epoch_seconds(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEksauthEksauthfrontendState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEksauthEksauthfrontendState>)
}
