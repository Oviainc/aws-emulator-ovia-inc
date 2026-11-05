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

pub mod kms_reference;

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

/// The expression a `blockToken` must satisfy, as the LIVE service reports it in its
/// validation refusal: `1 validation error detected: Value '' at 'blockToken' failed to
/// satisfy constraint: Member must satisfy regular expression pattern:
/// ^[A-Za-z0-9+/=_-]+$`.
///
/// Wider than the on-disk model's `BlockToken` `@pattern`
/// (`smithy_query(query="shape",shape="com.amazonaws.ebs#BlockToken")` reports
/// `^[A-Za-z0-9+/=]+$`): the service accepts the URL-safe base64 alphabet, so `_` and `-`
/// are admitted. The member therefore carries `skip_validation` on
/// `GetSnapshotBlockInput` and `crate::ops::get_snapshot_block` enforces THIS expression,
/// reporting the one it enforced.
pub const BLOCK_TOKEN_PATTERN: &str = "^[A-Za-z0-9+/=_-]+$";

/// The modelled `BlockToken` `@length`, which `skip_validation` takes off the boundary
/// along with the pattern, so `crate::ops::get_snapshot_block` enforces it too.
pub const BLOCK_TOKEN_LENGTH: (i64, i64) = (0, 256);

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
///
/// A snapshot that is not encrypted has NO SSE type: the member is optional with no
/// `@default`, and the recorded response of an unencrypted `StartSnapshot` carries no
/// `SseType` key at all. So "no server-side encryption" is `Option::None` — the member is
/// omitted — rather than the modelled `none` value, and this enum holds only the types a
/// snapshot can actually have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SseType {
    /// Encrypted with the EBS-managed key.
    SseEbs,
    /// Encrypted with a caller-named KMS key.
    SseKms,
}

impl SseType {
    /// The wire value for this type: one of `SSEType`'s modeled values.
    pub const fn as_wire(self) -> &'static str {
        match self {
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

/// One block of a snapshot: the bytes `PutSnapshotBlock` persisted, or the ones
/// `StartSnapshot` copied from a parent snapshot.
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
    /// The server-side encryption this snapshot has, or `None` when it has none — in
    /// which case `SseType` is omitted from a response rather than reported as `none`.
    pub sse_type: Option<SseType>,
    pub timeout_minutes: i32,
    /// The blocks written, keyed by `BlockIndex`. A `BTreeMap` because
    /// `ListSnapshotBlocks` enumerates ascending by index and pages from an index.
    pub blocks: BTreeMap<i32, BlockEntry>,
}

impl Snapshot {
    /// Whether this snapshot's content is final: `CompleteSnapshot` sealed it.
    ///
    /// "You cannot write new blocks to a snapshot after it has been completed", so this
    /// is also the answer to "may a block still be written", negated.
    pub fn is_sealed(&self) -> bool {
        self.status == SnapshotStatus::Completed
    }

    /// Whether `PutSnapshotBlock` may still write into this snapshot: "The target
    /// snapshot must be in the pending state".
    pub fn accepts_writes(&self) -> bool {
        self.status == SnapshotStatus::Pending
    }

    /// Whether the READ path — the operations that report a snapshot's blocks — can see
    /// this snapshot at all.
    ///
    /// Only a snapshot whose content is final is visible to a read: `CompleteSnapshot`
    /// "seals and completes the snapshot after all of the required blocks of data have
    /// been written to it", and until it has run the snapshot is still being assembled,
    /// while a snapshot its `Timeout` cancelled was never assembled. A read of either
    /// resolves nothing, which is the snapshot not-found refusal — not an empty listing,
    /// which would say the snapshot exists and holds no block.
    pub fn is_readable(&self) -> bool {
        self.status == SnapshotStatus::Completed
    }
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
    /// The earliest instant at which some stored snapshot owes a status transition: a
    /// `Timeout` falling due, or `None` when no snapshot is still pending.
    /// [`Self::apply_due_transitions`]
    /// returns without scanning until that instant passes, so per-request cost does not
    /// grow with the number of stored snapshots.
    next_transition_due: Option<SystemTime>,
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
    /// A 32-byte digest rendered by [`query_safe_token_text`], so the token is inside both
    /// the modeled `BlockToken` pattern `^[A-Za-z0-9+/=]+$` and the wider
    /// [`BLOCK_TOKEN_PATTERN`] the service accepts, inside [`BLOCK_TOKEN_LENGTH`], and
    /// carried in `GetSnapshotBlock`'s `blockToken` query with no escaping.
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
        query_safe_token_text(&digest)
    }

