// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-dsql-dsql service.
//!
//! The service's primary resource is the Aurora DSQL cluster, so the store is a
//! map of clusters keyed by their 26-character identifier, plus the two
//! bookkeeping maps the cluster lifecycle needs: the retired (deleted) clusters
//! a repeated `@idempotent` `DeleteCluster` must still answer from, and the
//! `clientToken` -> identifier bindings that make `CreateCluster` idempotent.
//!
//! Besides the data, this module owns the few derivations every cluster handler
//! shares — the request's region, the cluster ARN, the connection endpoint, the
//! encryption details and the modelled "no such cluster" error — so the five
//! handlers compute them one way rather than five.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::sha2::{Digest, Sha256};

use crate::ops::types::{EncryptionDetails, MultiRegionProperties, ResourceNotFoundException};

/// The `ClusterStatus` values this service drives a cluster through, spelled as
/// the enum spells them on the wire (`smithy_query(query="shape",
/// shape="ClusterStatus")`: CREATING | ACTIVE | IDLE | INACTIVE | UPDATING |
/// DELETING | DELETED | FAILED | PENDING_SETUP | PENDING_DELETE).
pub const STATUS_CREATING: &str = "CREATING";
/// See [`STATUS_CREATING`].
pub const STATUS_ACTIVE: &str = "ACTIVE";
/// See [`STATUS_CREATING`].
pub const STATUS_IDLE: &str = "IDLE";
/// See [`STATUS_CREATING`].
pub const STATUS_UPDATING: &str = "UPDATING";
/// See [`STATUS_CREATING`].
pub const STATUS_DELETING: &str = "DELETING";
/// See [`STATUS_CREATING`].
pub const STATUS_DELETED: &str = "DELETED";

/// `EncryptionType` values (`smithy_query(query="shape", shape="EncryptionType")`).
pub const ENCRYPTION_AWS_OWNED: &str = "AWS_OWNED_KMS_KEY";
/// See [`ENCRYPTION_AWS_OWNED`].
pub const ENCRYPTION_CUSTOMER_MANAGED: &str = "CUSTOMER_MANAGED_KMS_KEY";
/// `EncryptionStatus` value for a cluster whose encryption is in place
/// (`smithy_query(query="shape", shape="EncryptionStatus")`).
pub const ENCRYPTION_STATUS_ENABLED: &str = "ENABLED";

/// The region an ARN and an endpoint are scoped to when the request carries no
/// SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The service segment of every ARN this service mints and resolves
/// (`ClusterArn`'s pattern fixes it: `^arn:aws(-[^:]+)?:dsql:...$`).
pub const ARN_SERVICE: &str = "dsql";

/// The resource-type segment of a cluster ARN (`...:cluster/{identifier}`), and
/// the `resourceType` a cluster's `ResourceNotFoundException` carries.
pub const RESOURCE_TYPE_CLUSTER: &str = "cluster";

/// How long a cluster stays in a transient status (`CREATING`, `UPDATING`,
/// `DELETING`) before the clock settles it into its terminal one.
///
/// A span this emulator CHOOSES, not a duration the service documents: AWS gives
/// no timing contract for cluster creation. It is deliberately non-zero so a
/// caller that acts immediately after `CreateCluster` still observes `CREATING`
/// and the state errors that go with it.
pub const SETTLE_AFTER: Duration = Duration::from_millis(750);

/// A status change the clock owes a cluster: at `due_at` it becomes
/// `next_status`, whether or not anybody calls.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    /// The instant the change became (or becomes) due.
    pub due_at: SystemTime,
    /// The `ClusterStatus` the cluster holds from `due_at` on.
    pub next_status: &'static str,
}

