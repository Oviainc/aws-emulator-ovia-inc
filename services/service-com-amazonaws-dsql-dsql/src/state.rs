// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-dsql-dsql service.
//!
//! The service's primary resource is the Aurora DSQL cluster, so the store is a
//! map of clusters keyed by their 26-character identifier, plus the two
//! bookkeeping maps the cluster lifecycle needs: the retired (deleted) clusters
//! a repeated `@idempotent` `DeleteCluster` must still answer from, and the
//! `clientToken` ledgers that make every `@idempotencyToken`-carrying operation
//! idempotent.
//!
//! A change data capture (CDC) stream belongs to exactly one cluster — every
//! stream operation is reached at `/stream/{clusterIdentifier}/...` and a
//! `StreamArn` nests the stream under its cluster's ARN — so a cluster's streams
//! are stored ON the cluster record rather than in a second map beside it: the
//! cluster's deletion takes its streams with it, and no handler can read one of
//! two copies.
//!
//! Besides the data, this module owns the few derivations every cluster handler
//! shares — the request's region, the cluster ARN, the connection endpoint, the
//! encryption details and the modelled "no such resource" errors — so the
//! handlers compute them one way rather than once each.
//!
//! The tagging operations address a resource by a caller-supplied `resourceArn`
//! rather than by identifier, so the read of that ARN ([`parse_resource_arn`]), the
//! tags it resolves to ([`ComAmazonawsDsqlDsqlState::resource_tags_mut`]) and the
//! refusal of a value that names no resource of this service
//! ([`invalid_resource_arn`]) live here too, for the same reason.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::sha2::{Digest, Sha256};

use crate::ops::types::{
    ConflictException, EncryptionDetails, MultiRegionProperties, ResourceNotFoundException,
    TargetDefinition, ValidationException, ValidationExceptionField,
};

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

/// The `StreamStatus` values a stream is driven through, spelled as the enum
/// spells them on the wire (`smithy_query(query="shape", shape="StreamStatus")`:
/// ACTIVE | CREATING | DELETED | DELETING | FAILED | IMPAIRED).
pub const STREAM_STATUS_CREATING: &str = "CREATING";
/// See [`STREAM_STATUS_CREATING`].
pub const STREAM_STATUS_ACTIVE: &str = "ACTIVE";

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

/// The resource-type segment of a stream ARN
/// (`...:cluster/{cluster}/stream/{stream}`), and the `resourceType` a stream's
/// `ResourceNotFoundException` carries.
pub const RESOURCE_TYPE_STREAM: &str = "stream";

/// The `resourceType` the `ResourceNotFoundException` of a cluster's
/// resource-based policy carries. A policy has no ARN of its own: it is addressed
/// by the cluster's identifier.
pub const RESOURCE_TYPE_POLICY: &str = "policy";

/// The `ValidationExceptionReason` for a member whose value failed this service's
/// own check (`smithy_query(query="shape", shape="ValidationExceptionReason")`:
/// cannotParse | deletionProtectionEnabled | fieldValidationFailed | other |
/// unknownOperation).
pub const VALIDATION_REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// How long a cluster or a stream stays in a transient status (`CREATING`,
/// `UPDATING`, `DELETING`) before the clock settles it into its terminal one.
///
/// A span this emulator CHOOSES, not a duration the service documents: AWS gives
/// no timing contract for cluster or stream creation. It is deliberately
/// non-zero so a caller that acts immediately after `CreateCluster` or
/// `CreateStream` still observes `CREATING` and the state errors that go with it.
pub const SETTLE_AFTER: Duration = Duration::from_millis(750);

/// A status change the clock owes a cluster or a stream: at `due_at` it becomes
/// `next_status`, whether or not anybody calls.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    /// The instant the change became (or becomes) due.
    pub due_at: SystemTime,
    /// The `ClusterStatus` (or `StreamStatus`) the resource holds from `due_at` on.
    pub next_status: &'static str,
}

/// The resource-based policy attached to a cluster: the document the caller
/// supplied and the version token `PutClusterPolicy` answered with.
#[derive(Debug, Clone)]
pub struct ClusterPolicy {
    /// The `PolicyDocument` verbatim.
    pub document: String,
    /// The `PolicyVersion` token this document was stored under.
    pub version: String,
}

