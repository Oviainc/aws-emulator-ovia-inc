// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cloudfrontkeyvaluestore-cloudfrontkeyvaluestore service.
//!
//! This service is the DATA PLANE of a CloudFront key value store: the store resource
//! itself is created by CloudFront's `CreateKeyValueStore` (a different service crate,
//! whose operation is still scaffolded), and every operation here is addressed by the
//! store's ARN. So the record below is materialised on the first reference to a
//! well-formed key-value-store ARN and then reused by every later call: that is what
//! makes `Created` stable across reads and the `ETag` a usable optimistic-concurrency
//! token. An ARN that does not name a key value store materialises nothing — the caller
//! answers `ResourceNotFoundException`.
//!
//! # Bounds
//!
//! Both the store map and each store's pair map are keyed by request data, so each
//! carries an explicit ceiling ([`MAX_STORES`], [`MAX_ITEMS_PER_STORE`],
//! [`MAX_STORE_BYTES`]) and the handlers refuse past it with a modelled error instead
//! of growing. Only [`DescribeKeyValueStore`](crate::ops::describe_key_value_store) and
//! the three mutating operations materialise a record at all; `GetKey` and `ListKeys`
//! resolve through the read-only [`store`](ComAmazonawsCloudfrontkeyvaluestoreCloudfrontkeyvaluestoreState::store)
//! accessor and allocate nothing.

use std::collections::{BTreeMap, HashMap};
use std::time::SystemTime;

use sha2::{Digest, Sha256};

/// The `Status` `DescribeKeyValueStore` reports for a store this service can serve.
///
/// A store reaches this service only once CloudFront has finished provisioning it —
/// nothing here can put keys into a store that is still being created — so the record
/// is materialised in the terminal, usable status and never transitions.
pub const STATUS_READY: &str = "READY";

/// The maximum total size of one key value store, in bytes.
///
/// AWS publishes this quota for CloudFront key value stores: "Maximum size of a key
/// value store — 5 MB" (CloudFront Developer Guide, key value store quotas). A mutating
/// call whose result would exceed it is refused with the modelled
/// `ServiceQuotaExceededException` rather than growing the store further.
pub const MAX_STORE_BYTES: i64 = 5 * 1024 * 1024;

/// The maximum number of pairs one key value store may hold.
///
/// CHOSEN by this emulator — AWS publishes a size quota, not a pair-count one — because
/// [`MAX_STORE_BYTES`] counts only key and value bytes: a store of millions of
/// single-byte pairs stays under the size quota while costing the process orders of
/// magnitude more in map overhead. It is also the ceiling on one `UpdateKeys` batch,
/// since a batch larger than the pairs a store may hold can never succeed.
pub const MAX_ITEMS_PER_STORE: usize = 50_000;

/// The maximum number of distinct key value stores this emulator materialises.
///
/// CHOSEN by this emulator. Stores are keyed by the caller-supplied ARN, so without a
/// ceiling a stream of requests naming fresh ARNs would grow the map for the life of
/// the process. It is far above the number of stores any one emulated scenario uses.
pub const MAX_STORES: usize = 1_000;

/// The one resource path prefix a CloudFront key value store ARN carries.
const KVS_RESOURCE_PREFIX: &str = "key-value-store/";

/// The service field of a CloudFront ARN.
const CLOUDFRONT_SERVICE: &str = "cloudfront";

/// Why a store could not be resolved from an ARN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    /// The identifier does not name a CloudFront key value store at all.
    NotAKeyValueStoreArn,
    /// This emulator already holds [`MAX_STORES`] distinct stores and will not
    /// materialise another.
    StoreLimitReached,
}

/// One pending change per key: `Some(value)` puts it, `None` deletes it.
///
/// Every mutating operation funnels through this one shape — a single put, a single
/// delete, or a whole `UpdateKeys` batch — so all three share one quota projection and
/// one application step. Keys are unique by construction, so a batch naming a key twice
/// collapses to the change that would have been applied last.
pub type PendingChanges<'a> = BTreeMap<&'a str, Option<&'a str>>;

/// Whether a store holding `item_count` pairs and `total_size_in_bytes` bytes of key and
/// value data is within this service's quotas.
pub fn within_quota(item_count: usize, total_size_in_bytes: i64) -> bool {
    item_count <= MAX_ITEMS_PER_STORE && total_size_in_bytes <= MAX_STORE_BYTES
}