/// One persisted Aurora DSQL cluster.
#[derive(Debug, Clone)]
pub struct Cluster {
    /// The 26-character lowercase identifier (`ClusterId`).
    pub identifier: String,
    /// The cluster ARN, as [`cluster_arn`] builds it.
    pub arn: String,
    /// The current `ClusterStatus`.
    pub status: String,
    /// When `CreateCluster` minted this cluster.
    pub creation_time: SystemTime,
    /// Whether the cluster currently refuses deletion.
    pub deletion_protection_enabled: bool,
    /// The `kmsEncryptionKey` the caller supplied, verbatim, if any.
    pub kms_encryption_key: Option<String>,
    /// The encryption configuration derived from `kms_encryption_key`.
    pub encryption_details: Option<EncryptionDetails>,
    /// Multi-Region configuration, if the cluster has any.
    pub multi_region_properties: Option<MultiRegionProperties>,
    /// The connection endpoint, as [`cluster_endpoint`] builds it.
    pub endpoint: Option<String>,
    /// The cluster's tags. Sorted so a read is deterministic.
    pub tags: BTreeMap<String, String>,
    /// The resource-based policy document, if one was supplied.
    pub policy: Option<String>,
    /// The version of the stored policy, if one was supplied.
    pub policy_version: Option<String>,
    /// The status change the clock owes this cluster, if any.
    pub pending: Option<PendingTransition>,
    /// The `clientToken` of the `DeleteCluster` call that retired this cluster,
    /// so a retry of that exact call answers from the tombstone.
    pub delete_client_token: Option<String>,
}

impl Cluster {
    /// Whether the cluster is in a status that accepts a mutation. A cluster the
    /// clock still owes a transition (`CREATING`, `UPDATING`) or one already on
    /// its way out (`DELETING`) is not.
    pub fn accepts_mutation(&self) -> bool {
        self.status == STATUS_ACTIVE || self.status == STATUS_IDLE
    }
}

/// The most live clusters this emulator stores for one account.
///
/// A ceiling, not a guess at Aurora DSQL's published quota: a generated emulator
/// keeps every resource in memory for the life of the process, so without a
/// maximum entry count a `CreateCluster` loop grows its heap without bound.
/// `CreateCluster` models `ServiceQuotaExceededException`, which is the refusal
/// once the store is full.
pub const MAX_CLUSTERS: usize = 100;

/// The most retired (deleted) clusters kept as delete-idempotency tombstones.
///
/// A tombstone exists only so a retry of the `@idempotent` `DeleteCluster` that
/// retired the cluster answers with the same result; it is never read for anything
/// else. The cap is enforced by evicting the oldest tombstone, so a create/delete
/// loop cannot accumulate records forever even while never holding more than one
/// live cluster.
pub const MAX_RETIRED_CLUSTERS: usize = MAX_CLUSTERS;

/// The most `clientToken` entries kept for `CreateCluster` idempotency.
///
/// At most one token is recorded per stored cluster, and a token is dropped with
/// the cluster it minted, so this cap is never the binding one in practice; it is
/// enforced anyway, by evicting the oldest token, so the map cannot accumulate
/// caller-chosen strings independently of the clusters they name.
pub const MAX_CREATE_TOKENS: usize = MAX_CLUSTERS;

/// Per-service state for com-amazonaws-dsql-dsql. A handler reaches it through [`service_state`].
///
/// Every collection here is caller-driven and therefore BOUNDED: see
/// [`MAX_CLUSTERS`], [`MAX_RETIRED_CLUSTERS`] and [`MAX_CREATE_TOKENS`].
#[derive(Debug, Default)]
pub struct ComAmazonawsDsqlDsqlState {
    /// Live clusters, keyed by identifier. A `BTreeMap` because `ListClusters`
    /// pages over identifier order, and that order has to be the same on every
    /// page for the cursor to resume where the previous page stopped. Bounded by
    /// [`MAX_CLUSTERS`], which `CreateCluster` checks before it inserts.
    pub clusters: BTreeMap<String, Cluster>,
    /// Clusters whose deletion has completed. Kept so a retried
    /// `DeleteCluster` (an `@idempotent` operation) answers with the same
    /// result instead of a 404. Bounded by [`MAX_RETIRED_CLUSTERS`]; written only
    /// by [`Self::settle`], never directly.
    pub retired_clusters: BTreeMap<String, Cluster>,
    /// The order [`Self::retired_clusters`] was written in, so the cap evicts the
    /// oldest tombstone rather than an arbitrary one.
    retired_order: VecDeque<String>,
    /// `CreateCluster` `clientToken` -> the identifier that call minted. Bounded
    /// by [`MAX_CREATE_TOKENS`]; write it through [`Self::record_create_token`],
    /// never directly.
    pub create_client_tokens: HashMap<String, String>,
    /// The order [`Self::create_client_tokens`] was written in, so the cap evicts
    /// the oldest token rather than an arbitrary one.
    create_token_order: VecDeque<String>,
}