/// One persisted change data capture (CDC) stream of a cluster.
#[derive(Debug, Clone)]
pub struct Stream {
    /// The 26-character lowercase identifier (`StreamId`).
    pub stream_identifier: String,
    /// The stream ARN, as [`stream_arn`] builds it.
    pub arn: String,
    /// The current `StreamStatus`.
    pub status: String,
    /// When `CreateStream` minted this stream.
    pub creation_time: SystemTime,
    /// The `StreamOrdering` the caller asked for.
    pub ordering: String,
    /// The `StreamFormat` the caller asked for.
    pub format: String,
    /// Where the stream writes its records.
    pub target_definition: TargetDefinition,
    /// The stream's tags. Sorted so a read is deterministic.
    pub tags: BTreeMap<String, String>,
    /// The status change the clock owes this stream, if any.
    pub pending: Option<PendingTransition>,
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
    /// The customer-managed `kmsEncryptionKey` the caller supplied, verbatim, if
    /// the cluster has one. `None` when the cluster is on the key owned by Amazon
    /// Web Services — whether because no key was ever supplied or because the
    /// reserved `AWS_OWNED_KMS_KEY` switched it back (see
    /// [`customer_managed_key`]).
    pub kms_encryption_key: Option<String>,
    /// The encryption configuration derived from `kms_encryption_key`.
    pub encryption_details: Option<EncryptionDetails>,
    /// Multi-Region configuration, if the cluster has any.
    pub multi_region_properties: Option<MultiRegionProperties>,
    /// The connection endpoint, as [`cluster_endpoint`] builds it.
    pub endpoint: Option<String>,
    /// The cluster's tags. Sorted so a read is deterministic.
    pub tags: BTreeMap<String, String>,
    /// The resource-based policy currently attached, if any.
    pub policy: Option<ClusterPolicy>,
    /// How many policy versions this cluster has ever issued, so a version token
    /// is never reused after a policy is deleted and another attached — an
    /// `expectedPolicyVersion` from the first policy cannot match the second.
    pub policy_versions_issued: u64,
    /// The cluster's CDC streams, keyed by `StreamId`. Bounded by
    /// [`MAX_STREAMS_PER_CLUSTER`], which `CreateStream` checks before it
    /// inserts.
    pub streams: BTreeMap<String, Stream>,
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

    /// Mint the next unused `PolicyVersion` token for this cluster and record
    /// that it was issued.
    pub fn next_policy_version(&mut self) -> String {
        self.policy_versions_issued += 1;
        self.policy_versions_issued.to_string()
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

/// The most tags one resource holds.
///
/// Not a guess: `TagMap` is modelled `@length(min: 0, max: 200)`
/// (`smithy_query(query="shape", shape="TagMap")`), and every reader of a
/// resource's tags reports them through that shape, so a resource holding more
/// tags than the map admits could not be reported at all. A write that would
/// leave a resource past this ceiling is refused with the tagging operation's
/// own `ServiceQuotaExceededException`.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The most `clientToken` entries one ledger keeps.
///
/// At most one token is recorded per call that carried one, and a token is
/// dropped with the resource it names, so this cap is rarely the binding one; it
/// is enforced anyway, by evicting the oldest token, so a ledger cannot
/// accumulate caller-chosen strings without bound.
pub const MAX_CLIENT_TOKENS: usize = MAX_CLUSTERS;

/// The cap on `CreateCluster`'s ledger, named for the operation; every ledger
/// shares [`MAX_CLIENT_TOKENS`].
pub const MAX_CREATE_TOKENS: usize = MAX_CLIENT_TOKENS;

/// The most CDC streams one cluster holds.
///
/// A ceiling this emulator CHOOSES for the same reason as [`MAX_CLUSTERS`]:
/// every stream lives in memory for the life of the process. `CreateStream`
/// models `ServiceQuotaExceededException`, which is the refusal once a cluster's
/// stream table is full.
pub const MAX_STREAMS_PER_CLUSTER: usize = 20;

/// Which operation's `clientToken`s a ledger holds. One ledger per operation, so
/// the same token used on two different operations is two separate requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenScope {
    /// `CreateCluster.clientToken`.
    CreateCluster,
    /// `UpdateCluster.clientToken`.
    UpdateCluster,
    /// `CreateStream.clientToken`.
    CreateStream,
    /// `PutClusterPolicy.clientToken`.
    PutClusterPolicy,
    /// `DeleteClusterPolicy.clientToken`.
    DeleteClusterPolicy,
}

impl TokenScope {
    /// Every scope, in ledger order. The array the state holds is indexed by
    /// this order, so the two cannot drift.
    pub const ALL: [TokenScope; 5] = [
        TokenScope::CreateCluster,
        TokenScope::UpdateCluster,
        TokenScope::CreateStream,
        TokenScope::PutClusterPolicy,
        TokenScope::DeleteClusterPolicy,
    ];