/// One key value store's persisted data plane.
///
/// `keys` is a `BTreeMap` so `ListKeys` enumerates in one stable order and a page token
/// naming the last key returned can resume after it.
#[derive(Debug, Clone)]
pub struct KeyValueStore {
    /// The ARN this store is addressed by; echoed back by `DescribeKeyValueStore`.
    pub kvs_arn: String,
    /// The stored pairs, key-ordered.
    pub keys: BTreeMap<String, String>,
    /// The store's current opaque version identifier. A caller's `If-Match` must equal
    /// it, and every accepted mutation replaces it (see [`KeyValueStore::commit`]).
    pub etag: String,
    /// When this store was first referenced through this service.
    pub created: SystemTime,
    /// When the last accepted mutation was applied.
    pub last_modified: SystemTime,
    /// The store's lifecycle status, [`STATUS_READY`] here.
    pub status: String,
    /// Why provisioning failed; never set for a store this service materialised.
    pub failure_reason: Option<String>,
    /// How many mutations this store has accepted. The etag is derived from it, so the
    /// etag is distinct for every version of the store rather than for every *content*
    /// of the store: re-putting the value a key already holds still answers a new etag,
    /// which is what invalidates the `If-Match` the caller just spent.
    pub revision: u64,
}

impl KeyValueStore {
    /// A freshly materialised, empty store at version zero.
    fn new(kvs_arn: String, now: SystemTime) -> Self {
        Self {
            etag: etag_for(&kvs_arn, 0),
            kvs_arn,
            keys: BTreeMap::new(),
            created: now,
            last_modified: now,
            status: STATUS_READY.to_string(),
            failure_reason: None,
            revision: 0,
        }
    }

    /// `ItemCount`: how many pairs the store actually holds, derived on every read.
    pub fn item_count(&self) -> i32 {
        i32::try_from(self.keys.len()).unwrap_or(i32::MAX)
    }

    /// `TotalSizeInBytes`: the summed byte length of every stored key and value,
    /// derived on every read.
    pub fn total_size_in_bytes(&self) -> i64 {
        self.keys
            .iter()
            .map(|(key, value)| byte_len(key).saturating_add(byte_len(value)))
            .sum()
    }

    /// The pair count and byte size this store WOULD hold after `changes` were applied,
    /// computed with lookups only — the existing map is never copied.
    ///
    /// Call this before [`KeyValueStore::apply`] and refuse the request when the result
    /// is outside [`within_quota`]: that is what keeps a batch all-or-nothing, since
    /// nothing can fail once application starts.
    pub fn projected_after(&self, changes: &PendingChanges<'_>) -> (usize, i64) {
        let mut count = self.keys.len();
        let mut bytes = self.total_size_in_bytes();
        for (key, change) in changes {
            match (self.keys.get(*key), change) {
                (Some(old), None) => {
                    count -= 1;
                    bytes = bytes.saturating_sub(byte_len(key).saturating_add(byte_len(old)));
                }
                (None, None) => {}
                (Some(old), Some(new)) => {
                    bytes = bytes
                        .saturating_add(byte_len(new))
                        .saturating_sub(byte_len(old));
                }
                (None, Some(new)) => {
                    count += 1;
                    bytes = bytes.saturating_add(byte_len(key).saturating_add(byte_len(new)));
                }
            }
        }
        (count, bytes)
    }

    /// Apply `changes` in place. Infallible, so either the whole batch lands or — when
    /// the caller refused it on the projection above — none of it does.
    pub fn apply(&mut self, changes: &PendingChanges<'_>) {
        for (key, change) in changes {
            match change {
                None => {
                    self.keys.remove(*key);
                }
                Some(value) => {
                    self.keys.insert((*key).to_string(), (*value).to_string());
                }
            }
        }
    }

    /// Record an accepted mutation: advance the version, mint the new etag from it, and
    /// stamp `LastModified`. Call this ONCE per accepted mutating request, after the
    /// `If-Match` check and after the whole batch has been applied, so a refused request
    /// leaves the version — and therefore every caller's `If-Match` — untouched.
    pub fn commit(&mut self, now: SystemTime) {
        self.revision = self.revision.saturating_add(1);
        self.etag = etag_for(&self.kvs_arn, self.revision);
        self.last_modified = now;
    }
}

/// Per-service state for com-amazonaws-cloudfrontkeyvaluestore-cloudfrontkeyvaluestore. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCloudfrontkeyvaluestoreCloudfrontkeyvaluestoreState {
    /// Every key value store this emulator has materialised, by ARN. Bounded by
    /// [`MAX_STORES`].
    pub stores: HashMap<String, KeyValueStore>,
}

impl ComAmazonawsCloudfrontkeyvaluestoreCloudfrontkeyvaluestoreState {
    /// The store `kvs_arn` names, WITHOUT materialising anything.
    ///
    /// `Ok(None)` is a well-formed ARN this emulator holds no record for — a store that
    /// has no pairs, so a read answers the empty view. `Err` is an identifier that names
    /// no key value store. This is the accessor every read-only operation uses, so a
    /// read allocates nothing however many distinct ARNs a caller names.
    pub fn store(&self, kvs_arn: &str) -> Result<Option<&KeyValueStore>, StoreError> {
        if !is_key_value_store_arn(kvs_arn) {
            return Err(StoreError::NotAKeyValueStoreArn);
        }
        Ok(self.stores.get(kvs_arn))
    }