    /// Materialise every status change the CLOCK has already made due.
    ///
    /// One is owed, at an instant stored when the call that started it ran, so it was
    /// owed whether or not anybody called again: a `Timeout`.
    /// `StartSnapshotRequest$Timeout` documents that a snapshot is "automatically
    /// cancelled" when it is not completed within that many minutes; a cancelled
    /// snapshot is what the modeled `Status` enum calls `error`.
    ///
    /// Every handler applies this before reading, and it never moves a status the clock
    /// did not already owe. A sealed snapshot has nothing left to cancel, so only a
    /// snapshot still `pending` is considered.
    ///
    /// A cancelled snapshot can never be completed, nor its blocks listed or fetched
    /// again, so this is also where their bytes are released (and with them their block
    /// tokens, which live on the entries): it is this store's retention policy, not just
    /// a status flip.
    pub fn apply_due_transitions(&mut self, now: SystemTime) {
        // Nothing is due yet, so do not walk the map on every request.
        if self.next_transition_due.is_none_or(|due| now < due) {
            return;
        }
        for snapshot in self.snapshots.values_mut() {
            if snapshot.status != SnapshotStatus::Pending {
                continue;
            }
            if now >= snapshot.expiry_time {
                snapshot.status = SnapshotStatus::Error;
                snapshot.blocks.clear();
            }
        }
        self.refresh_transition_schedule();
    }