    /// This scope's slot in the state's ledger array.
    fn index(self) -> usize {
        match self {
            TokenScope::CreateCluster => 0,
            TokenScope::UpdateCluster => 1,
            TokenScope::CreateStream => 2,
            TokenScope::PutClusterPolicy => 3,
            TokenScope::DeleteClusterPolicy => 4,
        }
    }
}

/// What a `clientToken` the ledger may already hold means for the call carrying
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenVerdict {
    /// The token has not been used on this operation: serve the request.
    Unused,
    /// The token was used with the SAME parameters: this is a retry of that
    /// request, so answer from the resource it names and have no further effect.
    Replay(String),
    /// The token was used with DIFFERENT parameters: refuse with the
    /// operation's `ConflictException`.
    Mismatch,
}

/// One recorded use of a `clientToken`.
#[derive(Debug, Clone)]
struct TokenEntry {
    /// [`request_fingerprint`] of the parameters the first call carried.
    fingerprint: String,
    /// The resource that call acted on, as the handler names it.
    resource: String,
}

/// The `clientToken`s one operation has already served, bounded by
/// [`MAX_CLIENT_TOKENS`].
#[derive(Debug, Default)]
pub struct TokenLedger {
    entries: HashMap<String, TokenEntry>,
    /// Insertion order, so the cap evicts the oldest token rather than an
    /// arbitrary one.
    order: VecDeque<String>,
}

impl TokenLedger {
    /// What `token` means for a call carrying `fingerprint`.
    pub fn verdict(&self, token: &str, fingerprint: &str) -> TokenVerdict {
        match self.entries.get(token) {
            None => TokenVerdict::Unused,
            Some(entry) if entry.fingerprint == fingerprint => {
                TokenVerdict::Replay(entry.resource.clone())
            }
            Some(_) => TokenVerdict::Mismatch,
        }
    }

    /// Record that `token` served a request with `fingerprint` against
    /// `resource`, evicting the oldest token first if that would push the ledger
    /// past [`MAX_CLIENT_TOKENS`].
    pub fn record(&mut self, token: String, fingerprint: String, resource: &str) {
        while self.entries.len() >= MAX_CLIENT_TOKENS {
            match self.order.pop_front() {
                Some(oldest) => {
                    self.entries.remove(&oldest);
                }
                // The order queue and the map are written together, so this is
                // unreachable; clearing rather than looping forever keeps the
                // cap an invariant even if they ever drift.
                None => {
                    self.entries.clear();
                    break;
                }
            }
        }
        let entry = TokenEntry {
            fingerprint,
            resource: resource.to_string(),
        };
        if self.entries.insert(token.clone(), entry).is_none() {
            self.order.push_back(token);
        }
    }