    /// The store `kvs_arn` names, materialising its record on first reference.
    ///
    /// Refuses an identifier that names no key value store, and refuses to materialise
    /// a new record once [`MAX_STORES`] are held — an existing store is still served in
    /// that case, so the ceiling never breaks a scenario already under way.
    pub fn store_mut(
        &mut self,
        kvs_arn: &str,
        now: SystemTime,
    ) -> Result<&mut KeyValueStore, StoreError> {
        if !is_key_value_store_arn(kvs_arn) {
            return Err(StoreError::NotAKeyValueStoreArn);
        }
        if !self.stores.contains_key(kvs_arn) && self.stores.len() >= MAX_STORES {
            return Err(StoreError::StoreLimitReached);
        }
        Ok(self
            .stores
            .entry(kvs_arn.to_string())
            .or_insert_with(|| KeyValueStore::new(kvs_arn.to_string(), now)))
    }
}

/// Whether `arn` is a CloudFront key value store ARN —
/// `arn:<partition>:cloudfront::<account-id>:key-value-store/<uuid>`.
///
/// CloudFront is a global service, so the region field of its ARNs is empty; the
/// account field is a 12-digit account id, and the resource is the single
/// `key-value-store/<id>` path whose id is the UUID CloudFront mints for a store.
/// Anything else names no key value store.
pub fn is_key_value_store_arn(arn: &str) -> bool {
    let mut fields = arn.splitn(6, ':');
    let (
        Some("arn"),
        Some(partition),
        Some(CLOUDFRONT_SERVICE),
        Some(""),
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
        return false;
    };
    if partition.is_empty()
        || !partition
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return false;
    }
    if account.len() != 12 || !account.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    resource.strip_prefix(KVS_RESOURCE_PREFIX).is_some_and(is_uuid)
}

/// Whether `id` has the shape of the UUID CloudFront mints as a key value store id:
/// five hex groups of 8-4-4-4-12 characters.
fn is_uuid(id: &str) -> bool {
    let mut groups = id.split('-');
    for width in [8usize, 4, 4, 4, 12] {
        let Some(group) = groups.next() else {
            return false;
        };
        if group.len() != width || !group.bytes().all(|b| b.is_ascii_hexdigit()) {
            return false;
        }
    }
    groups.next().is_none()
}

/// A string's byte length as the wire type `TotalSizeInBytes` carries.
fn byte_len(value: &str) -> i64 {
    i64::try_from(value.len()).unwrap_or(i64::MAX)
}

/// The version identifier a store carries before it has accepted any mutation.
///
/// A mutating operation compares `If-Match` against this when the emulator holds no
/// record for a well-formed ARN yet, so the comparison can be made — and a mismatch
/// refused — WITHOUT materialising anything.
pub fn initial_etag(kvs_arn: &str) -> String {
    etag_for(kvs_arn, 0)
}

/// The opaque version identifier of `kvs_arn` at `revision`.
///
/// Derived rather than random so one store's versions are reproducible within a run and
/// two stores never collide, and hashed so the value carries no structure a client could
/// read or predict a successor from — an ETag is opaque to its holder.
fn etag_for(kvs_arn: &str, revision: u64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(kvs_arn.as_bytes());
    hasher.update([0u8]);
    hasher.update(revision.to_be_bytes());
    let digest = hasher.finalize();
    format!("KV{}", hex::encode_upper(&digest[..10]))
}

/// A `SystemTime` as the wire timestamp type the modelled `Created` / `LastModified`
/// members carry.
pub fn as_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_nanos(),
    )
}

/// Encode a `ListKeys` page token naming the last key returned on a page.
///
/// Hex-encoded so the token is one opaque URL-safe word rather than the key itself:
/// a key may hold any character, including the `&` and `=` a query string reserves.
pub fn encode_page_token(last_key: &str) -> String {
    hex::encode(last_key.as_bytes())
}

/// Recover the key a page token names, or `None` when the token was not produced by
/// [`encode_page_token`] — the caller refuses that request rather than silently
/// restarting the listing from the top.
pub fn decode_page_token(token: &str) -> Option<String> {
    let bytes = hex::decode(token).ok()?;
    String::from_utf8(bytes).ok()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCloudfrontkeyvaluestoreCloudfrontkeyvaluestoreState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCloudfrontkeyvaluestoreCloudfrontkeyvaluestoreState>)
}
