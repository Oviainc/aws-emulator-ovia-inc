// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ebs-ebs service.
//!
//! The EBS direct APIs are a write-then-read loop over one resource — a snapshot and
//! the blocks written into it — so that is exactly what this store holds. Every handler
//! in `src/ops/` computes its response from this record; nothing here is derived from a
//! request at read time.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

/// The block size, in bytes, of every snapshot this service creates.
///
/// `PutSnapshotBlockRequest$DataLength` documents "Currently, the only supported size
/// is `524288` bytes. Valid values: `524288`", so a snapshot's `BlockSize` is that same
/// value and a write of any other length is refused.
pub const SNAPSHOT_BLOCK_SIZE_BYTES: i32 = 524_288;

/// `StartSnapshotRequest$Timeout` documents: "If no value is specified, the timeout
/// defaults to 60 minutes."
pub const DEFAULT_SNAPSHOT_TIMEOUT_MINUTES: i32 = 60;

/// `StartSnapshotRequest$VolumeSize` documents "The maximum size is 65536 GiB (64
/// TiB)"; the model's own `range` only carries the minimum, so the maximum is enforced
/// by the handler.
pub const MAX_VOLUME_SIZE_GIB: i64 = 65_536;

/// The only checksum algorithm the model's `ChecksumAlgorithm` enum declares, and the
/// one `PutSnapshotBlockRequest$Checksum` documents ("A Base64-encoded SHA256 checksum
/// of the data. Only SHA256 checksums are supported.").
pub const CHECKSUM_ALGORITHM_SHA256: &str = "SHA256";

/// Blocks in one GiB of volume.
///
/// `PutSnapshotBlockRequest$BlockIndex` documents "A block index is a logical index in
/// units of 512 KiB blocks … (logical offset of data/524288)", so a volume of N GiB has
/// exactly `N * BLOCKS_PER_GIB` addressable block indexes and an index at or past that
/// names an offset outside the volume.
pub const BLOCKS_PER_GIB: i64 = (1024 * 1024 * 1024) / (SNAPSHOT_BLOCK_SIZE_BYTES as i64);

/// Emulator storage guard: the most blocks one snapshot may hold in this store.
///
/// Not an AWS limit — AWS stores snapshot blocks durably, this emulator holds them in
/// memory — so it is answered with the modelled `ServiceQuotaExceededException` (HTTP
/// 402, "Your current service quotas do not allow you to perform this action") rather
/// than dressed up as a validation failure. At 512 KiB a block this caps one snapshot
/// at 128 MiB.
pub const MAX_STORED_BLOCKS_PER_SNAPSHOT: usize = 256;

/// Emulator storage guard: the most blocks this store holds across every snapshot,
/// capping resident block bytes at 256 MiB. See [`MAX_STORED_BLOCKS_PER_SNAPSHOT`] for
/// why this is a quota rather than a validation.
pub const MAX_STORED_BLOCKS_TOTAL: usize = 512;

/// Emulator storage guard: the most snapshot records this store holds. Also bounds
/// `client_tokens`, which only ever gains a row alongside a stored snapshot.
pub const MAX_STORED_SNAPSHOTS: usize = 1024;

/// The number of block indexes a volume of `volume_size` GiB addresses.
pub fn blocks_in_volume(volume_size: i64) -> i64 {
    volume_size.saturating_mul(BLOCKS_PER_GIB)
}

/// A snapshot's lifecycle status, one of the modeled `Status` enum's values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotStatus {
    /// Blocks may still be written; the snapshot has not been sealed.
    Pending,
    /// `CompleteSnapshot` sealed the snapshot; no further writes are accepted.
    Completed,
    /// The snapshot was cancelled — here, by its `Timeout` elapsing while pending.
    Error,
}

impl SnapshotStatus {
    /// The wire value for this status: one of `Status`'s modeled values.
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::Error => "error",
        }
    }
}

/// The server-side encryption a snapshot reports through `StartSnapshotResponse$SseType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SseType {
    /// Not encrypted.
    None,
    /// Encrypted with the EBS-managed key.
    SseEbs,
    /// Encrypted with a caller-named KMS key.
    SseKms,
}

impl SseType {
    /// The wire value for this type: one of `SSEType`'s modeled values.
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::SseEbs => "sse-ebs",
            Self::SseKms => "sse-kms",
        }
    }
}

/// A tag stored on a snapshot. Mirrors the modeled `Tag` shape, whose members are both
/// optional.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredTag {
    pub key: Option<String>,
    pub value: Option<String>,
}