impl ComAmazonawsDsqlDsqlState {
    /// Materialise every status change the clock already owes at `now`.
    ///
    /// Called by reporting operations too: it writes nothing a caller asked for,
    /// only what was already due at this instant even if nobody had called.
    ///
    /// The scan is over [`Self::clusters`], which is capped at [`MAX_CLUSTERS`],
    /// so the per-request cost and the critical section it runs in are bounded by
    /// a constant rather than by anything a caller can grow. Nothing is allocated
    /// unless a transition is actually due.
    pub fn settle(&mut self, now: SystemTime) {
        let is_due = |cluster: &Cluster| {
            cluster
                .pending
                .as_ref()
                .is_some_and(|pending| pending.due_at <= now)
        };
        if !self.clusters.values().any(is_due) {
            return;
        }

        let due: Vec<String> = self
            .clusters
            .iter()
            .filter(|(_, cluster)| is_due(cluster))
            .map(|(identifier, _)| identifier.clone())
            .collect();

        for identifier in due {
            let Some(cluster) = self.clusters.get_mut(&identifier) else {
                continue;
            };
            let Some(pending) = cluster.pending.take() else {
                continue;
            };
            cluster.status = pending.next_status.to_string();
            // A settled deletion retires the cluster: `GetCluster` answers
            // `ResourceNotFoundException` and `ListClusters` stops listing it.
            if pending.next_status == STATUS_DELETED {
                if let Some(retired) = self.clusters.remove(&identifier) {
                    self.forget_create_tokens_for(&identifier);
                    self.retire(identifier, retired);
                }
            }
        }
    }

    /// Mint a fresh, unused cluster identifier.
    ///
    /// The shape comes from [`new_cluster_identifier`]; the loop re-draws rather
    /// than ever handing back an identifier this store already holds, live or
    /// retired.
    pub fn mint_cluster_identifier(&mut self) -> String {
        loop {
            let candidate = new_cluster_identifier();
            if !self.clusters.contains_key(&candidate)
                && !self.retired_clusters.contains_key(&candidate)
            {
                return candidate;
            }
        }
    }

    /// Record `token` as having minted `identifier`, evicting the oldest token
    /// first if that would push the map past [`MAX_CREATE_TOKENS`].
    pub fn record_create_token(&mut self, token: String, identifier: &str) {
        while self.create_client_tokens.len() >= MAX_CREATE_TOKENS {
            match self.create_token_order.pop_front() {
                Some(oldest) => {
                    self.create_client_tokens.remove(&oldest);
                }
                // The order queue and the map are written together, so this is
                // unreachable; clearing rather than looping forever keeps the cap
                // an invariant even if they ever drift.
                None => {
                    self.create_client_tokens.clear();
                    break;
                }
            }
        }
        if self
            .create_client_tokens
            .insert(token.clone(), identifier.to_string())
            .is_none()
        {
            self.create_token_order.push_back(token);
        }
    }

    /// Drop every idempotency token that minted `identifier`, so a token never
    /// outlives the cluster it names.
    fn forget_create_tokens_for(&mut self, identifier: &str) {
        self.create_client_tokens
            .retain(|_, minted| minted != identifier);
        let live = &self.create_client_tokens;
        self.create_token_order
            .retain(|token| live.contains_key(token));
    }