    /// How many tokens the ledger holds.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the ledger holds no tokens.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Drop every token recorded against `resource`, so a token never outlives
    /// the resource it names.
    pub fn forget(&mut self, resource: &str) {
        self.entries.retain(|_, entry| entry.resource != resource);
        let live = &self.entries;
        self.order.retain(|token| live.contains_key(token));
    }
}

/// A fingerprint of the parameters a `clientToken` was first used with.
///
/// The `@idempotencyToken` contract compares the WHOLE request, so a handler
/// renders every member it honours into `parts` (in a fixed order, maps and
/// lists sorted) and this hashes them into one comparable value. Hashed rather
/// than stored verbatim so a ledger entry's size does not grow with the request
/// it describes.
pub fn request_fingerprint(parts: &[String]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        // The length prefix keeps the parts unambiguous: without it
        // ["a", "bc"] and ["ab", "c"] hash alike.
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    ::hex::encode(hasher.finalize())
}

/// Render a tag map into one fingerprint part, in key order.
pub fn fingerprint_tags(tags: Option<&std::collections::HashMap<String, String>>) -> String {
    let sorted: BTreeMap<&str, &str> = tags
        .map(|tags| {
            tags.iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect()
        })
        .unwrap_or_default();
    sorted
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Render `multiRegionProperties` into one fingerprint part, peer clusters in
/// sorted order (the member is a set of peers, so the order a caller sends them
/// in is not part of the request's meaning).
pub fn fingerprint_multi_region(properties: Option<&MultiRegionProperties>) -> String {
    match properties {
        None => String::new(),
        Some(properties) => {
            let mut clusters: Vec<&str> = properties
                .clusters
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(String::as_str)
                .collect();
            clusters.sort_unstable();
            format!(
                "witness={};clusters={}",
                properties.witness_region.as_deref().unwrap_or_default(),
                clusters.join(",")
            )
        }
    }
}

/// The message every `@idempotencyToken` operation refuses a token reuse with.
///
/// The token names a request, so reusing it for a DIFFERENT request is a
/// conflict rather than a retry; AWS states it with this sentence.
pub const CLIENT_TOKEN_MISMATCH_MESSAGE: &str =
    "Requests using a client token should supply identical request parameters";

/// Per-service state for com-amazonaws-dsql-dsql. A handler reaches it through [`service_state`].
///
/// Every collection here is caller-driven and therefore BOUNDED: see
/// [`MAX_CLUSTERS`], [`MAX_RETIRED_CLUSTERS`], [`MAX_CLIENT_TOKENS`] and
/// [`MAX_STREAMS_PER_CLUSTER`].
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
    /// One `clientToken` ledger per [`TokenScope`], reached through
    /// [`Self::tokens`] / [`Self::tokens_of`].
    tokens: [TokenLedger; TokenScope::ALL.len()],
}

impl ComAmazonawsDsqlDsqlState {
    /// The `clientToken` ledger of `scope`, for writing.
    pub fn tokens(&mut self, scope: TokenScope) -> &mut TokenLedger {
        &mut self.tokens[scope.index()]
    }

    /// The `clientToken` ledger of `scope`, for reading.
    pub fn tokens_of(&self, scope: TokenScope) -> &TokenLedger {
        &self.tokens[scope.index()]
    }

    /// Materialise every status change the clock already owes at `now`.
    ///
    /// Called by reporting operations too: it writes nothing a caller asked for,
    /// only what was already due at this instant even if nobody had called.
    ///
    /// The scan is over [`Self::clusters`], which is capped at [`MAX_CLUSTERS`],
    /// and each cluster's streams, capped at [`MAX_STREAMS_PER_CLUSTER`], so the
    /// per-request cost and the critical section it runs in are bounded by a
    /// constant rather than by anything a caller can grow. Nothing is allocated
    /// unless a transition is actually due.
    pub fn settle(&mut self, now: SystemTime) {
        let is_due = |pending: &Option<PendingTransition>| {
            pending.as_ref().is_some_and(|pending| pending.due_at <= now)
        };
        let anything_due = self.clusters.values().any(|cluster| {
            is_due(&cluster.pending)
                || cluster
                    .streams
                    .values()
                    .any(|stream| is_due(&stream.pending))
        });
        if !anything_due {
            return;
        }

        // Every cluster is visited, because a settled stream transition can be
        // owed to a cluster that owes none itself.
        let identifiers: Vec<String> = self.clusters.keys().cloned().collect();

        for identifier in identifiers {
            let Some(cluster) = self.clusters.get_mut(&identifier) else {
                continue;
            };
            for stream in cluster.streams.values_mut() {
                let settled = stream
                    .pending
                    .as_ref()
                    .filter(|pending| pending.due_at <= now)
                    .map(|pending| pending.next_status);
                if let Some(next_status) = settled {
                    stream.status = next_status.to_string();
                    stream.pending = None;
                }
            }
            if !is_due(&cluster.pending) {
                continue;
            }
            let Some(pending) = cluster.pending.take() else {
                continue;
            };
            cluster.status = pending.next_status.to_string();
            // A settled deletion retires the cluster: `GetCluster` answers
            // `ResourceNotFoundException` and `ListClusters` stops listing it.
            if pending.next_status == STATUS_DELETED {
                if let Some(retired) = self.clusters.remove(&identifier) {
                    self.forget_tokens_for(&identifier);
                    self.retire(identifier, retired);
                }
            }
        }
    }