/// One block of a snapshot, as `PutSnapshotBlock` persisted it.
#[derive(Debug, Clone)]
pub struct BlockEntry {
    /// The bytes written, verbatim. `GetSnapshotBlock` returns exactly these.
    pub data: Vec<u8>,
    /// The length the write declared, which equals `data.len()`.
    pub data_length: i32,
    /// Base64 SHA256 of [`data`](Self::data), computed when the block was stored.
    pub checksum: String,
    /// The algorithm [`checksum`](Self::checksum) was computed with.
    pub checksum_algorithm: String,
    /// The token `ListSnapshotBlocks` hands out for this block and
    /// `GetSnapshotBlock` resolves. Minted when the block is first written, so a
    /// listing never has to mint (and store) anything.
    pub block_token: String,
    /// The write progress the caller reported, if any.
    pub progress: Option<i32>,
}

/// A snapshot and everything written into it.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub snapshot_id: String,
    pub owner_id: String,
    pub status: SnapshotStatus,
    pub volume_size: i64,
    pub block_size: i32,
    pub start_time: SystemTime,
    /// When the snapshot's `Timeout` elapses: also the instant the block tokens it
    /// hands out stop being valid, which is what `ListSnapshotBlocks$ExpiryTime`
    /// reports ("The time when the `BlockToken` expires").
    pub expiry_time: SystemTime,
    pub description: Option<String>,
    pub tags: Vec<StoredTag>,
    pub parent_snapshot_id: Option<String>,
    pub kms_key_arn: Option<String>,
    pub encrypted: bool,
    pub sse_type: SseType,
    pub timeout_minutes: i32,
    /// The blocks written, keyed by `BlockIndex`. A `BTreeMap` because
    /// `ListSnapshotBlocks` enumerates ascending by index and pages from an index.
    pub blocks: BTreeMap<i32, BlockEntry>,
}

/// What a `ClientToken` remembers, so a repeat of the SAME request replays the original
/// answer while a repeat with DIFFERENT parameters is a `ConflictException`.
#[derive(Debug, Clone)]
pub struct StartSnapshotIdempotencyRecord {
    pub snapshot_id: String,
    pub request_fingerprint: String,
}

/// Per-service state for com-amazonaws-ebs-ebs. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsEbsEbsState {
    /// Every snapshot, keyed by its `snap-…` id.
    pub snapshots: HashMap<String, Snapshot>,
    /// `StartSnapshot` idempotency, keyed by `ClientToken`.
    pub client_tokens: HashMap<String, StartSnapshotIdempotencyRecord>,
    /// The earliest instant at which some stored snapshot's `Timeout` falls due, or
    /// `None` when nothing is pending. [`Self::expire_due_snapshots`] returns without
    /// scanning until that instant passes, so per-request cost does not grow with the
    /// number of stored snapshots.
    next_expiry_due: Option<SystemTime>,
    /// Bumped on every minted id or token, so two mints in the same nanosecond still
    /// differ.
    mint_counter: u64,
}