    /// Recompute the earliest transition still outstanding. Call after storing a
    /// snapshot or sealing one.
    pub fn refresh_transition_schedule(&mut self) {
        self.next_transition_due = self
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

// ---------------------------------------------------------------------------
// How this service words the refusals whose text is part of the contract
// ---------------------------------------------------------------------------
//
// A refusal's status and error code are always the contract. Its MESSAGE is only
// part of the contract where the rule that produces it is known — a template the
// service fills in from the request, or a fixed sentence it always sends. Those are
// the ones below, each stated once so every operation that raises the same refusal
// words it the same way; every other refusal in this crate words itself at the site
// that raises it, because no published rule pins the words.

/// The widths of the hex suffix of a snapshot id the service accepts: the original
/// 8-character EC2 id and the 17-character one it mints today.
///
/// The modelled `SnapshotId` pattern `^snap-[0-9a-f]+$` with `@length` 1..64
/// (`smithy_query(query="shape",shape="com.amazonaws.ebs#SnapshotId")`) admits ANY width,
/// so the boundary lets `snap-0` through; the service accepts only these two widths and
/// refuses every other as a malformed PARAMETER — a different refusal from a well-formed
/// id it holds no snapshot for.
pub const SNAPSHOT_ID_HEX_WIDTHS: [usize; 2] = [8, 17];

/// Whether `snapshot_id` has the form a snapshot id the service would mint has:
/// `snap-` followed by exactly one of [`SNAPSHOT_ID_HEX_WIDTHS`] lowercase hex digits,
/// not all of them zero.
///
/// The hex suffix is the resource's number: zero is not one, so an all-zero id is as
/// malformed as one of the wrong width. (The recorded refusals show both halves: an
/// all-zero id of an accepted width is refused as a malformed PARAMETER, while an id of
/// the same width carrying non-zero digits is refused as a snapshot that does not
/// exist.)
///
/// Form only — it says nothing about whether a snapshot with that id is stored.
pub fn is_well_formed_snapshot_id(snapshot_id: &str) -> bool {
    let Some(hex) = snapshot_id.strip_prefix("snap-") else {
        return false;
    };
    SNAPSHOT_ID_HEX_WIDTHS.contains(&hex.len())
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && hex.bytes().any(|byte| byte != b'0')
}

/// The message this service answers with when a snapshot id is not even shaped like one,
/// for ANY operation that takes one: a request-derived template naming the rejected value
/// and the parameter it arrived in, `Value (<value>) for parameter <parameter> is
/// invalid. Expected: 'snap-...'.`
///
/// `parameter` is the member's wire name, lower-camelled as the service reports it
/// (`snapshotId`, `parentSnapshotId`).
pub fn malformed_snapshot_id_message(parameter: &str, value: &str) -> String {
    format!("Value ({value}) for parameter {parameter} is invalid. Expected: 'snap-...'.")
}

/// The message `GetSnapshotBlock` answers with when a `BlockToken` does not name a block
/// of the snapshot at the requested index: a request-derived template naming the rejected
/// token, `The block token <BlockToken> is invalid.`
///
/// It states nothing about the block index: a token is the only thing that names a block,
/// so a token that resolves to nothing is the refusal whatever the index holds.
pub fn invalid_block_token_message(block_token: &str) -> String {
    format!("The block token {block_token} is invalid.")
}

/// The message this service answers with when a request names a snapshot that is not
/// stored, for ANY operation that resolves a snapshot id.
///
/// A request-derived template naming the snapshot id, shared by every snapshot
/// not-found refusal: `The snapshot 'snap-…' does not exist.` It says only that the id
/// does not resolve — never that a snapshot exists but is in some other state, which is
/// a different refusal (a `ValidationException` naming the state).
pub fn snapshot_not_found_message(snapshot_id: &str) -> String {
    format!("The snapshot '{snapshot_id}' does not exist.")
}

/// The message `PutSnapshotBlock` answers with when the request carries no
/// `x-amz-Checksum` at all.
///
/// FIXED and request-independent: the block payload is excluded from the request's
/// SigV4 signature (`PutSnapshotBlock` is `@unsignedPayload`) and that header is what
/// binds the payload to it, so a request without one carries no block the service can
/// accept and the refusal names the missing binding rather than echoing anything the
/// request did carry.
pub const MISSING_CHECKSUM_MESSAGE: &str = "Missing checksum for request payload";

/// The message this service answers with when a member the model marks `@required` is
/// absent from the request, for ANY operation and member.
///
/// The framework's own `ValidationException` envelope — `1 validation error detected:
/// Value null at '<member>' failed to satisfy constraint: Member must not be null` —
/// rendered by the same helper the request boundary uses, so an absent member the
/// SERVICE reports is worded exactly as one the boundary reports; only the envelope it
/// travels in differs, and this one carries the modelled `Message`/`Reason` a client
/// reads. The path is the member's name lower-camelled, which is the casing that
/// envelope uses.
pub fn absent_required_member_message(smithy_member_name: &str) -> String {
    ::aws_smithy_schema_server::missing_member_message(
        &::aws_smithy_schema_server::constraints::path::member(smithy_member_name),
    )
}

/// The message `PutSnapshotBlock` answers with when the checksum the request presented
/// for its block payload is not the checksum of the bytes that arrived.
///
/// A request-derived template naming BOTH values: the one computed over the received
/// payload and the one the request supplied in `x-amz-Checksum` (the member's
/// `@httpHeader` binding), so a caller can tell a corrupted upload from a checksum it
/// computed over the wrong bytes.
pub fn checksum_mismatch_message(computed: &str, supplied: &str) -> String {
    format!(
        "The value passed in as x-amz-Checksum does not match the computed checksum. \
         Computed checksum: {computed} expected checksum: {supplied}"
    )
}

/// The message this service answers with when a request asks for a checksum algorithm
/// the service does not support, for ANY operation carrying `ChecksumAlgorithm`.
///
/// A request-derived template naming the rejected algorithm and the supported set, which
/// is [`CHECKSUM_ALGORITHM_SHA256`] — the only value the modelled `ChecksumAlgorithm`
/// enum declares.
pub fn unsupported_checksum_algorithm_message(algorithm: &str) -> String {
    format!(
        "Unsupported Checksum Algorithm '{algorithm}', supported algorithms are \
         {CHECKSUM_ALGORITHM_SHA256}"
    )
}

/// The message `StartSnapshot` answers with when `VolumeSize` is outside the sizes the
/// service accepts: a request-derived template naming only the rejected value.
///
/// It states no bound: the refusal reports which input was rejected, and the bound that
/// rejected it ([`MAX_VOLUME_SIZE_GIB`], from `StartSnapshotRequest$VolumeSize`'s "The
/// maximum size is 65536 GiB (64 TiB)") is documentation, not part of this sentence.
pub fn invalid_volume_size_message(volume_size: i64) -> String {
    format!("Invalid volume size: {volume_size}")
}

/// The message `StartSnapshot` answers with when a request asks for a snapshot taken
/// from a parent AND for encryption: a snapshot inherits its parent's encryption, so the
/// two cannot be requested together.
///
/// FIXED and request-independent: the sentence names the two input members and
/// interpolates nothing — not the parent id, not the volume, not any other value the
/// request carried. The second member is named as the service names it in this refusal,
/// which is not the spelling the input member carries.
pub const PARENT_SNAPSHOT_ID_AND_ENCRYPT_MESSAGE: &str =
    "ParentSnapshotId and Encrypt cannot be specified together";

/// The message `PutSnapshotBlock` answers with when `DataLength` is not the block size
/// this service writes.
///
/// A request-derived template naming the rejected length and the one length the service
/// accepts, which is [`SNAPSHOT_BLOCK_SIZE_BYTES`] —
/// `PutSnapshotBlockRequest$DataLength` documents "Currently, the only supported size is
/// 524288 bytes. Valid values: 524288". It states no other derived quantity.
pub fn invalid_block_data_length_message(data_length: i32) -> String {
    format!("Block data length {data_length} is not valid, expected {SNAPSHOT_BLOCK_SIZE_BYTES}")
}

/// The message `PutSnapshotBlock` answers with when `BlockIndex` names an offset outside
/// the snapshot's volume.
///
/// A request-derived template naming only the rejected index: the extent that rejected it
/// follows from the snapshot's own `VolumeSize` (`BlockIndex` is "a logical index in units
/// of 512 KiB blocks", so an index at or past [`blocks_in_volume`] is outside it), and
/// that derived bound is not part of this sentence.
pub fn block_index_beyond_volume_message(block_index: i32) -> String {
    format!("Block index {block_index} is beyond the volume size extent")
}

/// The message `PutSnapshotBlock` answers with when the block payload the request
/// DECLARED cannot be read off it.
///
/// FIXED and request-independent: the sentence reports that the read failed and
/// interpolates neither the declared `DataLength` nor how many bytes did arrive — a
/// payload the service could not read gives it no block to describe.
pub const FAILED_TO_READ_BLOCK_DATA_MESSAGE: &str = "Failed to read block data";

/// The message the modelled `ConflictException` (HTTP 409) carries when a `ClientToken`
/// is reused for a request that does not match the one it was stored with.
///
/// FIXED and request-independent: the refusal does not echo the token, the operation or
/// any other caller-supplied value.
pub const CLIENT_TOKEN_CONFLICT_MESSAGE: &str =
    "Current request does not match request stored with this token.";

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
/// that a later caller would observe. [`query_safe_token_text`] keeps the value inside the
/// modeled `PageToken` pattern `^[A-Za-z0-9+/=]+$` and carryable in the `pageToken` query
/// with no escaping.
pub fn encode_page_token(snapshot_id: &str, next_block_index: i32) -> String {
    query_safe_token_text(format!("{snapshot_id}:{next_block_index}").as_bytes())
}

/// The inverse of [`encode_page_token`]; `None` for anything this service did not mint.
pub fn decode_page_token(token: &str) -> Option<(String, i32)> {
    let raw = hex::decode(token).ok()?;
    let text = String::from_utf8(raw).ok()?;
    let (snapshot_id, index) = text.rsplit_once(':')?;
    Some((snapshot_id.to_string(), index.parse().ok()?))
}

/// The text of a token this service mints for a caller to hand back in a URI QUERY: a
/// `BlockToken` (`GetSnapshotBlock`'s `blockToken`) or a page token (`ListSnapshotBlocks`
/// and `ListChangedBlocks`'s `pageToken`).
///
/// Lowercase hex of `bytes`, so the token is drawn from `[0-9a-f]` alone — characters RFC
/// 3986 §2.3 leaves unreserved. A query value made only of those needs no escaping, so
/// every client spells the token on the wire exactly as this service minted it, and the
/// bytes a request signature is computed over are the bytes the token is read back from.
/// A token spelt with characters a query reserves (`+`, `/`, `=`, which standard base64
/// uses) instead travels in whichever of several forms the client chose, and the same
/// token then arrives as different query bytes from different callers.
///
/// Opacity is unchanged: the caller is given a value to return verbatim, and 32 bytes of
/// digest render as 64 characters, well inside the modelled `@length` 0..256 of both
/// `BlockToken` and `PageToken`.
pub fn query_safe_token_text(bytes: &[u8]) -> String {
    hex::encode(bytes)
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