    /// Keep `cluster` as a delete-idempotency tombstone, evicting the oldest one
    /// first if that would push the map past [`MAX_RETIRED_CLUSTERS`].
    fn retire(&mut self, identifier: String, cluster: Cluster) {
        while self.retired_clusters.len() >= MAX_RETIRED_CLUSTERS {
            match self.retired_order.pop_front() {
                Some(oldest) => {
                    self.retired_clusters.remove(&oldest);
                }
                // Same invariant as `record_create_token`: unreachable, and
                // clearing keeps the cap rather than spinning.
                None => {
                    self.retired_clusters.clear();
                    break;
                }
            }
        }
        if self
            .retired_clusters
            .insert(identifier.clone(), cluster)
            .is_none()
        {
            self.retired_order.push_back(identifier);
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDsqlDsqlState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDsqlDsqlState>)
}

/// The region this request is scoped to: the region of its SigV4 credential
/// scope, or [`DEFAULT_REGION`] when it carries none.
///
/// Narrowed to what `ClusterArn`'s pattern accepts for the region segment
/// (`[a-z0-9-]{1,20}`), so a cluster minted for an oddly-signed request still
/// carries a well-formed ARN.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    let signed = ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let narrowed: String = signed
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
        .take(20)
        .collect();
    if narrowed.is_empty() {
        DEFAULT_REGION.to_string()
    } else {
        narrowed
    }
}

/// The ARN of a cluster: `arn:aws:dsql:{region}:{account}:cluster/{identifier}`,
/// built through the one ARN helper so the account segment is the emulator's.
pub fn cluster_arn(region: &str, identifier: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{RESOURCE_TYPE_CLUSTER}/{identifier}"),
    )
}

/// The resource an Aurora DSQL ARN names: its type segment and the id after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArnResource {
    /// The resource type, e.g. [`RESOURCE_TYPE_CLUSTER`].
    pub resource_type: String,
    /// The resource's own identifier.
    pub resource_id: String,
}

/// The resource a tagging ARN names, or `None` when the ARN names no Aurora DSQL
/// resource at all.
///
/// An ARN is the six colon-separated fields
/// `arn:{partition}:{service}:{region}:{account}:{resource}`, and its resource
/// field is `{type}/{id}` or `{type}:{id}`; anything else — too few fields, a
/// service field that is not this service's, a resource field with no type —
/// names nothing here.
///
/// The partition, region and account fields take no part in the lookup: this
/// emulator serves a single account, and a resource id already names at most one
/// stored resource, so the type and the id are what resolve it.
pub fn parse_resource_arn(arn: &str) -> Option<ArnResource> {
    // `splitn(6, ..)` keeps the resource field whole: it is the only field that
    // may itself contain a colon.
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let _partition = fields.next()?;
    if fields.next()? != ARN_SERVICE {
        return None;
    }
    let _region = fields.next()?;
    let _account = fields.next()?;
    let (resource_type, resource_id) = fields.next()?.split_once(['/', ':'])?;
    if resource_type.is_empty() || resource_id.is_empty() {
        return None;
    }
    Some(ArnResource {
        resource_type: resource_type.to_string(),
        resource_id: resource_id.to_string(),
    })
}

/// The connection endpoint of a cluster, in Aurora DSQL's own form
/// (`{identifier}.dsql.{region}.on.aws`), which satisfies `Endpoint`'s
/// `^[a-zA-Z0-9.-]+$`.
pub fn cluster_endpoint(region: &str, identifier: &str) -> String {
    format!("{identifier}.dsql.{region}.on.aws")
}