    /// Mint a fresh, unused cluster identifier.
    ///
    /// The shape comes from [`new_resource_identifier`]; the loop re-draws rather
    /// than ever handing back an identifier this store already holds, live or
    /// retired.
    pub fn mint_cluster_identifier(&mut self) -> String {
        loop {
            let candidate = new_resource_identifier();
            if !self.clusters.contains_key(&candidate)
                && !self.retired_clusters.contains_key(&candidate)
            {
                return candidate;
            }
        }
    }

    /// Drop every idempotency token recorded against `resource` in every ledger,
    /// so a token never outlives the resource it names.
    fn forget_tokens_for(&mut self, resource: &str) {
        for scope in TokenScope::ALL {
            self.tokens(scope).forget(resource);
        }
    }

    /// The cluster, and the stream of it, that a parsed tagging ARN addresses.
    ///
    /// Both tagged resources of this service are written under the cluster's own
    /// resource type: a cluster is `cluster/{identifier}` and a stream of it is
    /// `cluster/{cluster}/stream/{stream}` (`ClusterArn` / `StreamArn`), and
    /// [`parse_resource_arn`] splits the resource field at its FIRST separator, so
    /// a stream arrives as the type `cluster` with the id `{cluster}/stream/{stream}`.
    /// Anything else addresses nothing here.
    ///
    /// `None` here is a resource that is ABSENT, not a request that is malformed:
    /// the ARN does name this service and one of its resource types, so a sub-path
    /// this service mints no resource for (`cluster/{id}/foo/bar`, or a stream path
    /// carrying more segments) belongs with an unknown identifier — the modelled
    /// 404 — rather than with [`invalid_resource_arn`]'s 400, which is reserved for
    /// a value that names no Aurora DSQL resource at all.
    fn addressed_resource(resource: &ArnResource) -> Option<(&str, Option<&str>)> {
        if resource.resource_type != RESOURCE_TYPE_CLUSTER {
            return None;
        }
        match resource.resource_id.split_once('/') {
            None => Some((resource.resource_id.as_str(), None)),
            Some((cluster, rest)) => {
                let stream = rest
                    .strip_prefix(RESOURCE_TYPE_STREAM)
                    .and_then(|rest| rest.strip_prefix('/'))?;
                (!cluster.is_empty() && !stream.is_empty()).then_some((cluster, Some(stream)))
            }
        }
    }

    /// The tags of the resource `resource` names, if this service holds it.
    ///
    /// The one place a parsed `resourceArn` becomes the tag map it addresses, so
    /// every tagging operation agrees about which ARNs name something here: a
    /// cluster, a stream of a cluster (see [`Self::addressed_resource`]), and
    /// nothing else — a well-formed ARN of any other Aurora DSQL resource type
    /// answers `None` exactly as an unknown cluster id does. A retired (deleted)
    /// cluster is not addressable either; its tombstone exists only for delete
    /// idempotency.
    pub fn resource_tags(&self, resource: &ArnResource) -> Option<&BTreeMap<String, String>> {
        let (cluster_identifier, stream_identifier) = Self::addressed_resource(resource)?;
        let cluster = self.clusters.get(cluster_identifier)?;
        match stream_identifier {
            Some(stream_identifier) => cluster
                .streams
                .get(stream_identifier)
                .map(|stream| &stream.tags),
            None => Some(&cluster.tags),
        }
    }

    /// [`Self::resource_tags`] for a caller that WRITES the tags — the same
    /// resolution, so a tag write and a tag read never disagree about which ARNs
    /// name something.
    pub fn resource_tags_mut(
        &mut self,
        resource: &ArnResource,
    ) -> Option<&mut BTreeMap<String, String>> {
        let (cluster_identifier, stream_identifier) = Self::addressed_resource(resource)?;
        let cluster = self.clusters.get_mut(cluster_identifier)?;
        match stream_identifier {
            Some(stream_identifier) => cluster
                .streams
                .get_mut(stream_identifier)
                .map(|stream| &mut stream.tags),
            None => Some(&mut cluster.tags),
        }
    }