impl ComAmazonawsEbsEbsState {
    /// Mint a fresh snapshot id of the modeled form `^snap-[0-9a-f]+$`.
    ///
    /// Unique by construction: the loop re-mints on the (vanishing) chance of a
    /// collision with a snapshot already stored.
    pub fn mint_snapshot_id(&mut self) -> String {
        loop {
            let digest = self.mint_digest(b"snapshot-id", &[]);
            // 17 hex characters, the width EC2 snapshot ids use, well inside the
            // modeled 1..64 length.
            let candidate = format!("snap-{}", &hex::encode(digest)[..17]);
            if !self.snapshots.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Mint the opaque block token for `(snapshot_id, block_index)`.
    ///
    /// Base64 of a 32-byte digest, so the value matches the modeled `BlockToken`
    /// pattern `^[A-Za-z0-9+/=]+$` and its 0..256 length.
    ///
    /// The token is stored on the [`BlockEntry`] it names and NOWHERE else: there is no
    /// side table of live tokens to keep in step or to reclaim, so `GetSnapshotBlock`
    /// resolves a token by comparing it with the entry at the `(SnapshotId, BlockIndex)`
    /// the request already carries. A block whose entry is replaced or dropped therefore
    /// takes its token with it — a superseded or expired token simply stops resolving,
    /// and no token row can outlive the block it named.
    pub fn mint_block_token(&mut self, snapshot_id: &str, block_index: i32) -> String {
        let digest = self.mint_digest(
            b"block-token",
            &[snapshot_id.as_bytes(), &block_index.to_be_bytes()],
        );
        ::aws_smithy_types::base64::encode(digest)
    }

    /// Materialise the `Timeout` the clock has already made due.
    ///
    /// `StartSnapshotRequest$Timeout` documents that a snapshot is "automatically
    /// cancelled" when it is not completed within that many minutes; a cancelled
    /// snapshot is what the modeled `Status` enum calls `error`. This is owed at
    /// `expiry_time` whether or not anybody calls, so every handler applies it before
    /// reading — it never moves a status the clock did not already owe.
    ///
    /// A cancelled snapshot can never be completed, nor its blocks listed or fetched
    /// again, so this is also where their bytes are released (and with them their block
    /// tokens, which live on the entries): it is this store's retention policy, not just
    /// a status flip.
    pub fn expire_due_snapshots(&mut self, now: SystemTime) {
        // Nothing is due yet, so do not walk the map on every request.
        if self.next_expiry_due.is_none_or(|due| now < due) {
            return;
        }
        for snapshot in self.snapshots.values_mut() {
            if snapshot.status == SnapshotStatus::Pending && now >= snapshot.expiry_time {
                snapshot.status = SnapshotStatus::Error;
                snapshot.blocks.clear();
            }
        }
        self.refresh_expiry_schedule();
    }

    /// Recompute the earliest `Timeout` still outstanding. Call after storing a snapshot.
    pub fn refresh_expiry_schedule(&mut self) {
        self.next_expiry_due = self
            .snapshots
            .values()
            .filter(|snapshot| snapshot.status == SnapshotStatus::Pending)
            .map(|snapshot| snapshot.expiry_time)
            .min();
    }

    /// The blocks this store currently holds across every snapshot.
    pub fn stored_block_count(&self) -> usize {
        self.snapshots
            .values()
            .map(|snapshot| snapshot.blocks.len())
            .sum()
    }

    /// A 32-byte digest unique to this mint: the domain, the caller's extra parts, a
    /// monotonically bumped counter and the wall clock.
    fn mint_digest(&mut self, domain: &[u8], parts: &[&[u8]]) -> [u8; 32] {
        self.mint_counter = self.mint_counter.wrapping_add(1);
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let mut hasher = Sha256::new();
        hasher.update(domain);
        for part in parts {
            hasher.update(part);
        }
        hasher.update(self.mint_counter.to_be_bytes());
        hasher.update(nanos.to_be_bytes());
        hasher.finalize().into()
    }
}

/// Base64 SHA256 of `data`, the form `Checksum` is documented to carry.
pub fn checksum_base64_sha256(data: &[u8]) -> String {
    ::aws_smithy_types::base64::encode(Sha256::digest(data))
}

/// Hex SHA256 of `data`. Used for the `StartSnapshot` idempotency fingerprint, which is
/// never put on the wire.
pub fn fingerprint_hex_sha256(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// The expiry instant of a snapshot started at `start_time` with a `Timeout` of
/// `timeout_minutes`.
pub fn expiry_from_timeout(start_time: SystemTime, timeout_minutes: i32) -> SystemTime {
    let minutes = u64::try_from(timeout_minutes).unwrap_or(0);
    start_time + Duration::from_secs(minutes.saturating_mul(60))
}

/// Encode the opaque `NextToken` that resumes a `ListSnapshotBlocks` page at
/// `next_block_index` of `snapshot_id`.
///
/// Derived, not stored: a listing is a read, so it must not write a page-token record
/// that a later caller would observe. Base64 keeps the value inside the modeled
/// `PageToken` pattern `^[A-Za-z0-9+/=]+$`.
pub fn encode_page_token(snapshot_id: &str, next_block_index: i32) -> String {
    ::aws_smithy_types::base64::encode(format!("{snapshot_id}:{next_block_index}"))
}

/// The inverse of [`encode_page_token`]; `None` for anything this service did not mint.
pub fn decode_page_token(token: &str) -> Option<(String, i32)> {
    let raw = ::aws_smithy_types::base64::decode(token).ok()?;
    let text = String::from_utf8(raw).ok()?;
    let (snapshot_id, index) = text.rsplit_once(':')?;
    Some((snapshot_id.to_string(), index.parse().ok()?))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEbsEbsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEbsEbsState>)
}