/// The `EncryptionDetails` of a cluster, derived from the `kmsEncryptionKey` the
/// caller supplied: a customer-managed key when there is one, the
/// Amazon-owned key when there is not.
///
/// `kmsKeyArn` is modelled as an ARN, so a key id or an alias is expanded into
/// one in the request's own region.
pub fn encryption_details(region: &str, kms_encryption_key: Option<&str>) -> EncryptionDetails {
    match kms_encryption_key {
        Some(key) => EncryptionDetails {
            encryption_type: ENCRYPTION_CUSTOMER_MANAGED.to_string(),
            kms_key_arn: Some(kms_key_arn(region, key)),
            encryption_status: ENCRYPTION_STATUS_ENABLED.to_string(),
        },
        None => EncryptionDetails {
            encryption_type: ENCRYPTION_AWS_OWNED.to_string(),
            kms_key_arn: None,
            encryption_status: ENCRYPTION_STATUS_ENABLED.to_string(),
        },
    }
}

/// Expand a `kmsEncryptionKey` (an ARN, an alias, or a bare key id) into a KMS
/// key ARN in `region`.
fn kms_key_arn(region: &str, kms_encryption_key: &str) -> String {
    if kms_encryption_key.starts_with("arn:") {
        kms_encryption_key.to_string()
    } else if kms_encryption_key.starts_with("alias/") {
        ::engine::emulator_arn("kms", region, kms_encryption_key)
    } else {
        ::engine::emulator_arn("kms", region, &format!("key/{kms_encryption_key}"))
    }
}

/// A fresh cluster identifier: a ULID, lowercased.
///
/// A ULID is a 48-bit millisecond timestamp followed by 80 bits of entropy,
/// rendered in Crockford base32 — 26 characters, which is the form Aurora DSQL
/// cluster ids take and which satisfies `ClusterId`'s `^[a-z0-9]{26}$`. The
/// timestamp leads, so ULIDs sort in creation order and the identifier order
/// `ListClusters` pages over is creation order.
///
/// Prefer [`ComAmazonawsDsqlDsqlState::mint_cluster_identifier`], which re-draws
/// rather than ever handing back an id already in use.
pub fn new_cluster_identifier() -> String {
    /// Distinguishes two ids minted inside the same nanosecond.
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let millis = since_epoch.as_millis() & ((1u128 << 48) - 1);

    // 80 bits of entropy from a one-way hash of the sub-nanosecond clock, a
    // process-lifetime counter and the process id, so two ids never collide
    // within a run and ids from different runs stay apart.
    let mut hasher = Sha256::new();
    hasher.update(since_epoch.as_nanos().to_be_bytes());
    hasher.update(SEQUENCE.fetch_add(1, Ordering::Relaxed).to_be_bytes());
    hasher.update(std::process::id().to_be_bytes());
    let digest = hasher.finalize();
    let entropy = digest
        .iter()
        .take(10)
        .fold(0u128, |acc, byte| (acc << 8) | u128::from(*byte));

    crockford_base32((millis << 80) | entropy)
}

/// The 32 symbols Crockford base32 (and therefore ULID) uses, lowercased: the
/// digits plus the letters with `i`, `l`, `o` and `u` left out.
const CROCKFORD_SYMBOLS: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// Render the 128 bits of `value` as the 26 Crockford base32 characters a ULID
/// is written with, most significant group first.
fn crockford_base32(value: u128) -> String {
    (0..26)
        .map(|position| {
            let shift = 5 * (25 - position);
            let symbol = ((value >> shift) & 0x1f) as usize;
            char::from(CROCKFORD_SYMBOLS[symbol])
        })
        .collect()
}

/// The modelled `ResourceNotFoundException` for a cluster this service does not
/// hold.
pub fn cluster_not_found(identifier: &str) -> ResourceNotFoundException {
    resource_not_found(RESOURCE_TYPE_CLUSTER, identifier)
}

/// The modelled `ResourceNotFoundException` for any resource of this service that
/// the store does not hold. Every member of the shape is `@required`, so all three
/// are filled.
pub fn resource_not_found(resource_type: &str, resource_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("{resource_type} {resource_id} not found"),
        resource_id: resource_id.to_string(),
        resource_type: resource_type.to_string(),
    }
}