    /// Keep `cluster` as a delete-idempotency tombstone, evicting the oldest one
    /// first if that would push the map past [`MAX_RETIRED_CLUSTERS`].
    fn retire(&mut self, identifier: String, cluster: Cluster) {
        while self.retired_clusters.len() >= MAX_RETIRED_CLUSTERS {
            match self.retired_order.pop_front() {
                Some(oldest) => {
                    self.retired_clusters.remove(&oldest);
                }
                // Same invariant as `TokenLedger::record`: unreachable, and
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

/// The resource an Aurora DSQL ARN names: the region it is scoped to, its type
/// segment and the id after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArnResource {
    /// The region segment, as the ARN spells it (empty when the ARN carries none).
    pub region: String,
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
/// The partition and account fields take no part in the lookup: this emulator
/// serves a single account, and a resource id already names at most one stored
/// resource, so the type and the id are what resolve it. The region is reported
/// rather than matched on, for a caller that needs the region an ARN was minted
/// in (see [`arn_region`]).
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
    let region = fields.next()?;
    let _account = fields.next()?;
    let (resource_type, resource_id) = fields.next()?.split_once(['/', ':'])?;
    if resource_type.is_empty() || resource_id.is_empty() {
        return None;
    }
    Some(ArnResource {
        region: region.to_string(),
        resource_type: resource_type.to_string(),
        resource_id: resource_id.to_string(),
    })
}

/// The ARN of a stream of a cluster:
/// `arn:aws:dsql:{region}:{account}:cluster/{cluster}/stream/{stream}`, which is
/// what `StreamArn`'s pattern
/// (`^arn:aws(-[^:]+)?:dsql:[a-z0-9-]{1,20}:[0-9]{12}:cluster/[a-z0-9]{26}/stream/[a-z0-9]{26}$`,
/// `smithy_query(query="shape", shape="StreamArn")`) accepts.
pub fn stream_arn(region: &str, cluster_identifier: &str, stream_identifier: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "{RESOURCE_TYPE_CLUSTER}/{cluster_identifier}/{RESOURCE_TYPE_STREAM}/{stream_identifier}"
        ),
    )
}

/// The connection endpoint of a cluster, in Aurora DSQL's own form
/// (`{identifier}.dsql.{region}.on.aws`), which satisfies `Endpoint`'s
/// `^[a-zA-Z0-9.-]+$`.
pub fn cluster_endpoint(region: &str, identifier: &str) -> String {
    format!("{identifier}.dsql.{region}.on.aws")
}

/// The region segment of an ARN this service minted
/// (`arn:aws:dsql:{region}:{account}:cluster/{identifier}`), when the ARN is one of
/// this service's and carries a region `ClusterArn`'s pattern accepts.
///
/// A thin accessor over [`parse_resource_arn`], so this module walks an ARN's fields
/// in exactly one place; an ARN of another service names nothing here, for the same
/// reason it names no resource. The segment is narrowed the way [`request_region`]
/// narrows a signing region, so a region read out of an ARN can only ever be
/// interpolated into another well-formed value; anything else answers `None`, and the
/// caller falls back to the request's own region.
pub fn arn_region(arn: &str) -> Option<String> {
    parse_resource_arn(arn)
        .map(|resource| resource.region)
        .filter(|region| is_arn_region_segment(region))
}

/// Whether `value` is what `ClusterArn`'s pattern accepts for its region segment:
/// `[a-z0-9-]{1,20}`.
fn is_arn_region_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 20
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The VPC endpoint service that fronts one cluster:
/// `com.amazonaws.{region}.dsql-{identifier}`.
///
/// An interface VPC endpoint service is named `com.amazonaws.{region}.{service}`, and
/// Aurora DSQL qualifies that per cluster: `GetVpcEndpointServiceName` is keyed by a
/// cluster id and answers `ResourceNotFoundException` for an unknown one, and
/// `ServiceName`'s pattern (`smithy_query(query="shape", shape="ServiceName")`:
/// `^com\.amazonaws\.[a-z0-9-]+\.dsql[a-z0-9-]*$`) admits exactly such a suffix after
/// `dsql`. The cluster's own identifier is the qualifier, so the name is derived from
/// persisted state, stable across reads and distinct per cluster.
///
/// `region` is at most 20 characters and `identifier` is 26, so the result is at most
/// 66 characters — inside `ServiceName`'s `@length(1, 128)`.
pub fn vpc_endpoint_service_name(region: &str, identifier: &str) -> String {
    format!("com.amazonaws.{region}.dsql-{identifier}")
}

/// The customer-managed key a `kmsEncryptionKey` names, if it names one at all.
///
/// `AWS_OWNED_KMS_KEY` is a RESERVED value, not a key identifier: it is the
/// `EncryptionType` member that stands for the key owned by Amazon Web Services,
/// and `UpdateCluster`'s `kmsEncryptionKey` documentation
/// (`smithy_query(query="doc", shape="UpdateClusterInput",
/// member="kmsEncryptionKey")`) spells out that supplying it switches the cluster
/// TO that key. So it selects Amazon-owned encryption and names no customer key —
/// the same answer as a request that carries no `kmsEncryptionKey` at all. Every
/// caller resolves a supplied key through here, so neither the stored key nor the
/// reported [`EncryptionDetails`] can ever hold the sentinel as if it were a key.
pub fn customer_managed_key(kms_encryption_key: Option<&str>) -> Option<&str> {
    kms_encryption_key.filter(|key| *key != ENCRYPTION_AWS_OWNED)
}

/// The `EncryptionDetails` of a cluster, derived from the `kmsEncryptionKey` the
/// caller supplied: a customer-managed key when there is one, the
/// Amazon-owned key when there is not — including when the supplied value is the
/// reserved `AWS_OWNED_KMS_KEY` (see [`customer_managed_key`]).
///
/// `kmsKeyArn` is modelled as an ARN, so a key id or an alias is expanded into
/// one in the request's own region.
pub fn encryption_details(region: &str, kms_encryption_key: Option<&str>) -> EncryptionDetails {
    match customer_managed_key(kms_encryption_key) {
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

/// The length `ClusterId`'s and `StreamId`'s `^[a-z0-9]{26}$` fixes
/// (`smithy_query(query="shape", shape="ClusterId,StreamId")`).
pub const RESOURCE_ID_LEN: usize = 26;

/// Whether `value` is a value `ClusterId`'s and `StreamId`'s shared
/// `^[a-z0-9]{26}$` accepts.
pub fn is_resource_identifier(value: &str) -> bool {
    value.len() == RESOURCE_ID_LEN
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// Encode a list operation's pagination cursor: the identifier of the last entry
/// the page returned, hex-encoded so the `nextToken` a client round-trips is an
/// opaque token rather than a resource name.
///
/// Shared by every paginated reader of this service (`ListClusters` pages over
/// `ClusterId` order, `ListStreams` over a cluster's `StreamId` order), so a
/// cursor is spelled one way.
pub fn encode_pagination_cursor(last_identifier: &str) -> String {
    ::hex::encode(last_identifier.as_bytes())
}

/// Decode a cursor produced by [`encode_pagination_cursor`]: a hex-encoded
/// 26-character resource identifier.
///
/// `None` for anything this service did not issue — not only a token that is not
/// hex, but also one that decodes to anything other than a value
/// [`is_resource_identifier`] accepts, so a fabricated cursor is refused rather
/// than served as a page starting after some arbitrary key.
pub fn decode_pagination_cursor(token: &str) -> Option<String> {
    let decoded = ::hex::decode(token).ok()?;
    let cursor = String::from_utf8(decoded).ok()?;
    is_resource_identifier(&cursor).then_some(cursor)
}

/// A fresh cluster or stream identifier: a ULID, lowercased.
///
/// A ULID is a 48-bit millisecond timestamp followed by 80 bits of entropy,
/// rendered in Crockford base32 — 26 characters, which is the form Aurora DSQL
/// cluster and stream ids take and which satisfies `ClusterId`'s and `StreamId`'s
/// `^[a-z0-9]{26}$`. The timestamp leads, so ULIDs sort in creation order and the
/// identifier order `ListClusters` pages over is creation order.
///
/// Prefer [`ComAmazonawsDsqlDsqlState::mint_cluster_identifier`], which re-draws
/// rather than ever handing back an id already in use.
pub fn new_resource_identifier() -> String {
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

/// The modelled `ResourceNotFoundException` for a resource this service does not
/// hold, worded the way AWS words it: the message NAMES THE ARN the call
/// addressed, which is how a caller learns which resource was missing. Every
/// member of the shape is `@required`, so all three are filled.
pub fn resource_not_found(
    arn: &str,
    resource_id: &str,
    resource_type: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("The resource with ARN {arn} doesn't exist. Verify the ARN and try again."),
        resource_id: resource_id.to_string(),
        resource_type: resource_type.to_string(),
    }
}

/// The modelled `ValidationException` for a `resourceArn`-shaped member whose value
/// names no resource of this service at all, as [`parse_resource_arn`] reads it.
///
/// It names the member and the requirement the value failed, the way the boundary
/// validator does, so the caller can fix the request from the message alone, and it
/// fills the shape's `@required` `reason` so a client that branches on it can.
pub fn invalid_resource_arn(member: &str, value: &str) -> ValidationException {
    let requirement = format!(
        "must be the ARN of an Aurora DSQL resource \
         (arn:<partition>:{ARN_SERVICE}:<region>:<account>:<type>/<id>)"
    );
    ValidationException {
        message: format!("{member} {requirement}, got {value}"),
        reason: VALIDATION_REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: requirement,
        }]),
    }
}

/// The modelled `ResourceNotFoundException` for a cluster this service does not
/// hold: [`resource_not_found`] over the ARN the addressed identifier has in the
/// request's own region.
pub fn cluster_not_found(region: &str, identifier: &str) -> ResourceNotFoundException {
    resource_not_found(
        &cluster_arn(region, identifier),
        identifier,
        RESOURCE_TYPE_CLUSTER,
    )
}

/// The modelled `ResourceNotFoundException` for a stream a cluster does not hold.
pub fn stream_not_found(
    region: &str,
    cluster_identifier: &str,
    stream_identifier: &str,
) -> ResourceNotFoundException {
    resource_not_found(
        &stream_arn(region, cluster_identifier, stream_identifier),
        stream_identifier,
        RESOURCE_TYPE_STREAM,
    )
}

/// The modelled `ResourceNotFoundException` for a cluster that has no
/// resource-based policy attached.
///
/// A policy is not an ARN-addressed resource of its own — the call addresses it
/// by the cluster's identifier — so this is the sentence AWS words that case
/// with, built from the identifier the request carried.
pub fn cluster_policy_not_found(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!(
            "Aurora DSQL can't find the policy for the resource {identifier}. \
             Verify that the policy exists."
        ),
        resource_id: identifier.to_string(),
        resource_type: RESOURCE_TYPE_POLICY.to_string(),
    }
}

/// The sentence every refusal of a call whose `expectedPolicyVersion` does not
/// name the version currently attached is worded with, BUILT FROM THE REQUEST: it
/// names the version the caller claimed and the resource it claimed it for, which
/// is how a caller learns which version was not found.
///
/// One sentence for the whole class, whatever status the refusal carries: a claim
/// about a policy that is not attached at all is a `ResourceNotFoundException`
/// ([`cluster_policy_version_not_found`]) and a claim about a version that is no
/// longer the current one is a `ConflictException`
/// ([`cluster_policy_version_conflict`]), so no handler words this case inline.
pub fn cluster_policy_version_message(identifier: &str, expected_version: &str) -> String {
    format!(
        "Aurora DSQL can't find policy version {expected_version} for resource {identifier}. \
         Verify that the policy exists and that the version is correct."
    )
}

/// The modelled `ResourceNotFoundException` for a call that named a policy
/// version the cluster does not hold (including the version of a policy that is
/// already gone).
pub fn cluster_policy_version_not_found(
    identifier: &str,
    expected_version: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: cluster_policy_version_message(identifier, expected_version),
        resource_id: identifier.to_string(),
        resource_type: RESOURCE_TYPE_POLICY.to_string(),
    }
}

/// The modelled `ConflictException` for a call whose `expectedPolicyVersion`
/// names a version other than the one currently attached: the optimistic-
/// concurrency refusal, worded with the same request-derived sentence as the
/// sibling 404 ([`cluster_policy_version_message`]) so a client reading the text
/// can tell a version conflict from any other conflict.
pub fn cluster_policy_version_conflict(
    identifier: &str,
    expected_version: &str,
) -> ConflictException {
    ConflictException {
        message: cluster_policy_version_message(identifier, expected_version),
        resource_id: Some(identifier.to_string()),
        resource_type: Some(RESOURCE_TYPE_POLICY.to_string()),
    }
}

